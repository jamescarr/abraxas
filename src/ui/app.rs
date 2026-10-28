//! The application: shared context (`Ctx`), the screen stack, overlays, effects, generation
//! pumping and the real terminal loop.
//!
//! [`App::step`] and [`App::render`] never touch the terminal, so tests drive the whole app
//! through `ratatui::backend::TestBackend`; only [`run`] owns real I/O.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use rand::{RngExt, SeedableRng};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use tachyonfx::{EffectManager, IntoEffect, fx};

use crate::cli::GlobalOpts;
use crate::config::{Config, Paths, TruecolorMode};
use crate::director::{Director, DirectorEvent, JobKind};
use crate::game::host::{DoorSignal, DoorView};
use crate::game::programs;
use crate::game::save::{CallerRecord, CallerStatus, SaveStore};
use crate::game::seed::ensure_level;
use crate::game::state::{
    Backdoor, ClockReading, GenSource, HostInfo, IrcFile, Level, Passage, Phase, Place, RunState,
    Sysop,
};
use crate::hidden::board::{self, HIDDEN_BOT};
use crate::hidden::{HiddenEvent, HiddenLine};
use crate::llm::{Llm, OpenAiCompat};
use crate::pack::model::{ActView, Character, HostOs, PaletteId, PostDef, StoryPack, VisionKind};
use crate::palette::{Dos, Palette};
use crate::rng::Rng;
use crate::ui::canvas::{self, HEIGHT, STATUS_ROW, WIDTH};
use crate::ui::crt;
use crate::ui::keys::{self, Key, classify};
use crate::ui::screens::{
    BootScreen, DeathScreen, DialerScreen, MenuScreen, Nav, Screen, ScreenImpl, help,
};
use crate::ui::status::{ModemLights, render_status};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::visions::Vision;

// ---------------------------------------------------------------------------------------------
// Overlays and effects
// ---------------------------------------------------------------------------------------------

/// A modal layer drawn over the current screen. While one is open the screen gets no keys and no
/// ticks (so the trace clock pauses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// The key and command reference (F1 or `?`); Esc closes.
    Help,
    /// `Hang up? [Y/n]`: Y/Enter autosaves and quits, N/Esc closes.
    ConfirmQuit,
    /// A message box; any key closes.
    Notice(String),
}

/// A visual effect a screen asks for by pushing it on `ctx.fx`. The app turns these into
/// tachyonfx effects over the canvas. `area` rectangles are **canvas-local** (0,0 is the
/// canvas's top-left corner, 80x25). In calm mode every command is ignored.
#[derive(Debug, Clone, PartialEq)]
pub enum FxCommand {
    /// Dissolve the previous frame (`ctx.last_frame`) away over `ms`, revealing the new screen.
    Dissolve { ms: u64 },
    /// Text in `area` fades in from black over `ms`.
    FadeFromFg { ms: u64, area: Rect },
    /// The whole canvas reforms from scattered cells over `ms`.
    Coalesce { ms: u64 },
    /// Shift the foreground HSL of `area` (hue in degrees, saturation and lightness in percent
    /// points) over `ms`, then hold the shift until the screen changes or `Clear`.
    HslShiftFg {
        area: Rect,
        hue: f32,
        sat: f32,
        light: f32,
        ms: u64,
    },
    /// A burst of random cell glitches for `ms`.
    Glitch { ms: u64 },
    /// Cancel every running effect.
    Clear,
}

/// Keys of the effects that replace themselves when requested again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum FxKey {
    #[default]
    Transition,
    Tint,
    Glitch,
}

// ---------------------------------------------------------------------------------------------
// Ctx
// ---------------------------------------------------------------------------------------------

/// The streamed epilogue text, filled from `EpilogueDelta`/`EpilogueDone` events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpilogueBuf {
    /// Text received so far (the final text once `done`).
    pub text: String,
    /// Set by `EpilogueDone`.
    pub source: Option<GenSource>,
    pub done: bool,
}

/// Everything screens share. Fields are public; prefer the helper methods where one exists.
pub struct Ctx {
    pub config: Config,
    pub paths: Paths,
    /// The command-line options (`seed`, `pack`, `dev`, ...).
    pub opts: GlobalOpts,
    pub pack: Arc<StoryPack>,
    pub director: Director,
    /// The receiving end of the director's channel. The app drains it every frame; screens never
    /// read it (use `chat_inbox`, `streams`, `epilogue` instead).
    pub events: Receiver<DirectorEvent>,
    pub store: SaveStore,
    /// The current run; `None` before login and after death.
    pub run: Option<RunState>,
    pub lights: ModemLights,
    pub palette: Palette,
    /// `--dev`: `xyzzy`, generation-source footers.
    pub dev: bool,
    /// Total time the app has been running.
    pub elapsed: Duration,
    /// Real seconds spent in the current level (drives the in-world clock; resets when the run's
    /// depth changes).
    pub level_elapsed: f32,
    /// Effect requests; drained by the app every frame.
    pub fx: Vec<FxCommand>,
    /// Set to open an overlay on the next frame (e.g. `Some(Overlay::ConfirmQuit)` or a
    /// `Notice`). Taken by the app.
    pub overlay_request: Option<Overlay>,
    /// UI randomness (not part of the run's reproducible randomness).
    pub rng: Rng,
    /// Interlude text received so far, by depth (deltas, then the final text).
    pub streams: BTreeMap<u32, String>,
    /// `ChatDelta`/`ChatDone` events not yet consumed; the Chat screen takes them with
    /// `std::mem::take` and matches `session` ids.
    pub chat_inbox: Vec<DirectorEvent>,
    /// The epilogue stream; the Ending screen resets it with `EpilogueBuf::default()` before
    /// requesting an epilogue.
    pub epilogue: EpilogueBuf,
    /// The previous frame's canvas (80x25, origin 0,0), for dissolve and the mirror vision.
    pub last_frame: Buffer,
    /// Time left of the noise-vision burst that plays when generation fell back to the offline
    /// generator after an AI failure (400 ms; never in calm mode).
    pub noise_burst: Duration,
    /// The hidden line: the Unix-socket server of the current board. Idle (no socket) until
    /// `listen` succeeds, so tests never bind.
    pub hidden: HiddenLine,
    /// What socket sessions ask of the run (backdoors, downloads, learned attacks). The app
    /// drains it every frame.
    pub hidden_events: Receiver<HiddenEvent>,
    /// Signals from hidden-line sessions waiting for the open door: the Run screen drains them
    /// every tick; the app drops them while no door is open.
    pub door_signals: Vec<DoorSignal>,
}

impl Ctx {
    /// Build a context: the save store, an mpsc channel, the director and the colour palette.
    /// `llm` is `None` for offline play.
    pub fn new(
        config: Config,
        paths: Paths,
        opts: GlobalOpts,
        pack: Arc<StoryPack>,
        llm: Option<Arc<dyn Llm>>,
    ) -> Ctx {
        let (tx, rx) = channel();
        let (hidden_tx, hidden_rx) = channel();
        let director = Director::new(llm, pack.clone(), config.ai.clone(), tx);
        let palette = Palette::detect(config.display.truecolor);
        Ctx {
            store: SaveStore::new(&paths),
            dev: opts.dev,
            rng: Rng::seed_from_u64(rand::random()),
            palette,
            director,
            events: rx,
            run: None,
            lights: ModemLights::new(),
            elapsed: Duration::ZERO,
            level_elapsed: 0.0,
            fx: Vec::new(),
            overlay_request: None,
            streams: BTreeMap::new(),
            chat_inbox: Vec::new(),
            epilogue: EpilogueBuf::default(),
            noise_burst: Duration::ZERO,
            hidden: HiddenLine::idle(hidden_tx, config.display.baud),
            hidden_events: hidden_rx,
            door_signals: Vec::new(),
            last_frame: Buffer::empty(Rect::new(0, 0, WIDTH, HEIGHT)),
            config,
            paths,
            opts,
            pack,
        }
    }

    /// Calm mode: no flicker, glitch, dissolve or CRT; visions freeze.
    pub fn calm(&self) -> bool {
        self.config.display.calm
    }

    /// Modem speed in baud (0 = instant text).
    pub fn baud(&self) -> u32 {
        self.config.display.baud
    }

    pub fn run_ref(&self) -> Option<&RunState> {
        self.run.as_ref()
    }

    pub fn run_mut(&mut self) -> Option<&mut RunState> {
        self.run.as_mut()
    }

    /// Save the run through the store. Failures are logged and ignored; the game goes on.
    pub fn autosave(&self) {
        if let Some(run) = &self.run
            && let Err(e) = self.store.save(run)
        {
            tracing::warn!("autosave failed: {e:#}");
        }
    }

    /// Record the player in `callers.json` with `status` at the current depth and clock.
    /// Failures are logged and ignored. Does nothing without a run.
    pub fn record_caller(&self, status: CallerStatus) {
        let (Some(run), Some(clock)) = (&self.run, self.clock()) else {
            return;
        };
        let rec = CallerRecord {
            handle: run.handle.clone(),
            depth: run.depth,
            clock: clock.hms(),
            status,
        };
        if let Err(e) = self.store.record_caller(rec) {
            tracing::warn!("recording caller failed: {e:#}");
        }
    }

    /// The in-world clock at the current depth, `None` without a run.
    pub fn clock(&self) -> Option<ClockReading> {
        self.run
            .as_ref()
            .map(|r| r.clock(&self.pack, self.level_elapsed))
    }

    /// The act of the current depth, falling back to the first act (and the endless tail beyond
    /// the authored acts). `None` only for a pack without acts.
    pub fn act(&self) -> Option<ActView<'_>> {
        let depth = self.run.as_ref().map_or(0, |r| r.depth);
        self.pack
            .act_for_depth(depth)
            .or_else(|| self.pack.act_for_depth(0))
    }

    /// The current act's palette id (prism when the pack has no acts).
    pub fn act_palette(&self) -> PaletteId {
        self.act().map_or(PaletteId::Prism, |a| a.palette)
    }

    /// The current act's six gradient colours, dark to bright.
    pub fn gradient(&self) -> [Color; 6] {
        self.palette.gradient(self.act_palette())
    }

    /// The current act's chrome (border/accent) colour.
    pub fn chrome(&self) -> Color {
        self.palette.chrome(self.act_palette())
    }

    /// Body text style: light gray on black.
    pub fn text_style(&self) -> Style {
        self.palette.style(Dos::LightGray, Dos::Black)
    }

    /// `fg` on black.
    pub fn fg(&self, fg: Dos) -> Style {
        self.palette.style(fg, Dos::Black)
    }

    /// The current level (`run.levels[depth]`).
    pub fn level(&self) -> Option<&Level> {
        self.run.as_ref().and_then(RunState::level)
    }

    pub fn place(&self) -> Option<&Place> {
        self.level().and_then(|l| l.place.as_ref())
    }

    pub fn passage(&self) -> Option<&Passage> {
        self.level().and_then(|l| l.passage.as_ref())
    }

    /// The cast member in focus at this level.
    pub fn focus_character(&self) -> Option<&Character> {
        let id = self.level()?.seed.focus.as_deref()?;
        self.pack.character(id)
    }

    /// Whether to show the `[place ai ...] [passage ...]` footer (`--dev`).
    pub fn is_dev_footer(&self) -> bool {
        self.dev
    }
}

/// A [`Place`] with plausible content for tests of screens and widgets.
pub fn sample_place(title: impl Into<String>) -> Place {
    Place {
        title: title.into(),
        bbs_name: "SOL NIGER".into(),
        tagline: "the sun shines black".into(),
        level_card: "Wet carpet. A payphone rings somewhere below.".into(),
        sysop: Sysop {
            handle: "TESTSYS".into(),
            cast_id: None,
        },
        host: HostInfo {
            name: "ODS TEST EXCHANGE".into(),
            os: HostOs::Unix,
            banner: "ODS".into(),
            objective: "Retrieve WREN-01.LOG.".into(),
            target_file: "WREN-01.LOG".into(),
            objective_ice: None,
            nodes: None,
            node_names: (0..8).map(|i| format!("NODE{i}")).collect(),
            ice_names: (0..3).map(|i| format!("ICE{i}")).collect(),
        },
        posts: vec![PostDef {
            from: "anonymous".into(),
            subject: "re: front door".into(),
            body: "whoever runs this never changed the front door.".into(),
        }],
        text_files: Vec::new(),
        host_files: Vec::new(),
        words: vec!["RAVENS".into()],
        password: "RAVENS".into(),
    }
}

/// A context for tests (also usable by other modules' tests): offline director, `Paths::in_dir`
/// `dir`, the embedded pack, baud 0, calm off, CRT off, indexed (16-colour) palette, `dev` on,
/// `opts.seed = Some(42)` and a UI rng seeded with 42. No run exists yet.
///
/// # Panics
/// If the embedded pack is invalid (the pack tests guarantee it is not).
pub fn test_ctx(dir: &Path) -> Ctx {
    let mut config = Config::default();
    config.ai.enabled = false;
    config.display.baud = 0;
    config.display.calm = false;
    config.display.crt = false;
    config.display.truecolor = TruecolorMode::Never;
    config.ans.dir = dir.join("ans");
    let opts = GlobalOpts {
        offline: true,
        seed: Some(42),
        baud: Some(0),
        dev: true,
        ..GlobalOpts::default()
    };
    let pack = match crate::pack::embedded() {
        Ok(p) => Arc::new(p),
        Err(e) => panic!("the embedded story pack is invalid:\n{e}"),
    };
    let mut ctx = Ctx::new(config, Paths::in_dir(dir.to_path_buf()), opts, pack, None);
    ctx.rng = Rng::seed_from_u64(42);
    ctx
}

/// Test helper: install a registered run (`TESTER`) at `depth` as `ctx.run`, with the levels at
/// `depth` and `depth + 1` fully populated by the offline generator (place, passage and
/// interlude), as if generation had already finished. The run is in `Phase::Arrival`.
pub fn install_test_run(ctx: &mut Ctx, depth: u32) {
    use crate::director::offline;
    use crate::director::validate::GenContext;
    use crate::game::state::{GenSource, RunState};

    let seed = ctx.opts.seed.unwrap_or(42);
    let mut run = RunState::new(
        "Tester",
        "Test Person",
        "a dream",
        seed,
        ctx.pack.meta.id.clone(),
        None,
    );
    run.depth = depth;
    for d in [depth, depth + 1] {
        ensure_level(&mut run, &ctx.pack, d);
        let Some(level_seed) = run.levels.get(&d).map(|l| l.seed.clone()) else {
            continue;
        };
        let generated = {
            let gen_ctx = GenContext {
                run: &run,
                pack: &ctx.pack,
                seed: &level_seed,
            };
            let place = offline::place(&gen_ctx);
            let passage = offline::passage(&gen_ctx, &place);
            let interlude = offline::interlude(&gen_ctx, &place, None);
            (place, passage, interlude)
        };
        let source = GenSource::Offline {
            reason: "test".to_string(),
        };
        if let Some(level) = run.levels.get_mut(&d) {
            level.place = Some(generated.0);
            level.place_source = Some(source.clone());
            level.passage = Some(generated.1);
            level.passage_source = Some(source.clone());
            level.interlude = Some(generated.2);
            level.interlude_source = Some(source);
        }
    }
    ctx.run = Some(run);
}

// ---------------------------------------------------------------------------------------------
// Generation pumping
// ---------------------------------------------------------------------------------------------

/// How long the noise burst after an AI fallback lasts.
const NOISE_BURST: Duration = Duration::from_millis(400);

/// One generation request the run needs right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenNeed {
    Place(u32),
    Passage(u32),
    Interlude(u32),
}

fn choice_made(run: &RunState) -> bool {
    use crate::game::state::Phase;
    run.chronicle
        .entry(run.depth)
        .is_some_and(|e| e.choice.is_some())
        || matches!(
            run.phase,
            Phase::Hub {
                choice: Some(_),
                ..
            } | Phase::Descending { .. }
        )
}

/// The generation requests `run` needs that are not already pending. Pure: the same inputs give
/// the same answer, which is what makes pumping every frame idempotent.
///
/// For the current depth `c`: `place(c)` when missing; `passage(c)` once `place(c)` is ready.
/// For `c + 1` (only when the pack has an act there, so a finished pack with endless disabled
/// stops at its last depth): `place(c+1)` as soon as `place(c)` exists; `passage(c+1)` once the
/// choice at `c` is made and `place(c+1)` is ready; `interlude(c+1)` while descending and
/// `place(c+1)` is ready.
pub fn generation_needs(
    run: &RunState,
    pack: &StoryPack,
    is_pending: impl Fn(JobKind, u32) -> bool,
) -> Vec<GenNeed> {
    let mut needs = Vec::new();
    let c = run.depth;
    let place_ready = |d: u32| run.levels.get(&d).is_some_and(|l| l.place.is_some());
    let passage_ready = |d: u32| run.levels.get(&d).is_some_and(|l| l.passage.is_some());
    let interlude_ready = |d: u32| run.levels.get(&d).is_some_and(|l| l.interlude.is_some());

    if !place_ready(c) {
        if !is_pending(JobKind::Place, c) {
            needs.push(GenNeed::Place(c));
        }
        return needs;
    }
    if !passage_ready(c) && !is_pending(JobKind::Passage, c) {
        needs.push(GenNeed::Passage(c));
    }
    let next = c + 1;
    if pack.act_for_depth(next).is_none() {
        return needs;
    }
    if !place_ready(next) {
        if !is_pending(JobKind::Place, next) {
            needs.push(GenNeed::Place(next));
        }
        return needs;
    }
    if choice_made(run) && !passage_ready(next) && !is_pending(JobKind::Passage, next) {
        needs.push(GenNeed::Passage(next));
    }
    if matches!(run.phase, crate::game::state::Phase::Descending { .. })
        && !interlude_ready(next)
        && !is_pending(JobKind::Interlude, next)
    {
        needs.push(GenNeed::Interlude(next));
    }
    needs
}

/// Request whatever [`generation_needs`] reports, creating level skeletons as needed. Call once
/// per frame; does nothing without a run.
pub fn pump_generation(ctx: &mut Ctx) {
    let Ctx {
        run,
        pack,
        director,
        ..
    } = ctx;
    let Some(run) = run.as_mut() else {
        return;
    };
    let needs = generation_needs(run, pack, |kind, depth| director.is_pending(kind, depth));
    for need in needs {
        match need {
            GenNeed::Place(d) => {
                ensure_level(run, pack, d);
                director.request_place(run, d);
            }
            GenNeed::Passage(d) => {
                ensure_level(run, pack, d);
                director.request_passage(run, d);
            }
            GenNeed::Interlude(d) => {
                ensure_level(run, pack, d);
                director.request_interlude(run, d);
            }
        }
    }
}

/// Fold one director event into the context: modem lights, the run's levels (with their
/// `GenSource`), `used_pool`, the streaming buffers, and an autosave when the run changed.
/// Results for depths other than the current one or the next are stale and dropped; a part the
/// level already has is kept.
pub fn apply_director_event(ctx: &mut Ctx, ev: DirectorEvent) {
    ctx.lights.on_event(&ev);
    let fell_back = match &ev {
        DirectorEvent::PlaceReady { source, .. }
        | DirectorEvent::PassageReady { source, .. }
        | DirectorEvent::InterludeDone { source, .. }
        | DirectorEvent::ChatDone { source, .. }
        | DirectorEvent::EpilogueDone { source, .. } => {
            matches!(source, GenSource::Offline { reason } if !reason.is_empty())
        }
        _ => false,
    };
    if fell_back {
        ctx.noise_burst = NOISE_BURST;
    }
    let Ctx {
        run,
        pack,
        streams,
        chat_inbox,
        epilogue,
        ..
    } = ctx;
    let mut changed = false;
    let wanted = |run: &RunState, depth: u32| depth == run.depth || depth == run.depth + 1;
    match ev {
        DirectorEvent::PlaceReady {
            depth,
            place,
            source,
        } => {
            if let Some(run) = run.as_mut().filter(|r| wanted(r, depth)) {
                let level = ensure_level(run, pack, depth);
                if level.place.is_none() {
                    let keys = place.pool_keys();
                    level.place = Some(place);
                    level.place_source = Some(source);
                    run.used_pool.extend(keys);
                    changed = true;
                }
            }
        }
        DirectorEvent::PassageReady {
            depth,
            passage,
            source,
        } => {
            if let Some(run) = run.as_mut().filter(|r| wanted(r, depth)) {
                let level = ensure_level(run, pack, depth);
                if level.passage.is_none() {
                    level.passage = Some(passage);
                    level.passage_source = Some(source);
                    changed = true;
                }
            }
        }
        DirectorEvent::InterludeDelta { depth, text } => {
            streams.entry(depth).or_default().push_str(&text);
        }
        DirectorEvent::InterludeDone {
            depth,
            text,
            source,
        } => {
            streams.insert(depth, text.clone());
            if let Some(run) = run.as_mut().filter(|r| wanted(r, depth)) {
                let level = ensure_level(run, pack, depth);
                if level.interlude.is_none() {
                    level.interlude = Some(text);
                    level.interlude_source = Some(source);
                    changed = true;
                }
            }
        }
        ev @ (DirectorEvent::ChatDelta { .. } | DirectorEvent::ChatDone { .. }) => {
            chat_inbox.push(ev);
        }
        DirectorEvent::EpilogueDelta { text } => epilogue.text.push_str(&text),
        DirectorEvent::EpilogueDone { text, source } => {
            epilogue.text = text;
            epilogue.source = Some(source);
            epilogue.done = true;
        }
        DirectorEvent::Request { .. }
        | DirectorEvent::Chunk
        | DirectorEvent::CallOk
        | DirectorEvent::CallFailed
        | DirectorEvent::CallCancelled
        | DirectorEvent::Fallback => {}
    }
    if changed {
        ctx.autosave();
    }
}

/// Apply one request from a hidden-line session to the run and save. A backdoor for a board the
/// player has already left is ignored, and a backdoor only moves forward: a session can hold a
/// snapshot a frame old, so an event may arrive out of date.
pub fn apply_hidden_event(ctx: &mut Ctx, ev: HiddenEvent) {
    let Some(run) = ctx.run.as_mut() else {
        return;
    };
    match ev {
        HiddenEvent::Backdoor { depth, state } => {
            if depth == run.depth {
                let slot = run.backdoors.entry(depth).or_default();
                *slot = match (*slot, state) {
                    // Planted and locked are final; misses only count up.
                    (Backdoor::Planted | Backdoor::Locked, _) => *slot,
                    (Backdoor::Open { misses: held }, Backdoor::Open { misses }) => {
                        Backdoor::Open {
                            misses: held.max(misses),
                        }
                    }
                    (_, state) => state,
                };
            }
        }
        HiddenEvent::Download { depth, file } => {
            if !run.irc.has_file(depth, HIDDEN_BOT, &file.name) {
                run.irc.files.push(IrcFile {
                    depth,
                    bot: HIDDEN_BOT.to_string(),
                    file,
                });
            }
        }
        HiddenEvent::Learn(program) => {
            programs::add(&mut run.deck, program);
        }
        // The door lives and dies with its session: nothing in the run changes, nothing to save.
        HiddenEvent::Signal(signal) => {
            ctx.door_signals.push(signal);
            return;
        }
    }
    ctx.autosave();
}

// ---------------------------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------------------------

/// What identifies the board the hidden line serves: run seed, depth, door attempt, and whether
/// the level's place exists yet. A different key is a different board. (The exit taken is set a
/// moment before the depth changes; it counts once the new depth does.)
type BoardKey = (u64, u32, u32, bool);

/// The running application: the current screen, the stack of screens under it, the overlay, the
/// effect manager and the context.
pub struct App {
    pub ctx: Ctx,
    pub screen: Screen,
    /// Screens kept underneath by `Nav::Push`.
    pub stack: Vec<Screen>,
    pub overlay: Option<Overlay>,
    /// Set when the app should exit.
    pub quit: bool,
    effects: EffectManager<FxKey>,
    flicker: crt::Flicker,
    /// Frame time not yet fed to the effects (consumed by `render`).
    pending_dt: Duration,
    /// Where the canvas was last drawn (used to place effects); starts at the origin.
    canvas: Rect,
    tracked_depth: Option<u32>,
    /// The board last offered to the hidden line.
    hidden_key: Option<BoardKey>,
    /// The door last offered to the hidden line.
    door_key: Option<DoorView>,
}

impl App {
    /// A new app showing the Boot screen.
    pub fn new(mut ctx: Ctx) -> App {
        let boot = Screen::Boot(BootScreen::new(&mut ctx));
        App::with_screen(ctx, boot)
    }

    /// A new app showing `screen` (for tests and resuming).
    pub fn with_screen(mut ctx: Ctx, screen: Screen) -> App {
        let flicker = crt::Flicker::new(ctx.rng.random());
        let mut app = App {
            ctx: {
                ctx.fx.clear();
                ctx
            },
            screen,
            stack: Vec::new(),
            overlay: None,
            quit: false,
            effects: EffectManager::default(),
            flicker,
            pending_dt: Duration::ZERO,
            canvas: Rect::new(0, 0, WIDTH, HEIGHT),
            tracked_depth: None,
            hidden_key: None,
            door_key: None,
        };
        app.drain_fx();
        app
    }

    /// Advance one frame: drain director events, pump generation, handle `event` (only key
    /// presses matter), tick the screen (unless an overlay is open) and apply what it asked for.
    /// Performs no I/O.
    pub fn step(&mut self, event: Option<Event>, dt: Duration) {
        self.ctx.elapsed += dt;
        self.pending_dt += dt;
        self.ctx.lights.tick(dt);
        self.ctx.noise_burst = self.ctx.noise_burst.saturating_sub(dt);
        self.track_level_clock(dt);

        while let Ok(ev) = self.ctx.events.try_recv() {
            self.ctx.director.on_event(&ev);
            apply_director_event(&mut self.ctx, ev);
        }
        let mut touched = false;
        while let Ok(ev) = self.ctx.hidden_events.try_recv() {
            apply_hidden_event(&mut self.ctx, ev);
            touched = true;
        }
        // A signal sent while no shell is ticking (Beat, Choice, Menu, after death) is dropped.
        if !matches!(self.screen, Screen::Run(_)) {
            self.ctx.door_signals.clear();
        }
        self.publish_board(touched);
        // While the player chats, the next level waits: a chat reply must not queue behind a
        // multi-minute prefetch on a model server that runs one request at a time.
        let chatting = matches!(self.screen, Screen::Chat(_));
        let hold = chatting
            .then(|| self.ctx.run.as_ref().map(|r| r.depth))
            .flatten();
        self.ctx.director.hold_prefetch(hold);
        pump_generation(&mut self.ctx);

        if let Some(Event::Key(key)) = event
            && keys::is_press(&key)
        {
            self.handle_key(key);
        }
        if self.overlay.is_none() && !self.quit {
            let nav = self.screen.tick(dt, &mut self.ctx);
            self.apply_nav(nav);
        }
        self.check_ego_death();
        let door = match &self.screen {
            Screen::Run(run) => run.door_view(),
            _ => None,
        };
        if door != self.door_key {
            self.door_key = door;
            self.ctx.hidden.set_door(door);
        }
        if let Some(ov) = self.ctx.overlay_request.take() {
            self.overlay = Some(ov);
        }
        self.drain_fx();
    }

    /// EGO 0 ends the run wherever it happens: whatever screen is up, the Death screen takes over.
    fn check_ego_death(&mut self) {
        let dead = self.ctx.run.as_ref().is_some_and(|r| r.stats.is_dead());
        if dead && !matches!(self.screen, Screen::Death(_)) && !self.quit {
            let death = Screen::Death(DeathScreen::new(&mut self.ctx));
            self.apply_nav(Nav::To(Box::new(death)));
        }
    }

    fn track_level_clock(&mut self, dt: Duration) {
        let depth = self.ctx.run.as_ref().map(|r| r.depth);
        if depth != self.tracked_depth {
            self.tracked_depth = depth;
            self.ctx.level_elapsed = 0.0;
        } else if depth.is_some() {
            self.ctx.level_elapsed += dt.as_secs_f32();
        }
    }

    /// Offer the current board to the hidden line. A different board (new level, a new door
    /// attempt, the place arriving) is published and bumps the generation, so a connected session
    /// reloads; the same board after the player's own GET/TYPE/TAP is only refreshed, or the
    /// session would reset in the middle of a TAP.
    fn publish_board(&mut self, touched: bool) {
        if self.ctx.hidden.path().is_none() {
            return;
        }
        // Mid-descent the old board stays up: the exit taken is already the next level's but the
        // depth has not changed, so a snapshot now would describe a host that never existed.
        if self
            .ctx
            .run
            .as_ref()
            .is_some_and(|r| matches!(r.phase, Phase::Descending { .. }))
        {
            return;
        }
        let key = self.ctx.run.as_ref().map(|r| {
            (
                r.seed,
                r.depth,
                r.door_attempts,
                r.level().is_some_and(|l| l.place.is_some()),
            )
        });
        let changed = key != self.hidden_key;
        if !changed && !touched {
            return;
        }
        self.hidden_key = key;
        let ctx = &self.ctx;
        let snapshot = ctx
            .run
            .as_ref()
            .and_then(|run| board::snapshot(run, &ctx.pack, &ctx.config.ans.dir));
        if changed {
            ctx.hidden.publish(snapshot);
        } else {
            ctx.hidden.refresh(snapshot);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if let Some(overlay) = &self.overlay {
            match (overlay, classify(&key)) {
                (Overlay::ConfirmQuit, Key::Char('y' | 'Y') | Key::Enter) => self.quit_now(),
                (Overlay::ConfirmQuit, Key::Char('n' | 'N') | Key::Esc) => self.overlay = None,
                (Overlay::ConfirmQuit, _) => {}
                (Overlay::Help, Key::Esc | Key::F(1) | Key::Char('q' | 'Q')) => {
                    self.overlay = None;
                }
                (Overlay::Help, _) => {}
                (Overlay::Notice(_), _) => self.overlay = None,
            }
            return;
        }
        match classify(&key) {
            _ if keys::is_quit_chord(&key) => self.overlay = Some(Overlay::ConfirmQuit),
            Key::F(1) => self.overlay = Some(Overlay::Help),
            Key::Char('?') if !self.screen.in_text_input() => self.overlay = Some(Overlay::Help),
            _ => {
                let nav = self.screen.handle_key(key, &mut self.ctx);
                self.apply_nav(nav);
            }
        }
    }

    /// Apply a screen's navigation request. Tint effects end whenever the screen changes.
    pub fn apply_nav(&mut self, nav: Nav) {
        match nav {
            Nav::Stay => {}
            Nav::To(next) => {
                self.screen_changed();
                // Level-root screens start a fresh stack: nothing underneath them is still valid.
                if matches!(
                    *next,
                    Screen::Dialer(_)
                        | Screen::Dialing(_)
                        | Screen::Login(_)
                        | Screen::LevelCard(_)
                        | Screen::Menu(_)
                        | Screen::Death(_)
                ) {
                    self.stack.clear();
                }
                self.screen = *next;
            }
            Nav::Push(next) => {
                self.screen_changed();
                let under = std::mem::replace(&mut self.screen, *next);
                self.stack.push(under);
            }
            Nav::Back => {
                self.screen_changed();
                self.screen = match self.stack.pop() {
                    Some(s) => s,
                    None if self.ctx.run.is_some() => Screen::Menu(MenuScreen::new(&mut self.ctx)),
                    None => Screen::Dialer(DialerScreen::new(&mut self.ctx)),
                };
            }
            Nav::Quit => self.quit_now(),
        }
    }

    fn screen_changed(&mut self) {
        // Commands the outgoing screen queued (a dissolve, say) still play.
        self.drain_fx();
        self.effects.cancel_unique_effect(FxKey::Tint);
    }

    /// Autosave the run, stop generation and leave.
    fn quit_now(&mut self) {
        self.ctx.autosave();
        self.ctx.director.cancel_all();
        self.quit = true;
    }

    /// Turn queued `FxCommand`s into tachyonfx effects.
    fn drain_fx(&mut self) {
        let cmds = std::mem::take(&mut self.ctx.fx);
        if self.ctx.calm() {
            return;
        }
        let origin = self.canvas;
        let abs = |r: Rect| Rect::new(origin.x + r.x, origin.y + r.y, r.width, r.height);
        for cmd in cmds {
            match cmd {
                FxCommand::Dissolve { ms } => {
                    let fx = dissolve_from(self.ctx.last_frame.clone(), ms);
                    self.effects.add_unique_effect(FxKey::Transition, fx);
                }
                FxCommand::Coalesce { ms } => {
                    self.effects
                        .add_unique_effect(FxKey::Transition, fx::coalesce(ms_u32(ms)));
                }
                FxCommand::FadeFromFg { ms, area } => {
                    let black = self.ctx.palette.color(Dos::Black);
                    self.effects
                        .add_effect(fx::fade_from_fg(black, ms_u32(ms)).with_area(abs(area)));
                }
                FxCommand::HslShiftFg {
                    area,
                    hue,
                    sat,
                    light,
                    ms,
                } => {
                    let shift = fx::hsl_shift_fg([hue, sat, light], ms_u32(ms));
                    self.effects.add_unique_effect(
                        FxKey::Tint,
                        fx::never_complete(shift).with_area(abs(area)),
                    );
                }
                FxCommand::Glitch { ms } => {
                    let glitch = tachyonfx::fx::Glitch::builder()
                        .cell_glitch_ratio(0.04)
                        .action_start_delay_ms(0..150)
                        .action_ms(80..300)
                        .build()
                        .into_effect();
                    self.effects.add_unique_effect(
                        FxKey::Glitch,
                        fx::timed_never_complete(Duration::from_millis(ms), glitch),
                    );
                }
                FxCommand::Clear => self.effects = EffectManager::default(),
            }
        }
    }

    /// Whether any effect is running.
    pub fn effects_running(&self) -> bool {
        self.effects.is_running()
    }

    /// Draw one frame: black everywhere, the 80x25 canvas centred (screen, status bar, effects,
    /// overlay, CRT pass), or only the too-small message on a smaller terminal. The canvas
    /// before the overlay and CRT pass is copied into `ctx.last_frame`.
    pub fn render(&mut self, frame: &mut Frame) {
        let term = frame.area();
        let black = self.ctx.palette.style(Dos::LightGray, Dos::Black);
        frame.buffer_mut().set_style(term, black);
        let Some(c) = canvas::canvas_rect(term) else {
            canvas::render_too_small(frame.buffer_mut(), term, black);
            self.pending_dt = Duration::ZERO;
            return;
        };
        self.canvas = c;
        self.screen.render(frame, c, &self.ctx);
        if self.screen.wants_status() && self.ctx.run.is_some() {
            let row = Rect::new(c.x, c.y + STATUS_ROW, WIDTH, 1);
            render_status(&self.ctx, frame.buffer_mut(), row);
        }
        let dt = std::mem::take(&mut self.pending_dt);
        self.effects.process_effects(dt, frame.buffer_mut(), c);
        copy_canvas(frame.buffer_mut(), c, &mut self.ctx.last_frame);
        if !self.ctx.noise_burst.is_zero() && !self.ctx.calm() {
            let mut noise = Vision::new(
                VisionKind::Noise,
                self.ctx.opts.seed.unwrap_or(0),
                0,
                self.ctx.gradient(),
                false,
            );
            noise.tick(self.ctx.elapsed);
            noise.render(c, frame.buffer_mut());
        }
        if let Some(overlay) = &self.overlay {
            render_overlay(overlay, frame.buffer_mut(), c, &self.ctx);
        }
        if crt::enabled(&self.ctx.config, &self.ctx.palette) {
            let factor = self.flicker.factor(self.ctx.elapsed);
            crt::apply(frame.buffer_mut(), c, factor);
        }
    }
}

fn ms_u32(ms: u64) -> u32 {
    u32::try_from(ms).unwrap_or(u32::MAX)
}

/// Copy the canvas `c` of `src` into `dst` (an 80x25 buffer at the origin).
fn copy_canvas(src: &Buffer, c: Rect, dst: &mut Buffer) {
    for y in 0..c.height {
        for x in 0..c.width {
            dst[(x, y)].clone_from(&src[(c.x + x, c.y + y)]);
        }
    }
}

/// An effect that shows `snapshot` (canvas-local cells) over the new content, dissolving cell by
/// cell (a fixed scatter) as time passes, so the old screen erodes into the new one.
fn dissolve_from(snapshot: Buffer, ms: u64) -> tachyonfx::Effect {
    fx::effect_fn_buf(snapshot, ms_u32(ms), |snap, ctx, buf| {
        let alpha = ctx.alpha();
        let area = ctx.area;
        for y in 0..area.height {
            for x in 0..area.width {
                let scatter =
                    u32::from(x).wrapping_mul(73_856_093) ^ u32::from(y).wrapping_mul(19_349_663);
                let h = f32::from((scatter % 1000) as u16) / 1000.0;
                if h >= alpha
                    && let Some(old) = snap.cell((x, y))
                    && let Some(cell) = buf.cell_mut((area.x + x, area.y + y))
                {
                    cell.clone_from(old);
                }
            }
        }
    })
}

fn render_overlay(overlay: &Overlay, buf: &mut Buffer, canvas: Rect, ctx: &Ctx) {
    match overlay {
        Overlay::Help => help::render(buf, canvas, ctx),
        Overlay::ConfirmQuit => {
            let area = centred(canvas, 30, 5);
            let style = ctx.palette.style(Dos::White, Dos::Blue);
            let inner = boxed(buf, area, Some("Hang up"), BoxKind::Double, style);
            canvas::put_centered(buf, inner, inner.y + 1, "Hang up? [Y/n]", style);
        }
        Overlay::Notice(msg) => {
            let lines = crate::text::wrap(msg, 44);
            let h = u16::try_from(lines.len()).unwrap_or(10) + 4;
            let area = centred(canvas, 50, h.min(canvas.height));
            let style = ctx.palette.style(Dos::White, Dos::Blue);
            let inner = boxed(buf, area, None, BoxKind::Double, style);
            for (i, line) in lines.iter().enumerate() {
                let y = inner.y + 1 + u16::try_from(i).unwrap_or(0);
                canvas::put_centered(buf, inner, y, line, style);
            }
            let y = inner.bottom().saturating_sub(1);
            canvas::put_centered(
                buf,
                inner,
                y,
                "press any key",
                ctx.palette.style(Dos::LightCyan, Dos::Blue),
            );
        }
    }
}

fn centred(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

// ---------------------------------------------------------------------------------------------
// The real loop
// ---------------------------------------------------------------------------------------------

/// Play the game in the real terminal. Loads the pack first (pack errors go to stderr and are
/// returned before the terminal is touched), warms the AI up when enabled, and always restores
/// the terminal.
pub fn run(global: &GlobalOpts, config: Config, paths: Paths) -> anyhow::Result<()> {
    let pack = match crate::pack::open_for_run(global.pack.as_deref()) {
        Ok(p) => Arc::new(p),
        Err(errors) => {
            eprintln!("{errors}");
            anyhow::bail!("the story pack has errors");
        }
    };
    let llm: Option<Arc<dyn Llm>> = config
        .ai
        .enabled
        .then(|| Arc::new(OpenAiCompat::new(&config.ai)) as Arc<dyn Llm>);
    let ai_enabled = config.ai.enabled;
    let mut ctx = Ctx::new(config, paths, global.clone(), pack, llm);
    let socket = global
        .socket
        .clone()
        .unwrap_or_else(|| ctx.paths.socket_path());
    if let Err(e) = ctx.hidden.listen(socket.clone()) {
        tracing::warn!("hidden line: cannot listen on {}: {e}", socket.display());
    }
    if ai_enabled {
        ctx.director.warmup();
    }
    let mut app = App::new(ctx);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    app.ctx.director.cancel_all();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    let frame_time = if app.ctx.calm() {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(33)
    };
    let mut last = Instant::now();
    while !app.quit {
        terminal.draw(|f| app.render(f))?;
        let deadline = last + frame_time;
        let mut events = Vec::new();
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            if event::poll(deadline - now)? {
                events.push(event::read()?);
                while event::poll(Duration::ZERO)? {
                    events.push(event::read()?);
                }
            }
        }
        let now = Instant::now();
        let dt = now - last;
        last = now;
        for ev in events {
            app.step(Some(ev), Duration::ZERO);
        }
        app.step(None, dt);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::host::Signal;
    use crate::game::state::Phase;
    use crate::pack::model::{ChoiceDef, ExitDef, ExitKind, OptionDef, Stance, VisionKind};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char, press_ctrl};
    use crate::ui::screens::RunScreen;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn passage() -> Passage {
        Passage {
            beat: "beat".into(),
            choice: ChoiceDef {
                prompt: "p".into(),
                options: [Stance::Integrate, Stance::Repress, Stance::Neutral]
                    .into_iter()
                    .map(|stance| OptionDef {
                        text: format!("{stance:?}"),
                        stance,
                        flags: vec![],
                    })
                    .collect(),
            },
            exits: ExitKind::ALL
                .into_iter()
                .map(|kind| ExitDef {
                    kind,
                    text: kind.as_str().into(),
                })
                .collect(),
            vision: VisionKind::Fire,
            captions: vec!["a".into(), "b".into(), "c".into()],
            summary: "s".into(),
        }
    }

    fn new_run() -> RunState {
        RunState::new("Tester", "T", "a dream", 42, "rollover", None)
    }

    fn level_with(run: &mut RunState, pack: &StoryPack, depth: u32, place: bool, passage_: bool) {
        let level = ensure_level(run, pack, depth);
        if place {
            level.place = Some(sample_place(format!("L{depth}")));
        }
        if passage_ {
            level.passage = Some(passage());
        }
    }

    fn none_pending(_: JobKind, _: u32) -> bool {
        false
    }

    // ----- generation_needs -------------------------------------------------------------

    #[test]
    fn a_fresh_run_needs_only_the_current_place() {
        let ctx_dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(ctx_dir.path());
        let run = new_run();
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Place(0)]
        );
    }

    #[test]
    fn a_ready_place_asks_for_its_passage_and_prefetches_the_next_place() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let mut run = new_run();
        level_with(&mut run, &ctx.pack, 0, true, false);
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Passage(0), GenNeed::Place(1)]
        );
    }

    #[test]
    fn pending_markers_make_the_pump_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let mut run = new_run();
        assert!(generation_needs(&run, &ctx.pack, |k, d| (k, d) == (JobKind::Place, 0)).is_empty());
        level_with(&mut run, &ctx.pack, 0, true, false);
        let pending = |k, d| matches!((k, d), (JobKind::Passage, 0) | (JobKind::Place, 1));
        assert!(generation_needs(&run, &ctx.pack, pending).is_empty());
        // Only the pending one is suppressed.
        assert_eq!(
            generation_needs(&run, &ctx.pack, |k, d| (k, d) == (JobKind::Passage, 0)),
            vec![GenNeed::Place(1)]
        );
    }

    #[test]
    fn the_next_passage_waits_for_the_choice_and_the_next_place() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let mut run = new_run();
        level_with(&mut run, &ctx.pack, 0, true, true);
        level_with(&mut run, &ctx.pack, 1, false, false);
        run.phase = Phase::Hub {
            door_done: true,
            choice: Some(0),
        };
        // Choice made, but place(1) is not ready: only the place is requested.
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Place(1)]
        );
        level_with(&mut run, &ctx.pack, 1, true, false);
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Passage(1)]
        );
        // Before the choice, nothing about depth 1 beyond its place.
        run.phase = Phase::Hub {
            door_done: false,
            choice: None,
        };
        assert!(generation_needs(&run, &ctx.pack, none_pending).is_empty());
    }

    #[test]
    fn the_interlude_is_requested_only_while_descending_and_once_the_place_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let mut run = new_run();
        level_with(&mut run, &ctx.pack, 0, true, true);
        level_with(&mut run, &ctx.pack, 1, true, true);
        run.phase = Phase::Hub {
            door_done: true,
            choice: Some(1),
        };
        assert!(generation_needs(&run, &ctx.pack, none_pending).is_empty());
        run.phase = Phase::Descending { exit: 2 };
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Interlude(1)]
        );
        assert!(
            generation_needs(&run, &ctx.pack, |k, d| (k, d) == (JobKind::Interlude, 1)).is_empty()
        );
        run.levels.get_mut(&1).unwrap().interlude = Some("text".into());
        assert!(generation_needs(&run, &ctx.pack, none_pending).is_empty());
    }

    #[test]
    fn a_chronicle_choice_also_counts_as_the_choice_being_made() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let mut run = new_run();
        level_with(&mut run, &ctx.pack, 0, true, true);
        level_with(&mut run, &ctx.pack, 1, true, false);
        assert!(generation_needs(&run, &ctx.pack, none_pending).is_empty());
        assert!(run.apply_choice(&ctx.pack, 0));
        run.phase = Phase::Arrival;
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Passage(1)]
        );
    }

    #[test]
    fn nothing_is_requested_past_the_last_depth_when_endless_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let last = ctx.pack.last_depth();
        let mut run = new_run();
        run.depth = last;
        level_with(&mut run, &ctx.pack, last, true, true);
        assert_eq!(
            generation_needs(&run, &ctx.pack, none_pending),
            vec![GenNeed::Place(last + 1)],
            "endless is on in the rollover pack"
        );
        let mut closed = (*ctx.pack).clone();
        closed.meta.endless.as_mut().unwrap().enabled = false;
        assert!(generation_needs(&run, &closed, none_pending).is_empty());
        // The current depth is still served.
        run.levels.get_mut(&last).unwrap().passage = None;
        assert_eq!(
            generation_needs(&run, &closed, none_pending),
            vec![GenNeed::Passage(last)]
        );
    }

    #[test]
    fn pumping_without_a_run_does_nothing_and_with_a_run_creates_the_level_skeleton() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        pump_generation(&mut ctx);
        assert!(ctx.run.is_none());
        ctx.run = Some(new_run());
        pump_generation(&mut ctx);
        let run = ctx.run.as_ref().unwrap();
        assert!(run.levels.contains_key(&0));
        assert!(!run.levels.contains_key(&1), "place(0) comes first");
    }

    // ----- apply_director_event ---------------------------------------------------------

    fn ctx_with_run(dir: &Path) -> Ctx {
        let mut ctx = test_ctx(dir);
        ctx.run = Some(new_run());
        ctx
    }

    #[test]
    fn place_ready_lands_in_the_right_level_with_its_source_and_extends_used_pool() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_with_run(dir.path());
        let place = sample_place("THE DRIP");
        apply_director_event(
            &mut ctx,
            DirectorEvent::PlaceReady {
                depth: 1,
                place: place.clone(),
                source: GenSource::Ai {
                    model: "m".into(),
                    millis: 1500,
                },
            },
        );
        let run = ctx.run.as_ref().unwrap();
        let level = &run.levels[&1];
        assert_eq!(level.place.as_ref(), Some(&place));
        assert_eq!(
            level.place_source,
            Some(GenSource::Ai {
                model: "m".into(),
                millis: 1500
            })
        );
        assert_eq!(level.seed.depth, 1);
        for key in place.pool_keys() {
            assert!(run.used_pool.contains(&key), "{key}");
        }
        assert!(!run.levels.contains_key(&0), "no other level is created");
        assert!(ctx.store.exists("tester"), "stored events autosave");
    }

    #[test]
    fn stale_and_duplicate_results_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_with_run(dir.path());
        let ready = |depth, title: &str, source| DirectorEvent::PlaceReady {
            depth,
            place: sample_place(title),
            source,
        };
        apply_director_event(&mut ctx, ready(5, "TOO FAR", GenSource::Authored));
        assert!(ctx.run.as_ref().unwrap().levels.is_empty());
        apply_director_event(&mut ctx, ready(0, "FIRST", GenSource::Authored));
        apply_director_event(
            &mut ctx,
            ready(0, "SECOND", GenSource::Offline { reason: "x".into() }),
        );
        let level = &ctx.run.as_ref().unwrap().levels[&0];
        assert_eq!(level.place.as_ref().unwrap().title, "FIRST");
        assert_eq!(level.place_source, Some(GenSource::Authored));
    }

    #[test]
    fn passage_and_interlude_are_stored_with_their_sources() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_with_run(dir.path());
        apply_director_event(
            &mut ctx,
            DirectorEvent::PassageReady {
                depth: 0,
                passage: passage(),
                source: GenSource::Offline {
                    reason: "timeout".into(),
                },
            },
        );
        apply_director_event(
            &mut ctx,
            DirectorEvent::InterludeDelta {
                depth: 1,
                text: "You fa".into(),
            },
        );
        apply_director_event(
            &mut ctx,
            DirectorEvent::InterludeDelta {
                depth: 1,
                text: "ll.".into(),
            },
        );
        assert_eq!(ctx.streams[&1], "You fall.");
        assert!(!ctx.run.as_ref().unwrap().levels.contains_key(&1));
        apply_director_event(
            &mut ctx,
            DirectorEvent::InterludeDone {
                depth: 1,
                text: "You fall for a long time.".into(),
                source: GenSource::Authored,
            },
        );
        let run = ctx.run.as_ref().unwrap();
        assert_eq!(
            run.levels[&0].passage_source,
            Some(GenSource::Offline {
                reason: "timeout".into()
            })
        );
        assert_eq!(
            run.levels[&1].interlude.as_deref(),
            Some("You fall for a long time.")
        );
        assert_eq!(run.levels[&1].interlude_source, Some(GenSource::Authored));
        assert_eq!(ctx.streams[&1], "You fall for a long time.");
    }

    #[test]
    fn chat_and_epilogue_events_go_to_their_mailboxes_and_lights_follow_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        apply_director_event(
            &mut ctx,
            DirectorEvent::ChatDelta {
                session: 3,
                text: "hel".into(),
            },
        );
        apply_director_event(
            &mut ctx,
            DirectorEvent::ChatDone {
                session: 3,
                text: "hello".into(),
                source: GenSource::Authored,
            },
        );
        assert_eq!(ctx.chat_inbox.len(), 2);
        assert!(
            matches!(&ctx.chat_inbox[1], DirectorEvent::ChatDone { session: 3, text, .. } if text == "hello")
        );
        apply_director_event(
            &mut ctx,
            DirectorEvent::EpilogueDelta {
                text: "The ".into(),
            },
        );
        apply_director_event(
            &mut ctx,
            DirectorEvent::EpilogueDelta { text: "end".into() },
        );
        assert_eq!(ctx.epilogue.text, "The end");
        assert!(!ctx.epilogue.done);
        apply_director_event(
            &mut ctx,
            DirectorEvent::EpilogueDone {
                text: "The end.".into(),
                source: GenSource::Authored,
            },
        );
        assert_eq!(ctx.epilogue.text, "The end.");
        assert!(ctx.epilogue.done);
        apply_director_event(&mut ctx, DirectorEvent::Chunk);
        assert!(ctx.lights.rd_on());
        apply_director_event(&mut ctx, DirectorEvent::CallFailed);
        assert_eq!(ctx.lights.carrier(), crate::ui::status::Carrier::Failed);
    }

    // ----- App: layout, navigation, overlays --------------------------------------------

    fn dialer_app(dir: &Path) -> App {
        let mut ctx = test_ctx(dir);
        let screen = Screen::Dialer(DialerScreen::new(&mut ctx));
        App::with_screen(ctx, screen)
    }

    fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        term.backend().buffer().clone()
    }

    fn key(c: ratatui::crossterm::event::KeyEvent) -> Option<Event> {
        Some(Event::Key(c))
    }

    #[test]
    fn the_canvas_is_centred_and_black_outside() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        let buf = draw(&mut app, 100, 40);
        let a = buf.area;
        // Canvas origin is (10, 7): the dialer's title bar starts there.
        let title: String = row_text(&buf, a, 7).chars().skip(10).take(33).collect();
        assert_eq!(title, " TERMINUS 3.5 ─ Dialing Directory");
        assert_eq!(row_text(&buf, a, 6).trim(), "");
        assert_eq!(buf[(0, 0)].bg, app.ctx.palette.color(Dos::Black));
        assert_eq!(buf[(99, 39)].bg, app.ctx.palette.color(Dos::Black));
        assert_eq!(buf[(10, 7)].bg, app.ctx.palette.color(Dos::Blue));
        assert_eq!(buf[(9, 7)].bg, app.ctx.palette.color(Dos::Black));
    }

    #[test]
    fn a_small_terminal_shows_only_the_size_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        let buf = draw(&mut app, 79, 24);
        let a = buf.area;
        let all: Vec<String> = (0..24).map(|y| row_text(&buf, a, y)).collect();
        let lit: Vec<&String> = all.iter().filter(|r| !r.trim().is_empty()).collect();
        assert_eq!(lit.len(), 1);
        assert_eq!(
            lit[0].trim(),
            "ABRAXAS needs an 80x25 terminal. This one is 79x24."
        );
    }

    #[test]
    fn install_test_run_populates_two_levels_with_valid_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 3);
        let run = ctx.run.as_ref().unwrap();
        assert_eq!(run.depth, 3);
        for d in [3, 4] {
            let level = &run.levels[&d];
            assert!(level.place.is_some() && level.passage.is_some() && level.interlude.is_some());
        }
        assert_eq!(
            run.levels[&3].place.as_ref().unwrap().title,
            "THE BLACK SUN"
        );
        assert_eq!(ctx.passage().unwrap().exits.len(), 3);
    }

    #[test]
    fn an_ai_fallback_triggers_a_noise_burst_that_expires_and_authored_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 3);
        let place = ctx.place().unwrap().clone();
        let event = |source| DirectorEvent::PlaceReady {
            depth: 4,
            place: place.clone(),
            source,
        };
        apply_director_event(&mut ctx, event(GenSource::Authored));
        apply_director_event(
            &mut ctx,
            event(GenSource::Offline {
                reason: String::new(),
            }),
        );
        assert!(
            ctx.noise_burst.is_zero(),
            "authored and plain offline mode are not failures"
        );
        apply_director_event(
            &mut ctx,
            event(GenSource::Offline {
                reason: "timed out".into(),
            }),
        );
        assert_eq!(ctx.noise_burst, NOISE_BURST);

        let mut app = App::with_screen(
            ctx,
            Screen::Dialer(DialerScreen::new(&mut test_ctx(dir.path()))),
        );
        app.step(None, Duration::from_millis(250));
        assert!(!app.ctx.noise_burst.is_zero());
        let noisy = draw(&mut app, 80, 25);
        app.step(None, Duration::from_millis(250));
        assert!(app.ctx.noise_burst.is_zero());
        let clean = draw(&mut app, 80, 25);
        assert_ne!(noisy, clean, "the burst replaces the screen while it lasts");
    }

    #[test]
    fn nav_to_a_level_root_clears_the_stack() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.run = Some(new_run());
        let boot = Screen::Boot(BootScreen::new(&mut app.ctx));
        app.apply_nav(Nav::Push(Box::new(boot)));
        let first_menu = Screen::Menu(MenuScreen::new(&mut app.ctx));
        app.apply_nav(Nav::Push(Box::new(first_menu)));
        assert_eq!(app.stack.len(), 2);
        let menu = Screen::Menu(MenuScreen::new(&mut app.ctx));
        app.apply_nav(Nav::To(Box::new(menu)));
        assert!(
            app.stack.is_empty(),
            "stale screens must not survive a level-root transition"
        );
    }

    #[test]
    fn ego_zero_hands_the_screen_to_death_from_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        let mut run = new_run();
        run.stats.ego = 0;
        app.ctx.run = Some(run);
        app.step(None, Duration::from_millis(33));
        assert_eq!(app.screen.name(), "Death");
        // Once on Death it stays (no re-entry loop).
        app.step(None, Duration::from_millis(33));
        assert_eq!(app.screen.name(), "Death");
    }

    #[test]
    fn nav_to_replaces_push_stacks_and_back_pops() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        let boot = Screen::Boot(BootScreen::new(&mut app.ctx));
        app.apply_nav(Nav::Push(Box::new(boot)));
        assert_eq!(app.screen.name(), "Boot");
        assert_eq!(app.stack.len(), 1);
        let boot2 = Screen::Boot(BootScreen::new(&mut app.ctx));
        app.apply_nav(Nav::To(Box::new(boot2)));
        assert_eq!(app.stack.len(), 1, "To keeps the stack");
        app.apply_nav(Nav::Back);
        assert_eq!(app.screen.name(), "Dialer");
        assert!(app.stack.is_empty());
        // Empty stack and no run: Back lands on the Dialer.
        app.apply_nav(Nav::Back);
        assert_eq!(app.screen.name(), "Dialer");
        // Empty stack with a run: Back lands on the Menu.
        app.ctx.run = Some(new_run());
        app.apply_nav(Nav::Back);
        assert_eq!(app.screen.name(), "Menu");
        assert!(!app.quit);
        app.apply_nav(Nav::Quit);
        assert!(app.quit);
    }

    #[test]
    fn boot_runs_into_the_dialer_on_a_key_and_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(test_ctx(dir.path()));
        assert_eq!(app.screen.name(), "Boot");
        app.step(key(press_char('x')), Duration::from_millis(16));
        assert_eq!(app.screen.name(), "Dialer");

        let mut app = App::new(test_ctx(dir.path()));
        for _ in 0..40 {
            app.step(None, Duration::from_millis(100));
        }
        assert_eq!(app.screen.name(), "Dialer");
    }

    #[test]
    fn ctrl_q_opens_the_prompt_n_closes_it_and_y_saves_and_quits() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.run = Some(new_run());
        app.step(key(press_ctrl('q')), Duration::ZERO);
        assert_eq!(app.overlay, Some(Overlay::ConfirmQuit));
        let buf = draw(&mut app, 80, 25);
        let all: String = (0..25).map(|y| row_text(&buf, buf.area, y)).collect();
        assert!(all.contains("Hang up? [Y/n]"));
        app.step(key(press_char('N')), Duration::ZERO);
        assert!(app.overlay.is_none());
        assert!(!app.quit);
        // Generation results autosave as they land; start the quit check from a clean slate.
        app.ctx.store.delete("tester").unwrap();

        app.step(key(press_ctrl('c')), Duration::ZERO);
        assert_eq!(app.overlay, Some(Overlay::ConfirmQuit));
        app.step(key(press_char('x')), Duration::ZERO);
        assert!(!app.quit, "other keys leave the prompt open");
        app.step(key(press_char('y')), Duration::ZERO);
        assert!(app.quit);
        assert!(app.ctx.store.exists("tester"), "quitting autosaves the run");
    }

    #[test]
    fn alt_x_asks_too_and_esc_dismisses() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.step(
            key(ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Char('x'),
                ratatui::crossterm::event::KeyModifiers::ALT,
            )),
            Duration::ZERO,
        );
        assert_eq!(app.overlay, Some(Overlay::ConfirmQuit));
        app.step(key(press(KeyCode::Esc)), Duration::ZERO);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn help_opens_with_f1_or_question_mark_and_blocks_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.step(key(press(KeyCode::F(1))), Duration::ZERO);
        assert_eq!(app.overlay, Some(Overlay::Help));
        // The screen gets no keys while help is open: '1' would dial entry 1.
        app.step(key(press_char('1')), Duration::ZERO);
        assert_eq!(app.screen.name(), "Dialer");
        assert_eq!(app.overlay, Some(Overlay::Help));
        app.step(key(press(KeyCode::Esc)), Duration::ZERO);
        assert!(app.overlay.is_none());
        app.step(key(press_char('?')), Duration::ZERO);
        assert_eq!(app.overlay, Some(Overlay::Help));
    }

    #[test]
    fn key_releases_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        let mut release = press_char('1');
        release.kind = ratatui::crossterm::event::KeyEventKind::Release;
        app.step(key(release), Duration::ZERO);
        assert_eq!(app.screen.name(), "Dialer");
        app.step(key(press_char('1')), Duration::ZERO);
        assert_eq!(app.screen.name(), "Dialing");
    }

    #[test]
    fn overlay_requests_from_screens_open_and_a_notice_closes_on_any_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.overlay_request = Some(Overlay::Notice("The line drops.".into()));
        app.step(None, Duration::ZERO);
        assert_eq!(app.overlay, Some(Overlay::Notice("The line drops.".into())));
        let buf = draw(&mut app, 80, 25);
        let all: String = (0..25).map(|y| row_text(&buf, buf.area, y)).collect();
        assert!(all.contains("The line drops.") && all.contains("press any key"));
        app.step(key(press_char('z')), Duration::ZERO);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn the_level_clock_resets_when_the_depth_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.step(None, Duration::from_secs(5));
        assert_eq!(app.ctx.level_elapsed, 0.0, "no run: no level clock");
        app.ctx.run = Some(new_run());
        app.step(None, Duration::from_secs(2));
        assert_eq!(
            app.ctx.level_elapsed, 0.0,
            "first frame of a run starts at 0"
        );
        app.step(None, Duration::from_secs(3));
        assert!((app.ctx.level_elapsed - 3.0).abs() < 1e-3);
        app.ctx.run.as_mut().unwrap().depth = 1;
        app.step(None, Duration::from_secs(1));
        assert_eq!(app.ctx.level_elapsed, 0.0);
    }

    // ----- Effects -----------------------------------------------------------------------

    #[test]
    fn calm_mode_ignores_every_effect_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.config.display.calm = true;
        app.ctx.fx.push(FxCommand::Coalesce { ms: 400 });
        app.ctx.fx.push(FxCommand::Glitch { ms: 1500 });
        app.ctx.fx.push(FxCommand::Dissolve { ms: 600 });
        app.step(None, Duration::ZERO);
        assert!(!app.effects_running());
        assert!(app.ctx.fx.is_empty());
        app.ctx.config.display.calm = false;
        app.ctx.fx.push(FxCommand::Coalesce { ms: 400 });
        app.step(None, Duration::ZERO);
        assert!(app.effects_running());
        app.ctx.fx.push(FxCommand::Clear);
        app.step(None, Duration::ZERO);
        assert!(!app.effects_running());
    }

    #[test]
    fn dissolve_shows_the_previous_frame_and_erodes_to_the_new_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                app.ctx.last_frame[(x, y)].set_symbol("@");
            }
        }
        app.ctx.fx.push(FxCommand::Dissolve { ms: 600 });
        app.step(None, Duration::ZERO);
        let first = draw(&mut app, 80, 25);
        let a = first.area;
        assert_eq!(
            row_text(&first, a, 0),
            "@".repeat(80),
            "frame 0 is the old screen"
        );
        // Halfway, part of each is visible.
        app.step(None, Duration::from_millis(300));
        let mid = draw(&mut app, 80, 25);
        let xs = (0..25)
            .map(|y| row_text(&mid, a, y).matches('@').count())
            .sum::<usize>();
        assert!((600..1400).contains(&xs), "{xs} of 2000 cells still old");
        app.step(None, Duration::from_millis(400));
        let end = draw(&mut app, 80, 25);
        let all: String = (0..25).map(|y| row_text(&end, a, y)).collect();
        assert!(!all.contains('@'));
        assert!(all.contains("TERMINUS 3.5"));
        assert!(
            !app.effects_running() || {
                let _ = draw(&mut app, 80, 25);
                !app.effects_running()
            }
        );
    }

    #[test]
    fn a_tint_holds_until_the_screen_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.palette = Palette::new(true);
        let plain = draw(&mut app, 80, 25)[(1, 24)].fg; // the hint bar's text (black on cyan)
        let cyan_bar = draw(&mut app, 80, 25)[(1, 0)].fg; // white title text
        app.ctx.fx.push(FxCommand::HslShiftFg {
            area: Rect::new(0, 0, 80, 1),
            hue: 0.0,
            sat: 0.0,
            light: -40.0,
            ms: 200,
        });
        app.step(None, Duration::ZERO);
        for _ in 0..10 {
            app.step(None, Duration::from_millis(100));
            let _ = draw(&mut app, 80, 25);
        }
        let held = draw(&mut app, 80, 25);
        assert_ne!(held[(1, 0)].fg, cyan_bar, "tint still applied after 1 s");
        assert_eq!(held[(1, 24)].fg, plain, "only the requested area changes");
        let boot = Screen::Boot(BootScreen::new(&mut app.ctx));
        app.apply_nav(Nav::To(Box::new(boot)));
        app.step(None, Duration::from_millis(50));
        let _ = draw(&mut app, 80, 25);
        app.step(None, Duration::from_millis(50));
        let _ = draw(&mut app, 80, 25);
        assert!(!app.effects_running(), "the tint ends with the screen");
    }

    #[test]
    fn the_crt_pass_runs_only_with_truecolor_and_not_in_calm_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.ctx.config.display.crt = true;
        app.ctx.palette = Palette::new(true);
        let on = draw(&mut app, 80, 25);
        // Row 2 is the header (even, untouched); row 3 is the divider (odd, dimmed).
        let bright = app.ctx.palette.color(Dos::DarkGray);
        assert_eq!(on[(0, 2)].bg, app.ctx.palette.color(Dos::Black));
        assert_ne!(on[(0, 3)].fg, bright);
        app.ctx.config.display.calm = true;
        let calm = draw(&mut app, 80, 25);
        assert_eq!(calm[(0, 3)].fg, bright);
        app.ctx.config.display.calm = false;
        app.ctx.palette = Palette::new(false);
        let indexed = draw(&mut app, 80, 25);
        assert_eq!(indexed[(0, 3)].fg, app.ctx.palette.color(Dos::DarkGray));
    }

    #[test]
    fn last_frame_holds_the_canvas_without_the_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = dialer_app(dir.path());
        app.overlay = Some(Overlay::ConfirmQuit);
        let _ = draw(&mut app, 100, 40);
        let a = app.ctx.last_frame.area;
        assert_eq!(a, Rect::new(0, 0, 80, 25));
        assert!(row_text(&app.ctx.last_frame, a, 0).starts_with(" TERMINUS 3.5"));
        let all: String = (0..25)
            .map(|y| row_text(&app.ctx.last_frame, a, y))
            .collect();
        assert!(!all.contains("Hang up?"));
    }

    #[test]
    fn hidden_line_events_update_the_run_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 2);
        let file = crate::pack::model::FileDef {
            name: "X.ANS".into(),
            description: "d".into(),
            body: "b".into(),
        };
        apply_hidden_event(&mut ctx, HiddenEvent::Learn(programs::Program::WinNuke));
        for _ in 0..2 {
            apply_hidden_event(
                &mut ctx,
                HiddenEvent::Download {
                    depth: 2,
                    file: file.clone(),
                },
            );
        }
        let planted = crate::game::state::Backdoor::Planted;
        apply_hidden_event(
            &mut ctx,
            HiddenEvent::Backdoor {
                depth: 2,
                state: planted,
            },
        );
        apply_hidden_event(
            &mut ctx,
            HiddenEvent::Backdoor {
                depth: 1,
                state: planted,
            },
        );
        let run = ctx.run.as_ref().unwrap();
        assert!(run.deck_owns(programs::Program::WinNuke));
        assert_eq!(run.irc.files.len(), 1);
        assert_eq!(run.irc.files[0].bot, HIDDEN_BOT);
        assert_eq!(run.backdoors.get(&2), Some(&planted));
        assert_eq!(run.backdoors.get(&1), None, "a board already left");
    }

    #[test]
    fn a_board_is_published_once_and_again_when_the_door_is_re_rolled() {
        // A short directory name keeps the socket path under the sun_path limit.
        let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.hidden.listen(dir.path().join("s.sock")).unwrap();
        install_test_run(&mut ctx, 2);
        let mut app = App::new(ctx);
        let start = app.ctx.hidden.generation();
        app.step(None, Duration::from_millis(16));
        assert_eq!(app.ctx.hidden.generation(), start + 1, "first board");
        app.step(None, Duration::from_millis(16));
        assert_eq!(app.ctx.hidden.generation(), start + 1, "same board");
        app.ctx.run.as_mut().unwrap().door_attempts += 1;
        app.step(None, Duration::from_millis(16));
        assert_eq!(
            app.ctx.hidden.generation(),
            start + 2,
            "a dump shock re-rolls"
        );
    }

    #[test]
    fn a_stale_backdoor_event_cannot_undo_a_planted_or_locked_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 2);
        let send = |ctx: &mut Ctx, state: Backdoor| {
            apply_hidden_event(ctx, HiddenEvent::Backdoor { depth: 2, state });
        };
        let held = |ctx: &Ctx| ctx.run.as_ref().unwrap().backdoors.get(&2).copied();
        send(&mut ctx, Backdoor::Open { misses: 2 });
        send(&mut ctx, Backdoor::Open { misses: 1 });
        assert_eq!(held(&ctx), Some(Backdoor::Open { misses: 2 }));
        send(&mut ctx, Backdoor::Planted);
        send(&mut ctx, Backdoor::Open { misses: 1 });
        send(&mut ctx, Backdoor::Locked);
        assert_eq!(held(&ctx), Some(Backdoor::Planted));
        ctx.run.as_mut().unwrap().backdoors.clear();
        send(&mut ctx, Backdoor::Locked);
        send(&mut ctx, Backdoor::Planted);
        assert_eq!(held(&ctx), Some(Backdoor::Locked));
    }

    #[test]
    fn the_old_board_stays_up_until_the_descent_arrives() {
        let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.hidden.listen(dir.path().join("s.sock")).unwrap();
        install_test_run(&mut ctx, 2);
        let pack = ctx.pack.clone();
        let mut app = App::new(ctx);
        let dt = Duration::from_millis(16);
        app.step(None, dt);
        let first = app.ctx.hidden.generation();
        let before = app.ctx.hidden.board().expect("the first board");
        {
            let run = app.ctx.run.as_mut().unwrap();
            assert!(run.apply_choice(&pack, 0));
            assert!(run.apply_exit(1), "the DEEP exit");
            assert_eq!(run.depth, 2, "still on the old level");
        }
        // A caller's own request lands mid-descent: it refreshes nothing, because a snapshot
        // built now would use the next level's exit on this level's host.
        let (tx, rx) = std::sync::mpsc::channel();
        app.ctx.hidden_events = rx;
        tx.send(HiddenEvent::Learn(programs::Program::WinNuke))
            .unwrap();
        app.step(None, dt);
        assert_eq!(app.ctx.hidden.generation(), first, "no board mid-descent");
        let held = app.ctx.hidden.board().unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &held), "no refresh either");
        app.ctx.run.as_mut().unwrap().arrive(3);
        app.step(None, dt);
        assert_eq!(
            app.ctx.hidden.generation(),
            first + 1,
            "one board on arrival"
        );
        assert_eq!(app.ctx.hidden.board().unwrap().depth, 3);
    }

    #[test]
    fn the_open_door_is_published_to_the_hidden_line_and_closed_with_the_shell() {
        let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.hidden.listen(dir.path().join("s.sock")).unwrap();
        install_test_run(&mut ctx, 2);
        let screen = Screen::Run(RunScreen::new(&mut ctx));
        let mut app = App::with_screen(ctx, screen);
        let dt = Duration::from_millis(16);
        assert_eq!(app.ctx.hidden.door(), None, "nothing published yet");
        app.step(None, dt);
        let view = app.ctx.hidden.door().expect("the shell is open");
        assert_eq!((view.depth, view.attempt, view.current), (2, 0, 0));

        // The shell closes: the door is withdrawn, and a signal that was still queued (or
        // sent a moment later) is dropped rather than held for the next door.
        app.screen = Screen::Menu(MenuScreen::new(&mut app.ctx));
        let signal = DoorSignal {
            depth: 2,
            attempt: 0,
            signal: Signal::Urg,
        };
        apply_hidden_event(&mut app.ctx, HiddenEvent::Signal(signal));
        assert_eq!(app.ctx.door_signals, vec![signal]);
        app.step(None, dt);
        assert_eq!(app.ctx.hidden.door(), None);
        assert!(app.ctx.door_signals.is_empty());
    }
}
