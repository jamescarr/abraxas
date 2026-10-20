//! The Run screen: a hacking session on the level's host.
//!
//! The shell (`game::shell`) and the host graph (`game::host`) hold all the rules; this screen
//! draws the RUN wireframe (header with the trace meter, scrollback, side panel with the map,
//! objective and deck, prompt), edits the command line, and interprets every [`ShellEffect`]:
//! it launches the ICE mini-games ([`IceView`]), moves EGO, RATIO and INTEGRATION on the run,
//! pushes Beat when the objective completes and handles DUMP SHOCK and logout.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::ice_view::{DuelArt, IceView, VIEW_ROWS, ice_input};
use super::{BeatScreen, Nav, Screen, ScreenImpl};
use crate::game::danger::{self, params};
use crate::game::host::{HostSession, IceKindAt, PlayerView, PromptColor};
use crate::game::programs::{self, ProgramKind};
use crate::game::shell::{
    self, EditKey, LineEditor, LineStyle, SHELL_COLS, ShellEffect, wrap_lines,
};
use crate::game::state::{Backdoor, Level, LevelSeed, Phase};
use crate::ice::Loadout;
use crate::ice::duel::{BlackClass, DuelKind};
use crate::pack::model::{ExitKind, HostOs, ObjectiveKind};
use crate::palette::{Dos, Palette};
use crate::rng::substream;
use crate::ui::app::Ctx;
use crate::ui::canvas::{WIDTH, fill, put_centered, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::map::{MapGrid, MapScale};
use crate::ui::widgets::progress::{self, BarStyle};

/// Scrollback lines kept.
const SCROLL_CAP: usize = 400;
/// Seconds the DUMP SHOCK banner stays up.
const DUMP_SECS: f32 = 2.5;
/// Seconds a transfer takes when the modem is instant (baud 0).
const INSTANT_TRANSFER_SECS: f32 = 0.5;
/// Column of the pane separator and where the side panel starts.
const SEP_X: u16 = SHELL_COLS as u16;
const SIDE_X: u16 = SEP_X + 1;
const SIDE_W: u16 = WIDTH - SIDE_X;
/// Rows of scrollback (canvas rows 1..=22) and the prompt row.
const PANE_ROWS: u16 = 22;
const PROMPT_ROW: u16 = 23;
/// Lines PgUp/PgDn scroll.
const PAGE: usize = 10;

#[derive(Debug)]
struct Transfer {
    file: String,
    bytes: usize,
    secs: f32,
    elapsed: f32,
    sending: bool,
    then: Vec<ShellEffect>,
}

#[derive(Debug)]
enum Mode {
    /// Typing commands.
    Shell,
    /// A mini-game owns the canvas. `node` is the guarded node, `None` for the ROUTER bounce.
    Ice {
        view: Box<IceView>,
        node: Option<usize>,
    },
    /// The `scan` map replaces the scrollback until a key.
    Scan {
        grid: MapGrid,
        scroll: (usize, usize),
    },
    /// A ZMODEM transfer is running.
    Transfer(Transfer),
    /// DUMP SHOCK banner; leaves for the menu when `left` runs out or on a key.
    Dump { left: f32 },
}

/// The shell screen.
#[derive(Debug)]
pub struct RunScreen {
    session: HostSession,
    scrollback: Vec<(LineStyle, String)>,
    /// Lines scrolled up from the newest.
    view_offset: usize,
    editor: LineEditor,
    mode: Mode,
    /// Compact map for the side panel, rebuilt whenever the session changes.
    mini: MapGrid,
    /// Mini-games launched so far (part of each game's RNG stream).
    launches: u32,
    /// True once the door reward was granted.
    rewarded: bool,
    /// True once the "trace is still running" hint was shown for the recorded choice.
    choice_seen: bool,
    /// The run or its place is missing; the screen leaves on its first tick.
    orphan: bool,
    /// First word of the last command (`put` sends, everything else receives).
    last_verb: String,
    /// The hidden line's backdoor has been looked at for this host (checked at creation and on
    /// every tick until a planted one opens the relay).
    backdoor_synced: bool,
}

/// A placeholder level for a screen built without a run (never reached in play).
fn fallback_level() -> Level {
    Level::new(
        LevelSeed {
            depth: 0,
            act_id: String::new(),
            anchor: None,
            base_danger: 1,
            omen: 1,
            motifs: Vec::new(),
            room: String::new(),
            focus: None,
            objective_kind: ObjectiveKind::Retrieve,
            target_daemon: None,
            wordlock_len: 5,
        },
        Vec::new(),
    )
}

fn exit_kind(ctx: &Ctx) -> Option<ExitKind> {
    ctx.run.as_ref()?.exit_taken.as_ref().map(|e| e.kind)
}

fn player_view(ctx: &Ctx) -> PlayerView {
    let Some(run) = ctx.run.as_ref() else {
        return PlayerView::default();
    };
    PlayerView {
        handle: run.handle.clone(),
        shadow: run.shadow(),
        deck: run.deck.clone(),
        dev: ctx.dev,
        clock: clock_text(ctx),
    }
}

fn clock_text(ctx: &Ctx) -> String {
    ctx.clock()
        .map_or_else(String::new, |c| format!("{} {}", c.date, c.hms()))
}

pub(super) fn line_style(p: &Palette, style: LineStyle) -> Style {
    p.style(
        match style {
            LineStyle::Normal => Dos::LightGray,
            LineStyle::Error => Dos::LightRed,
            LineStyle::Dim => Dos::DarkGray,
            LineStyle::Highlight => Dos::Yellow,
        },
        Dos::Black,
    )
}

fn prompt_style(p: &Palette, color: PromptColor) -> Style {
    match color {
        PromptColor::LightGreen => p.style(Dos::LightGreen, Dos::Black),
        PromptColor::Yellow => p.style(Dos::Yellow, Dos::Black),
        PromptColor::WhiteOnBlue => p.style(Dos::White, Dos::Blue),
        PromptColor::LightGray => p.style(Dos::LightGray, Dos::Black),
    }
}

fn edit_key(key: Key) -> Option<EditKey> {
    Some(match key {
        Key::Char(c) => EditKey::Char(c),
        Key::Left => EditKey::Left,
        Key::Right => EditKey::Right,
        Key::Home => EditKey::Home,
        Key::End => EditKey::End,
        Key::Backspace => EditKey::Backspace,
        Key::Delete => EditKey::Delete,
        Key::Up => EditKey::Up,
        Key::Down => EditKey::Down,
        Key::Tab => EditKey::Tab,
        Key::Enter => EditKey::Enter,
        Key::Ctrl('l') => EditKey::ClearScreen,
        _ => return None,
    })
}

impl RunScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let live = ctx
            .level()
            .filter(|l| l.place.is_some() && ctx.run.is_some())
            .cloned();
        let orphan = live.is_none();
        let level = live.unwrap_or_else(fallback_level);
        let run_danger = danger::run_danger(level.seed.base_danger, exit_kind(ctx));
        let (attempt, seed, daemons) = (
            ctx.run.as_ref().map_or(0, |r| r.door_attempts),
            ctx.run.as_ref().map_or(0, |r| r.seed),
            ctx.pack.pool.daemons.clone(),
        );
        let mut session =
            HostSession::generate(&level, run_danger, exit_kind(ctx), attempt, seed, &daemons);
        session.set_player(player_view(ctx));
        let choice_seen = ctx.run.as_ref().is_some_and(|r| {
            matches!(
                r.phase,
                Phase::Hub {
                    choice: Some(_),
                    ..
                }
            )
        });
        let mut screen = Self {
            mini: MapGrid::build(&session.map_layout(), MapScale::Mini),
            session,
            scrollback: Vec::new(),
            view_offset: 0,
            editor: LineEditor::new(),
            mode: Mode::Shell,
            launches: 0,
            rewarded: false,
            choice_seen,
            orphan,
            last_verb: String::new(),
            backdoor_synced: false,
        };
        if orphan {
            return screen;
        }
        let banner = format!(
            "CONNECTED: {}  [{}]",
            screen.session.host_name(),
            screen.session.os().label()
        );
        screen.push_wrapped(LineStyle::Highlight, &banner);
        let text = screen.session.banner().to_string();
        screen.push_wrapped(LineStyle::Normal, &text);
        screen.push_line(LineStyle::Normal, String::new());
        screen.sync_backdoor(ctx);
        if screen.session.needs_login() {
            screen.push_line(LineStyle::Highlight, "ENTER PASSWORD NOW".to_string());
            screen.launch_ice(0, IceKindAt::Wordlock, ctx);
        }
        screen
    }

    // ----- scrollback ----------------------------------------------------------------------

    fn push_line(&mut self, style: LineStyle, text: String) {
        self.scrollback.push((style, text));
        if self.scrollback.len() > SCROLL_CAP {
            self.scrollback.remove(0);
        }
    }

    fn push_wrapped(&mut self, style: LineStyle, text: &str) {
        for line in wrap_lines(text, SHELL_COLS) {
            self.push_line(style, line);
        }
    }

    fn refresh_map(&mut self) {
        self.mini = MapGrid::build(&self.session.map_layout(), MapScale::Mini);
    }

    /// Once the hidden line has planted this board's backdoor, the PHREAK relay is open before
    /// it is asked. Checked at creation and on every tick, so planting from the second terminal
    /// while this shell is already open takes effect at once.
    fn sync_backdoor(&mut self, ctx: &Ctx) {
        if self.backdoor_synced {
            return;
        }
        let planted = ctx
            .run
            .as_ref()
            .is_some_and(|r| r.backdoors.get(&r.depth) == Some(&Backdoor::Planted));
        if !planted {
            return;
        }
        self.backdoor_synced = true;
        if let Some((id, display)) = self.session.open_backdoor() {
            self.refresh_map();
            self.push_wrapped(
                LineStyle::Highlight,
                &format!("BACKDOOR: {id} [{display}] answers before it asks. the relay is yours."),
            );
        }
    }

    /// The guarded node of the running ICE view (tests).
    #[cfg(test)]
    pub(crate) fn ice_node(&self) -> Option<usize> {
        match &self.mode {
            Mode::Ice { node, .. } => *node,
            _ => None,
        }
    }

    /// The DAEMON console's step solutions, when a DAEMON view is running (tests).
    #[cfg(test)]
    pub(crate) fn daemon_solution(&self) -> Option<Vec<String>> {
        match &self.mode {
            Mode::Ice { view, .. } => view.daemon_game().map(|g| g.solution()),
            _ => None,
        }
    }

    // ----- mini-games ----------------------------------------------------------------------

    fn launch_ice(&mut self, node: usize, kind: IceKindAt, ctx: &mut Ctx) {
        let Some(run) = ctx.run.as_ref() else {
            return;
        };
        self.launches += 1;
        let id = self.session.nodes()[node].id.clone();
        let mut rng = substream(run.seed, run.depth, &format!("ice-{id}"), self.launches);
        let loadout = Loadout::from_deck(&run.deck);
        let base = ctx.level().map_or(1, |l| l.seed.base_danger);
        let (grade, class) = {
            let n = &self.session.nodes()[node];
            (n.grade, n.class)
        };
        let run_params = params(self.session.run_danger()).graded(grade);
        let view = match kind {
            IceKindAt::Wordlock => {
                let (words, password) = ctx
                    .place()
                    .map(|p| (p.words.clone(), p.password.clone()))
                    .unwrap_or_default();
                IceView::wordlock(&mut rng, &params(base), &words, &password, loadout, grade)
            }
            IceKindAt::Breach => IceView::breach(&mut rng, &run_params, loadout),
            IceKindAt::Phreak => IceView::phreak(&mut rng, &run_params, loadout),
            IceKindAt::Daemon => IceView::daemon(&mut rng, self.session.os(), grade),
            IceKindAt::Black | IceKindAt::Shadow => {
                let duel = if kind == IceKindAt::Black {
                    DuelKind::Black
                } else {
                    DuelKind::Shadow
                };
                let art = ctx
                    .pack
                    .shadow_character()
                    .and_then(|c| c.art.as_deref())
                    .and_then(|name| ctx.pack.art(name));
                IceView::duel(
                    &mut rng,
                    &run_params,
                    duel,
                    loadout,
                    run.stats.ego,
                    DuelArt {
                        palette: &ctx.palette,
                        shadow_art: art,
                    },
                    class.unwrap_or(BlackClass::Wolf),
                )
            }
        };
        self.mode = Mode::Ice {
            view: Box::new(view),
            node: Some(node),
        };
        if let Mode::Ice { view, .. } = &mut self.mode {
            view.set_calm(ctx.calm());
        }
    }

    fn launch_router(&mut self, ctx: &mut Ctx) {
        let Some(run) = ctx.run.as_ref() else {
            return;
        };
        self.launches += 1;
        let mut rng = substream(run.seed, run.depth, "ice-bounce", self.launches);
        let view = IceView::router(&mut rng, &params(self.session.run_danger()));
        self.mode = Mode::Ice {
            view: Box::new(view),
            node: None,
        };
        if let Mode::Ice { view, .. } = &mut self.mode {
            view.set_calm(ctx.calm());
        }
    }

    /// Drain the running game's side effects and, once it has an outcome, apply it to the
    /// session and the run.
    fn pump_ice(&mut self, ctx: &mut Ctx) -> Nav {
        let Mode::Ice { view, node } = &mut self.mode else {
            return Nav::Stay;
        };
        let mut effects = Vec::new();
        let trace = view.take_trace();
        if trace > 0.0 {
            self.session.bump(trace, true, &mut effects);
        }
        let ego = view.take_ego();
        if ego != 0
            && let Some(run) = ctx.run.as_mut()
        {
            run.stats.add_ego(ego);
        }
        if let Some(run) = ctx.run.as_mut() {
            for (program, n) in view.take_spent() {
                for _ in 0..n {
                    programs::spend(&mut run.deck, program);
                    programs::spend(&mut self.session.player.deck, program);
                }
            }
        }
        let Some(outcome) = view.outcome() else {
            return self.apply_effects(effects, ctx);
        };
        let node = *node;
        let (kb, wipe) = view.rewards();
        self.mode = Mode::Shell;

        if let Some(run) = ctx.run.as_mut()
            && kb > 0
        {
            run.stats.ratio_kb = run.stats.ratio_kb.saturating_add(kb);
            effects.push(ShellEffect::line(format!("+{kb}K credited.")));
        }
        if wipe != 0.0 {
            self.session.bump(wipe, true, &mut effects);
        }
        effects.extend(match node {
            Some(n) => self.session.apply_ice_outcome(n, outcome),
            None => self.session.apply_bounce_outcome(outcome),
        });
        self.apply_effects(effects, ctx)
    }

    // ----- effects -------------------------------------------------------------------------

    fn apply_effects(&mut self, effects: Vec<ShellEffect>, ctx: &mut Ctx) -> Nav {
        let mut nav = Nav::Stay;
        for effect in effects {
            match effect {
                ShellEffect::Print { style, text } => self.push_wrapped(style, &text),
                ShellEffect::Clear => {
                    self.scrollback.clear();
                    self.view_offset = 0;
                }
                ShellEffect::StartIce { node, kind } => self.launch_ice(node, kind, ctx),
                ShellEffect::Scan => {
                    let grid = MapGrid::build(&self.session.map_layout(), MapScale::Full);
                    let scroll = grid.scroll_for(
                        self.session.current(),
                        usize::from(SEP_X),
                        usize::from(PANE_ROWS - 2),
                    );
                    self.mode = Mode::Scan { grid, scroll };
                }
                ShellEffect::TraceDelta(_) | ShellEffect::Moved { .. } => {}
                ShellEffect::RatioReward(kb) => {
                    if let Some(run) = ctx.run.as_mut() {
                        run.stats.ratio_kb = run.stats.ratio_kb.saturating_add(kb);
                    }
                }
                ShellEffect::EgoDelta(d) => {
                    if let Some(run) = ctx.run.as_mut() {
                        run.stats.add_ego(d);
                    }
                }
                ShellEffect::IntegrationDelta(d) => {
                    if let Some(run) = ctx.run.as_mut() {
                        run.stats.add_integration(d);
                    }
                }
                ShellEffect::ObjectiveComplete => {
                    let pushed = self.complete_objective(ctx);
                    if matches!(nav, Nav::Stay) {
                        nav = pushed;
                    }
                }
                ShellEffect::DumpShock => self.dump_shock(ctx),
                ShellEffect::Logout { .. } => {
                    ctx.autosave();
                    nav = Nav::Back;
                }
                ShellEffect::StartTransfer {
                    file,
                    bytes,
                    secs,
                    then,
                } => {
                    let secs = if ctx.baud() == 0 {
                        secs.min(INSTANT_TRANSFER_SECS)
                    } else {
                        secs
                    };
                    self.mode = Mode::Transfer(Transfer {
                        file,
                        bytes,
                        secs,
                        elapsed: 0.0,
                        sending: self.last_verb == "put",
                        then,
                    });
                }
                ShellEffect::Bounce => self.launch_router(ctx),
                ShellEffect::UseProgram(program) => {
                    if let Some(run) = ctx.run.as_mut() {
                        programs::spend(&mut run.deck, program);
                    }
                }
            }
        }
        self.refresh_map();
        nav
    }

    /// Grant the door reward once, mark the door done and hand over to Beat.
    fn complete_objective(&mut self, ctx: &mut Ctx) -> Nav {
        if self.rewarded {
            return Nav::Stay;
        }
        self.rewarded = true;
        let kb = danger::door_reward(self.session.run_danger(), self.session.exit_kind());
        if let Some(run) = ctx.run.as_mut() {
            run.stats.ratio_kb = run.stats.ratio_kb.saturating_add(kb);
            let choice = match run.phase {
                Phase::Hub { choice, .. } => choice,
                _ => None,
            };
            run.phase = Phase::Hub {
                door_done: true,
                choice,
            };
        }
        self.push_line(
            LineStyle::Highlight,
            format!("OBJECTIVE COMPLETE. +{kb}K credited."),
        );
        ctx.autosave();
        let beat = BeatScreen::new(ctx);
        Nav::Push(Box::new(Screen::Beat(beat)))
    }

    /// EGO -20 already arrived as its own effect; a failed door counts as an attempt.
    fn dump_shock(&mut self, ctx: &mut Ctx) {
        if !self.session.objective_done()
            && let Some(run) = ctx.run.as_mut()
        {
            run.door_attempts += 1;
        }
        self.push_line(LineStyle::Error, "DUMP SHOCK".to_string());
        ctx.autosave();
        self.mode = Mode::Dump { left: DUMP_SECS };
    }

    // ----- input ---------------------------------------------------------------------------

    fn submit(&mut self, line: &str, ctx: &mut Ctx) -> Nav {
        let (prompt, _) = self.session.prompt();
        self.push_line(LineStyle::Normal, format!("{prompt}{line}"));
        self.view_offset = 0;
        if line.trim().is_empty() {
            return Nav::Stay;
        }
        self.run_command(line, ctx)
    }

    fn run_command(&mut self, line: &str, ctx: &mut Ctx) -> Nav {
        self.last_verb = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        self.session.player.clock = clock_text(ctx);
        let effects = shell::execute(&mut self.session, line);
        self.apply_effects(effects, ctx)
    }

    fn shell_key(&mut self, key: Key, ctx: &mut Ctx) -> Nav {
        match key {
            Key::Esc => {
                if self.editor.text().is_empty() {
                    return self.run_command("logout", ctx);
                }
                self.editor.clear();
                Nav::Stay
            }
            Key::PgUp => {
                self.view_offset = (self.view_offset + PAGE).min(self.scrollback.len());
                Nav::Stay
            }
            Key::PgDn => {
                self.view_offset = self.view_offset.saturating_sub(PAGE);
                Nav::Stay
            }
            other => {
                let Some(edit) = edit_key(other) else {
                    return Nav::Stay;
                };
                match self.editor.handle(edit, &self.session) {
                    Some(_) if edit == EditKey::ClearScreen => self.run_command("clear", ctx),
                    Some(line) => self.submit(&line, ctx),
                    None => Nav::Stay,
                }
            }
        }
    }
}

impl ScreenImpl for RunScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        let key = classify(&key);
        match &mut self.mode {
            Mode::Shell => self.shell_key(key, ctx),
            Mode::Ice { view, .. } => {
                if let Some(input) = ice_input(key) {
                    view.handle(input);
                }
                self.pump_ice(ctx)
            }
            Mode::Scan { grid, scroll } => {
                let (w, h) = (usize::from(SEP_X), usize::from(PANE_ROWS - 2));
                match key {
                    Key::Left => scroll.0 = scroll.0.saturating_sub(4),
                    Key::Right => *scroll = grid.clamp_scroll((scroll.0 + 4, scroll.1), w, h),
                    Key::Up => scroll.1 = scroll.1.saturating_sub(1),
                    Key::Down => *scroll = grid.clamp_scroll((scroll.0, scroll.1 + 1), w, h),
                    _ => self.mode = Mode::Shell,
                }
                Nav::Stay
            }
            Mode::Transfer(_) => Nav::Stay,
            Mode::Dump { .. } => Nav::Back,
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if self.orphan {
            return Nav::Back;
        }
        self.sync_backdoor(ctx);
        if !self.choice_seen
            && ctx.run.as_ref().is_some_and(|r| {
                matches!(
                    r.phase,
                    Phase::Hub {
                        choice: Some(_),
                        ..
                    }
                )
            })
        {
            self.choice_seen = true;
            self.push_line(
                LineStyle::Highlight,
                "The trace is still running. Jack out.".to_string(),
            );
        }
        let mut nav = Nav::Stay;
        let mut in_minigame = false;
        match &mut self.mode {
            Mode::Dump { left } => {
                *left -= dt.as_secs_f32();
                return if *left <= 0.0 { Nav::Back } else { Nav::Stay };
            }
            Mode::Ice { view, .. } => {
                in_minigame = true;
                view.tick(dt);
                nav = self.pump_ice(ctx);
            }
            Mode::Transfer(t) => {
                t.elapsed += dt.as_secs_f32();
                if t.elapsed >= t.secs {
                    let then = std::mem::take(&mut t.then);
                    let note = format!("{}: {} bytes transferred.", t.file, t.bytes);
                    self.mode = Mode::Shell;
                    self.push_line(LineStyle::Dim, note);
                    nav = self.apply_effects(then, ctx);
                }
            }
            Mode::Shell | Mode::Scan { .. } => {}
        }
        if matches!(nav, Nav::Stay) {
            let effects = self.session.tick(dt, in_minigame);
            if !effects.is_empty() {
                nav = self.apply_effects(effects, ctx);
            }
        }
        nav
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        fill(buf, area, p.style(Dos::LightGray, Dos::Black));
        self.render_header(buf, area, ctx);
        match &self.mode {
            Mode::Ice { view, .. } => {
                let view_area = Rect::new(area.x, area.y + 1, WIDTH, VIEW_ROWS);
                view.render(buf, view_area, p);
                return;
            }
            Mode::Scan { grid, scroll } => self.render_scan(buf, area, ctx, grid, *scroll),
            _ => self.render_scrollback(buf, area, ctx),
        }
        self.render_side(buf, area, ctx);
        self.render_prompt(buf, area, ctx);
        match &self.mode {
            Mode::Transfer(t) => render_transfer(buf, area, p, t),
            Mode::Dump { .. } => render_dump(buf, area, p),
            _ => {}
        }
    }

    fn wants_status(&self) -> bool {
        true
    }

    fn in_text_input(&self) -> bool {
        match &self.mode {
            Mode::Shell => true,
            Mode::Ice { view, .. } => view.wants_text(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

impl RunScreen {
    fn render_header(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let bar_bg = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, WIDTH, 1), bar_bg);
        let left = format!(
            " {} │ {} │ node: {}",
            self.session.host_name(),
            self.session.os().label(),
            self.session.current_node().id
        );
        put_str(buf, area.x, area.y, &left, 54, bar_bg);
        let trace = self.session.trace();
        let color = if trace >= 80.0 {
            Dos::LightRed
        } else if trace >= 50.0 {
            Dos::Yellow
        } else {
            Dos::LightGreen
        };
        let x = area.x + WIDTH - 24;
        put_str(buf, x, area.y, "TRACE", 5, bar_bg);
        progress::render(
            buf,
            x + 6,
            area.y,
            12,
            trace / 100.0,
            BarStyle {
                filled: p.style(color, Dos::Blue),
                empty: p.style(Dos::Cyan, Dos::Blue),
            },
            true,
        );
    }

    fn render_scrollback(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let mainframe = self.session.os() == HostOs::Mainframe;
        let rows = usize::from(PANE_ROWS) - usize::from(mainframe);
        let end = self.scrollback.len().saturating_sub(self.view_offset);
        let start = end.saturating_sub(rows);
        for (i, (style, text)) in self.scrollback[start..end].iter().enumerate() {
            put_str(
                buf,
                area.x,
                area.y + 1 + i as u16,
                text,
                SHELL_COLS as u16,
                line_style(p, *style),
            );
        }
        if mainframe && let Some(header) = self.session.prompt_header() {
            let style = prompt_style(p, PromptColor::WhiteOnBlue);
            fill(buf, Rect::new(area.x, area.y + PANE_ROWS, SEP_X, 1), style);
            put_str(buf, area.x, area.y + PANE_ROWS, header, SEP_X, style);
        }
        if self.view_offset > 0 {
            put_str(
                buf,
                area.x + SEP_X - 12,
                area.y + 1,
                "-- scrolled --",
                12,
                p.style(Dos::Black, Dos::LightGray),
            );
        }
    }

    fn render_scan(
        &self,
        buf: &mut Buffer,
        area: Rect,
        ctx: &Ctx,
        grid: &MapGrid,
        scroll: (usize, usize),
    ) {
        let p = &ctx.palette;
        put_str(
            buf,
            area.x,
            area.y + 1,
            &format!("NETWORK SCAN  {}", self.session.host_name()),
            SEP_X,
            p.style(Dos::White, Dos::Black),
        );
        grid.render(
            buf,
            (area.x, area.y + 2),
            (SEP_X, PANE_ROWS - 3),
            scroll,
            p,
            ctx.chrome(),
        );
        let y = area.y + PANE_ROWS - 1;
        let mut x = area.x;
        x += put_str(buf, x, y, "current", 7, p.style(Dos::Black, Dos::LightGray)) + 1;
        x += put_str(
            buf,
            x,
            y,
            "cleared",
            7,
            Style::new().fg(ctx.chrome()).bg(p.color(Dos::Black)),
        ) + 1;
        x += put_str(buf, x, y, "ICE", 3, p.style(Dos::LightRed, Dos::Black)) + 2;
        put_str(
            buf,
            x,
            y,
            "arrows scroll; any key closes",
            area.x + SEP_X - x,
            p.style(Dos::DarkGray, Dos::Black),
        );
    }

    fn render_side(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let x = area.x + SEP_X;
        let sep = p.style(Dos::DarkGray, Dos::Black);
        for y in 1..=PANE_ROWS {
            put_str(buf, x, area.y + y, "│", 1, sep);
        }
        let sx = area.x + SIDE_X;
        let chrome = Style::new().fg(ctx.chrome()).bg(p.color(Dos::Black));
        let title = |buf: &mut Buffer, y: u16, text: &str| {
            let rule = format!("─ {text} ");
            let used = put_str(buf, sx, area.y + y, &rule, SIDE_W, chrome);
            put_str(
                buf,
                sx + used,
                area.y + y,
                &"─".repeat(usize::from(SIDE_W - used)),
                SIDE_W - used,
                sep,
            );
        };
        let text = p.style(Dos::LightGray, Dos::Black);

        title(buf, 1, "MAP");
        let view = (SIDE_W - 1, 7);
        let scroll = self.mini.scroll_for(
            self.session.current(),
            usize::from(view.0),
            usize::from(view.1),
        );
        self.mini
            .render(buf, (sx + 1, area.y + 2), view, scroll, p, ctx.chrome());

        title(buf, 9, "OBJECTIVE");
        let objective = wrap_lines(self.session.objective_text(), usize::from(SIDE_W - 1));
        for (i, line) in objective.iter().take(4).enumerate() {
            put_str(buf, sx + 1, area.y + 10 + i as u16, line, SIDE_W - 1, text);
        }
        let task = match self.session.objective_kind() {
            ObjectiveKind::Retrieve => format!("get {}", self.session.target_file()),
            ObjectiveKind::Plant => format!("put {}", self.session.target_file()),
            ObjectiveKind::Crash => format!("kill {}", self.session.target_daemon()),
        };
        put_str(
            buf,
            sx + 1,
            area.y + 14,
            &task,
            SIDE_W - 1,
            p.style(Dos::White, Dos::Black),
        );
        let (state, color) = if self.session.objective_done() {
            ("[X] DONE", Dos::LightGreen)
        } else {
            ("[ ] PENDING", Dos::Yellow)
        };
        put_str(
            buf,
            sx + 1,
            area.y + 15,
            state,
            SIDE_W - 1,
            p.style(color, Dos::Black),
        );

        title(buf, 17, "DECK");
        let mut deck = vec!["PAYLOAD.PIF".to_string()];
        if let Some(run) = ctx.run.as_ref() {
            for (program, &n) in &run.deck {
                if n == 0 {
                    continue;
                }
                deck.push(match program.kind() {
                    ProgramKind::Persistent => program.file_name().to_string(),
                    ProgramKind::Charges(_) => format!("{} x{n}", program.file_name()),
                });
            }
        }
        if self.session.ghost_active() {
            deck.push("(GHOST running)".to_string());
        }
        let rows = 4;
        let hidden = deck.len().saturating_sub(rows);
        if hidden > 0 {
            deck.truncate(rows - 1);
            deck.push(format!("+{} more", hidden + 1));
        }
        for (i, line) in deck.iter().enumerate() {
            put_str(buf, sx + 1, area.y + 18 + i as u16, line, SIDE_W - 1, text);
        }
    }

    fn render_prompt(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let (prompt, color) = self.session.prompt();
        let style = prompt_style(p, color);
        let y = area.y + PROMPT_ROW;
        let mainframe = color == PromptColor::WhiteOnBlue;
        let input_style = if mainframe {
            style
        } else {
            p.style(Dos::White, Dos::Black)
        };
        if mainframe {
            fill(buf, Rect::new(area.x, y, WIDTH, 1), style);
        }
        let used = put_str(buf, area.x, y, &prompt, WIDTH, style);
        match &self.mode {
            Mode::Shell => {
                let chars: Vec<char> = self.editor.text().chars().collect();
                let room = usize::from(WIDTH - used).saturating_sub(1).max(1);
                let cursor = self.editor.cursor();
                let first = (cursor + 1).saturating_sub(room);
                let shown: String = chars.iter().skip(first).take(room).collect();
                put_str(buf, area.x + used, y, &shown, WIDTH - used, input_style);
                let cx = area.x + used + (cursor - first) as u16;
                let (fg, bg) = if mainframe {
                    (Dos::Blue, Dos::White)
                } else {
                    (Dos::Black, Dos::White)
                };
                let ch = chars.get(cursor).copied().unwrap_or(' ');
                let mut cell = [0u8; 4];
                put_str(buf, cx, y, ch.encode_utf8(&mut cell), 1, p.style(fg, bg));
            }
            Mode::Scan { .. } => {
                put_str(
                    buf,
                    area.x + used,
                    y,
                    "-- scan --",
                    10,
                    p.style(Dos::DarkGray, Dos::Black),
                );
            }
            _ => {}
        }
    }
}

fn render_transfer(buf: &mut Buffer, area: Rect, p: &Palette, t: &Transfer) {
    let top = area.y + PANE_ROWS - 4;
    fill(
        buf,
        Rect::new(area.x, top, SEP_X, 4),
        p.style(Dos::LightGray, Dos::Black),
    );
    let frac = if t.secs <= 0.0 {
        1.0
    } else {
        (t.elapsed / t.secs).clamp(0.0, 1.0)
    };
    let sent = (t.bytes as f32 * frac) as usize;
    let cps = (t.bytes as f32 / t.secs.max(0.01)) as usize;
    let left = (t.secs - t.elapsed).max(0.0).ceil() as u32;
    let verb = if t.sending { "Sending" } else { "Receiving" };
    put_str(
        buf,
        area.x,
        top,
        &format!("ZMODEM  {verb} {}", t.file),
        SEP_X,
        p.style(Dos::White, Dos::Blue),
    );
    put_str(
        buf,
        area.x,
        top + 1,
        &format!(
            "Bytes {sent}/{}  CPS {cps}  ETA {}:{:02}",
            t.bytes,
            left / 60,
            left % 60
        ),
        SEP_X,
        p.style(Dos::LightGray, Dos::Black),
    );
    progress::render(
        buf,
        area.x,
        top + 2,
        40,
        frac,
        BarStyle {
            filled: p.style(Dos::LightGreen, Dos::Black),
            empty: p.style(Dos::DarkGray, Dos::Black),
        },
        true,
    );
}

fn render_dump(buf: &mut Buffer, area: Rect, p: &Palette) {
    let red = p.style(Dos::White, Dos::Red);
    let panel = Rect::new(area.x + 2, area.y + 8, 50, 7);
    fill(buf, panel, red);
    let rows = [
        "*** DUMP SHOCK ***",
        "",
        "The trace found you. The line drops.",
        "EGO -20",
        "",
        "press a key",
    ];
    for (i, text) in rows.iter().enumerate() {
        let y = panel.y + 1 + i as u16;
        put_centered(buf, panel, y, text, red);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::host::{NodeKind, ObjectiveState};
    use crate::game::programs::Program;
    use crate::ice::IceOutcome;
    use crate::pack::model::ExitDef;
    use crate::ui::app::{App, install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char, press_ctrl};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event, KeyCode};

    const FRAME: Duration = Duration::from_millis(33);

    struct Rig {
        _dir: tempfile::TempDir,
        app: App,
    }

    fn rig_with(depth: u32, setup: impl FnOnce(&mut Ctx)) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        setup(&mut ctx);
        let screen = Screen::Run(RunScreen::new(&mut ctx));
        Rig {
            _dir: dir,
            app: App::with_screen(ctx, screen),
        }
    }

    fn rig(depth: u32) -> Rig {
        rig_with(depth, |_| {})
    }

    impl Rig {
        fn run(&mut self) -> &mut RunScreen {
            match &mut self.app.screen {
                Screen::Run(s) => s,
                other => panic!("expected the Run screen, found {}", other.name()),
            }
        }

        fn press(&mut self, ev: ratatui::crossterm::event::KeyEvent) {
            self.app.step(Some(Event::Key(ev)), FRAME);
        }

        fn type_line(&mut self, text: &str) {
            for c in text.chars() {
                self.press(press_char(c));
            }
            self.press(press(KeyCode::Enter));
        }

        fn wait(&mut self, secs: f32) {
            let mut left = secs;
            while left > 0.0 {
                let dt = left.min(0.1);
                self.app.step(None, Duration::from_secs_f32(dt));
                left -= dt;
            }
        }

        fn text(&mut self) -> String {
            let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
            term.draw(|f| self.app.render(f)).unwrap();
            let buf = term.backend().buffer();
            (0..25)
                .map(|y| row_text(buf, buf.area, y))
                .collect::<Vec<_>>()
                .join("\n")
        }

        fn row(&mut self, y: usize) -> String {
            self.text().lines().nth(y).unwrap_or_default().to_string()
        }

        fn ctx(&mut self) -> &mut Ctx {
            &mut self.app.ctx
        }

        /// Pass the portal without playing WORDLOCK.
        fn skip_login(&mut self) {
            let s = self.run();
            s.session.apply_ice_outcome(0, IceOutcome::Cleared);
            s.mode = Mode::Shell;
            s.refresh_map();
        }

        fn in_shell(&mut self) -> bool {
            matches!(self.run().mode, Mode::Shell)
        }
    }

    fn ego(r: &mut Rig) -> i32 {
        r.ctx().run.as_ref().unwrap().stats.ego
    }

    fn ratio(r: &mut Rig) -> u32 {
        r.ctx().run.as_ref().unwrap().stats.ratio_kb
    }

    #[test]
    fn entering_a_host_shows_the_banner_and_starts_the_portal_wordlock() {
        let mut r = rig(0);
        let t = r.text();
        assert!(t.contains("ENTER PASSWORD NOW"), "{t}");
        assert!(t.contains("WORDLOCK"));
        assert!(t.contains("ATTEMPT(S) LEFT: ■ ■ ■ ■"));
        // Header: host, OS and node, then the trace meter.
        let head = r.row(0);
        let host = r.run().session.host_name().to_string();
        assert!(
            head.contains(&host) && head.contains("node: portal0"),
            "{head:?}"
        );
        assert!(head.contains("TRACE") && head.contains("0%"), "{head:?}");
        assert!(!r.run().in_text_input(), "the mini-game owns the keys");
    }

    #[test]
    fn esc_in_the_portal_wordlock_costs_trace_and_leaves_the_locked_shell() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 0);
        let mut screen = RunScreen::new(&mut ctx);
        let nav = screen.handle_key(press(KeyCode::Esc), &mut ctx);
        assert!(matches!(nav, Nav::Stay), "the shell stays up");
        assert!(screen.session.trace() >= 5.0, "abort costs 5 trace");
        assert!(screen.session.needs_login(), "the portal was not passed");
        assert!(screen.in_text_input(), "the prompt is available again");
    }

    #[test]
    fn the_locked_portal_refuses_to_connect_and_reopens_the_password_prompt() {
        let mut r = rig(0);
        r.press(press(KeyCode::Esc));
        let neighbour = {
            let session = &r.run().session;
            session.nodes()[session.nodes()[0].neighbors[0]].id.clone()
        };
        for c in format!("connect {neighbour}").chars() {
            r.press(press_char(c));
        }
        r.press(press(KeyCode::Enter));
        assert!(r.text().contains("WORDLOCK"), "the lock is offered again");
        assert_eq!(r.run().session.current_node().id, "portal0");
    }

    /// Select the password through key events (cursor moves are linear over 2 x 12 columns).
    fn select_password(r: &mut Rig) {
        let (line, start) = {
            let Mode::Ice { view, .. } = &r.run().mode else {
                panic!("no mini-game")
            };
            let g = view.wordlock_game().unwrap();
            let slot = g.words.iter().find(|w| w.is_password).unwrap();
            (slot.line, slot.start)
        };
        for _ in 0..(line / 16) * 12 + start {
            r.press(press(KeyCode::Right));
        }
        for _ in 0..line % 16 {
            r.press(press(KeyCode::Down));
        }
        r.press(press(KeyCode::Enter));
    }

    #[test]
    fn solving_the_portal_wordlock_logs_in_and_enters_the_shell() {
        let mut r = rig(0);
        select_password(&mut r);
        assert!(r.in_shell());
        assert!(!r.run().session.needs_login());
        assert_eq!(r.run().session.current_node().id, "portal0");
        assert!(r.run().in_text_input());
        let t = r.text();
        assert!(t.contains("access granted: portal0"), "{t}");
        assert!(t.contains(&r.run().session.prompt().0), "{t}");
    }

    #[test]
    fn a_wrong_word_costs_an_attempt_and_the_game_goes_on() {
        let mut r = rig(0);
        let (line, start) = {
            let Mode::Ice { view, .. } = &r.run().mode else {
                panic!()
            };
            let g = view.wordlock_game().unwrap();
            let slot = g.words.iter().find(|w| !w.is_password).unwrap();
            (slot.line, slot.start)
        };
        for _ in 0..(line / 16) * 12 + start {
            r.press(press(KeyCode::Right));
        }
        for _ in 0..line % 16 {
            r.press(press(KeyCode::Down));
        }
        r.press(press(KeyCode::Enter));
        let t = r.text();
        assert!(t.contains(">Entry denied."), "{t}");
        assert!(t.contains("ATTEMPT(S) LEFT: ■ ■ ■  "));
        assert!(matches!(r.run().mode, Mode::Ice { .. }));
    }

    #[test]
    fn shell_commands_print_into_the_scrollback() {
        let mut r = rig(0);
        r.skip_login();
        let prompt = r.run().session.prompt().0;
        r.type_line("help");
        r.type_line("ls");
        r.type_line("frobnicate");
        let t = r.text();
        assert!(t.contains(&format!("{prompt}help")), "{t}");
        assert!(t.contains("scan               draw the node map"), "{t}");
        assert!(t.contains(&format!("{prompt}ls")));
        assert!(
            t.contains("frobnicate"),
            "unknown commands are answered: {t}"
        );
    }

    #[test]
    fn scan_shows_the_layered_map_until_a_key() {
        let mut r = rig(9);
        r.skip_login();
        r.type_line("scan");
        let t = r.text();
        assert!(t.contains("NETWORK SCAN"), "{t}");
        assert!(t.contains("[portal0]"));
        for node in r.run().session.nodes().to_vec() {
            assert!(
                t.contains(&format!("[{}]", node.id)),
                "{} missing\n{t}",
                node.id
            );
        }
        assert!(!r.run().in_text_input());
        r.press(press(KeyCode::Down));
        assert!(matches!(r.run().mode, Mode::Scan { .. }), "arrows scroll");
        r.press(press_char('q'));
        assert!(r.in_shell());
        assert!(!r.text().contains("NETWORK SCAN"));
    }

    #[test]
    fn line_editing_history_completion_and_ctrl_l() {
        let mut r = rig(0);
        r.skip_login();
        r.type_line("deck");
        r.press(press(KeyCode::Up));
        assert_eq!(r.run().editor.text(), "deck");
        r.press(press(KeyCode::Down));
        assert_eq!(r.run().editor.text(), "");
        r.press(press_char('h'));
        r.press(press_char('e'));
        r.press(press(KeyCode::Tab));
        assert_eq!(r.run().editor.text(), "help");
        r.press(press(KeyCode::Esc));
        assert_eq!(r.run().editor.text(), "", "Esc clears a typed line first");
        assert!(r.in_shell(), "and does not leave the host");
        assert!(
            r.text().contains("PAYLOAD.PIF"),
            "deck output is in the scrollback"
        );
        r.press(press_ctrl('l'));
        let t = r.text();
        assert!(!t.contains("deck"), "Ctrl+L clears the scrollback: {t}");
    }

    #[test]
    fn typing_a_question_mark_in_the_shell_is_text_not_help() {
        let mut r = rig(0);
        r.skip_login();
        r.press(press_char('?'));
        assert_eq!(r.run().editor.text(), "?");
        assert!(r.app.overlay.is_none());
    }

    #[test]
    fn prompts_follow_the_host_os() {
        let cases = [
            (HostOs::Unix, "portal0% ", Dos::LightGreen),
            (HostOs::Vax, "$ ", Dos::Yellow),
            (HostOs::Dos, "C:\\PORTAL0>", Dos::LightGray),
        ];
        for (os, prompt, color) in cases {
            let mut r = rig(0);
            r.skip_login();
            r.run().session.os = os;
            let row = r.row(23);
            assert!(row.starts_with(prompt), "{os:?}: {row:?}");
            let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
            term.draw(|f| r.app.render(f)).unwrap();
            let p = r.app.ctx.palette;
            assert_eq!(
                term.backend().buffer()[(0, 23)].fg,
                p.color(color),
                "{os:?}"
            );
        }
        let mut r = rig(0);
        r.skip_login();
        r.run().session.os = HostOs::Mainframe;
        assert!(r.row(22).starts_with("READY"), "{:?}", r.row(22));
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| r.app.render(f)).unwrap();
        let p = r.app.ctx.palette;
        let cell = &term.backend().buffer()[(30, 23)];
        assert_eq!(
            (cell.fg, cell.bg),
            (p.color(Dos::White), p.color(Dos::Blue))
        );
    }

    /// Teleport next to a guarded node that matches `want` and `connect` to it.
    fn connect_to_guard(r: &mut Rig, want: IceKindAt) -> bool {
        let found = {
            let s = &r.run().session;
            s.nodes().iter().enumerate().find_map(|(i, n)| {
                (n.ice == Some(want) && n.guarded() && i != 0)
                    .then(|| (i, n.id.clone(), n.neighbors[0]))
            })
        };
        let Some((_, id, near)) = found else {
            return false;
        };
        r.run().session.current = near;
        r.type_line(&format!("connect {id}"));
        true
    }

    #[test]
    fn connecting_into_guarded_nodes_opens_the_matching_mini_game() {
        let expect = [
            (IceKindAt::Breach, "BREACH PROTOCOL"),
            (IceKindAt::Phreak, "PHREAK  RELAY LINE"),
            (IceKindAt::Daemon, "CONSOLE"),
            (IceKindAt::Black, "DUEL  BLACK ICE"),
            (IceKindAt::Shadow, "DUEL  YOUR SHADOW"),
        ];
        let mut seen = Vec::new();
        for seed_depth in [9u32, 10, 11, 13, 14, 2, 6] {
            for exit_kind in [None, Some(ExitKind::Mirror)] {
                for (want, label) in expect {
                    let mut r = rig_with(seed_depth, |ctx| {
                        if let Some(kind) = exit_kind {
                            ctx.run.as_mut().unwrap().exit_taken = Some(ExitDef {
                                kind,
                                text: "a mirror".into(),
                            });
                        }
                    });
                    r.skip_login();
                    if !connect_to_guard(&mut r, want) {
                        continue;
                    }
                    let t = r.text();
                    assert!(t.contains(label), "{want:?} should show {label}\n{t}");
                    assert!(t.contains("TRACE"), "the trace header stays visible");
                    assert!(matches!(r.run().mode, Mode::Ice { node: Some(_), .. }));
                    seen.push(want);
                }
            }
        }
        for (want, _) in expect {
            assert!(seen.contains(&want), "never met {want:?}; saw {seen:?}");
        }
    }

    #[test]
    fn a_planted_backdoor_leaves_the_relay_open_on_entry() {
        let mut r = rig_with(4, |ctx| {
            let run = ctx.run.as_mut().unwrap();
            run.backdoors.insert(run.depth, Backdoor::Planted);
        });
        r.skip_login();
        let (id, near) = {
            let relay = r.run().session.phreak_node().expect("depth 4 has a relay");
            assert!(!relay.guarded(), "the backdoor opens the relay");
            (relay.id.clone(), relay.neighbors[0])
        };
        assert!(r.text().contains("BACKDOOR:"), "{}", r.text());
        r.run().session.current = near;
        r.type_line(&format!("connect {id}"));
        assert!(r.in_shell(), "no WATCH THE LINE screen");
        assert_eq!(r.run().session.current_node().id, id);
    }

    #[test]
    fn without_a_backdoor_the_relay_stays_guarded() {
        let mut r = rig(4);
        r.skip_login();
        assert!(r.run().session.phreak_node().expect("a relay").guarded());
        assert!(!r.text().contains("BACKDOOR:"));
    }

    #[test]
    fn a_backdoor_on_a_host_without_a_relay_says_nothing() {
        let mut r = rig_with(0, |ctx| {
            let run = ctx.run.as_mut().unwrap();
            run.backdoors.insert(run.depth, Backdoor::Planted);
        });
        r.skip_login();
        assert!(r.run().session.phreak_node().is_none());
        assert!(!r.text().contains("BACKDOOR:"));
    }

    #[test]
    fn a_backdoor_planted_while_the_shell_is_open_opens_the_relay_at_once() {
        let mut r = rig(4);
        r.skip_login();
        let (id, near) = {
            let relay = r.run().session.phreak_node().expect("depth 4 has a relay");
            assert!(relay.guarded());
            (relay.id.clone(), relay.neighbors[0])
        };
        // The hidden line plants it from the second terminal; the app records it on the run.
        let run = r.ctx().run.as_mut().unwrap();
        run.backdoors.insert(run.depth, Backdoor::Planted);
        r.wait(0.1);
        assert!(!r.run().session.phreak_node().unwrap().guarded());
        let text = r.text();
        assert!(text.contains("BACKDOOR:"), "{text}");
        r.run().session.current = near;
        r.type_line(&format!("connect {id}"));
        assert!(r.in_shell(), "no WATCH THE LINE screen");
        assert_eq!(r.run().session.current_node().id, id);
    }

    /// Connect to a DAEMON gate somewhere; the first seed/depth that has one is enough.
    fn open_daemon_guard(r: &mut Rig) -> Option<usize> {
        r.skip_login();
        if !connect_to_guard(r, IceKindAt::Daemon) {
            return None;
        }
        assert!(r.run().in_text_input(), "typed ICE keeps ? as a character");
        let node = r.run().ice_node().expect("ice view");
        Some(node)
    }

    #[test]
    fn daemon_console_is_playable_through_the_real_key_path() {
        for seed_depth in [9u32, 10, 11, 13, 14, 2, 6, 3, 5, 7, 12, 15, 4, 8] {
            let mut r = rig_with(seed_depth, |_| {});
            let Some(node) = open_daemon_guard(&mut r) else {
                continue;
            };
            let solution = r.run().daemon_solution().expect("daemon view");
            for step in solution {
                r.type_line(&step);
            }
            assert!(r.in_shell(), "daemon solved at depth {seed_depth}");
            assert!(!r.run().session.nodes()[node].guarded(), "the gate opened");
            let t = r.text();
            assert!(
                t.contains(&format!(
                    "access granted: {}",
                    r.run().session.nodes()[node].id
                )),
                "{t}"
            );
            return;
        }
        panic!("no seed/depth produced a DAEMON gate");
    }

    #[test]
    fn daemon_wrong_commands_lock_out_with_trace_and_keep_the_node_guarded() {
        for seed_depth in [9u32, 10, 11, 13, 14, 2, 6, 3, 5, 7, 12, 15, 4, 8] {
            let mut r = rig_with(seed_depth, |_| {});
            let Some(node) = open_daemon_guard(&mut r) else {
                continue;
            };
            let before = r.run().session.trace();
            for _ in 0..5 {
                r.type_line("zzz");
            }
            assert!(r.in_shell(), "the console locked out");
            assert!(
                r.run().session.nodes()[node].guarded(),
                "the gate stays shut"
            );
            assert!(
                r.run().session.trace() >= before + 12.0,
                "the lockout adds trace: {} -> {}",
                before,
                r.run().session.trace()
            );
            return;
        }
        panic!("no seed/depth produced a DAEMON gate");
    }

    #[test]
    fn ice_keys_reach_the_game_and_esc_aborts_with_trace() {
        let mut r = rig(9);
        r.skip_login();
        assert!(
            connect_to_guard(&mut r, IceKindAt::Breach)
                || connect_to_guard(&mut r, IceKindAt::Phreak)
        );
        let before = r.run().session.trace();
        r.press(press(KeyCode::Esc));
        assert!(r.in_shell());
        let after = r.run().session.trace();
        assert!(
            after >= before + 5.0 - 0.5,
            "abort adds 5 trace: {before} -> {after}"
        );
        assert!(r.text().contains(&r.run().session.prompt().0));
    }

    #[test]
    fn breach_is_solvable_through_key_events_and_clears_the_node() {
        // Plays BREACH by feeding the planted solution found with the game's own search.
        let mut r = rig(0);
        r.skip_login();
        assert!(connect_to_guard(&mut r, IceKindAt::Breach));
        let id = {
            let Mode::Ice { node: Some(n), .. } = &r.run().mode else {
                panic!()
            };
            *n
        };
        // Search the visible grid for a legal pick order that completes the required daemon.
        let (grid, size, buffer, seq) = {
            let Mode::Ice { view, .. } = &r.run().mode else {
                panic!()
            };
            {
                let g = view.breach_game().unwrap();
                (g.grid.clone(), g.size, g.buffer, g.daemons[0].seq.clone())
            }
        };
        let path = solve_breach(&grid, size, buffer, &seq).expect("planted solution exists");
        for (row, col) in path {
            // The game moves the cursor to the next legal cell after every pick, so read it.
            loop {
                let Mode::Ice { view, .. } = &r.run().mode else {
                    panic!("the game ended early")
                };
                let cur = view.breach_game().unwrap().cursor;
                if cur == (row, col) {
                    break;
                }
                let key = if cur.1 < col {
                    KeyCode::Right
                } else if cur.1 > col {
                    KeyCode::Left
                } else if cur.0 < row {
                    KeyCode::Down
                } else {
                    KeyCode::Up
                };
                r.press(press(key));
            }
            r.press(press(KeyCode::Enter));
        }
        assert!(r.in_shell(), "{}", r.text());
        assert_eq!(r.run().session.current(), id, "clearing the ICE moves in");
        assert!(r.text().contains("access granted"));
    }

    fn solve_breach(
        grid: &[Vec<u8>],
        n: usize,
        buffer: usize,
        seq: &[u8],
    ) -> Option<Vec<(usize, usize)>> {
        fn go(
            grid: &[Vec<u8>],
            n: usize,
            buffer: usize,
            seq: &[u8],
            path: &mut Vec<(usize, usize)>,
        ) -> bool {
            let codes: Vec<u8> = path.iter().map(|&(r, c)| grid[r][c]).collect();
            if codes.windows(seq.len()).any(|w| w == seq) {
                return true;
            }
            if path.len() >= buffer {
                return false;
            }
            let options: Vec<(usize, usize)> = match path.last() {
                None => (0..n).map(|c| (0, c)).collect(),
                Some(&(_, c)) if path.len() % 2 == 1 => (0..n).map(|i| (i, c)).collect(),
                Some(&(r, _)) => (0..n).map(|i| (r, i)).collect(),
            };
            for cell in options {
                if path.contains(&cell) {
                    continue;
                }
                path.push(cell);
                if go(grid, n, buffer, seq, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        go(grid, n, buffer, seq, &mut path).then_some(path)
    }

    #[test]
    fn xyzzy_completes_the_objective_once_pays_and_opens_beat() {
        let mut r = rig(0);
        r.skip_login();
        let before = ratio(&mut r);
        r.type_line("xyzzy");
        assert_eq!(r.app.screen.name(), "Beat");
        assert_eq!(r.app.stack.len(), 1);
        assert_eq!(ratio(&mut r), before + danger::door_reward(1, None));
        let phase = r.ctx().run.as_ref().unwrap().phase;
        assert_eq!(
            phase,
            Phase::Hub {
                door_done: true,
                choice: None
            }
        );
        // Back on the shell (as after Beat and Choice) the reward is not paid twice.
        r.app.apply_nav(Nav::Back);
        assert_eq!(r.app.screen.name(), "Run");
        r.type_line("xyzzy");
        assert_eq!(ratio(&mut r), before + danger::door_reward(1, None));
        assert_eq!(r.app.screen.name(), "Run");
        // The save on disk shows the door done.
        let saved = r.ctx().store.load("tester").unwrap().unwrap();
        assert_eq!(
            saved.phase,
            Phase::Hub {
                door_done: true,
                choice: None
            }
        );
    }

    #[test]
    fn reward_follows_run_danger_and_the_exit_taken() {
        let mut r = rig_with(5, |ctx| {
            ctx.run.as_mut().unwrap().exit_taken = Some(ExitDef {
                kind: ExitKind::Deep,
                text: "a rope".into(),
            });
        });
        r.skip_login();
        let rd = r.run().session.run_danger();
        assert_eq!(rd, danger::run_danger(2, Some(ExitKind::Deep)));
        let before = ratio(&mut r);
        r.type_line("xyzzy");
        assert_eq!(
            ratio(&mut r),
            before + danger::door_reward(rd, Some(ExitKind::Deep))
        );
    }

    #[test]
    fn a_choice_made_during_the_beat_prints_the_jack_out_hint_once() {
        let mut r = rig(0);
        r.skip_login();
        r.type_line("xyzzy");
        r.ctx().run.as_mut().unwrap().phase = Phase::Hub {
            door_done: true,
            choice: Some(1),
        };
        r.app.apply_nav(Nav::Back);
        r.wait(0.2);
        r.wait(0.2);
        let t = r.text();
        assert_eq!(
            t.matches("The trace is still running. Jack out.").count(),
            1,
            "{t}"
        );
    }

    #[test]
    fn logout_leaves_for_the_menu_and_keeps_an_open_door_open() {
        let mut r = rig(0);
        r.skip_login();
        r.type_line("logout");
        assert_eq!(r.app.screen.name(), "Menu");
        let phase = r.ctx().run.as_ref().unwrap().phase;
        assert_eq!(phase, Phase::Arrival, "a failed run leaves the phase alone");
        for word in ["exit", "jackout"] {
            let mut r = rig(0);
            r.skip_login();
            r.type_line(word);
            assert_eq!(r.app.screen.name(), "Menu", "{word}");
        }
    }

    #[test]
    fn dump_shock_costs_ego_counts_an_attempt_and_returns_to_the_menu() {
        let mut r = rig(0);
        r.skip_login();
        r.run().session.trace = 99.99;
        r.wait(0.5);
        assert_eq!(ego(&mut r), 80);
        assert_eq!(r.ctx().run.as_ref().unwrap().door_attempts, 1);
        assert!(matches!(r.run().mode, Mode::Dump { .. }));
        let t = r.text();
        assert!(t.contains("DUMP SHOCK") && t.contains("EGO -20"), "{t}");
        // The trace clock is stopped while the banner is up.
        r.wait(0.5);
        assert_eq!(ego(&mut r), 80);
        r.wait(3.0);
        assert_eq!(r.app.screen.name(), "Menu");
        // The retry builds the next host.
        let retry = RunScreen::new(r.ctx());
        assert_eq!(retry.session.attempt(), 1);
        assert!(retry.session.trace() < 1.0);
    }

    #[test]
    fn dump_shock_after_the_objective_is_not_a_failed_attempt() {
        let mut r = rig(0);
        r.skip_login();
        r.run().session.objective = ObjectiveState::Done;
        r.run().session.trace = 99.99;
        r.wait(0.5);
        assert_eq!(ego(&mut r), 80);
        assert_eq!(r.ctx().run.as_ref().unwrap().door_attempts, 0);
    }

    #[test]
    fn a_key_skips_the_dump_shock_banner() {
        let mut r = rig(0);
        r.skip_login();
        r.run().session.trace = 99.99;
        r.wait(0.5);
        r.press(press_char(' '));
        assert_eq!(r.app.screen.name(), "Menu");
    }

    #[test]
    fn ego_zero_hands_the_app_to_death() {
        let mut r = rig(0);
        r.skip_login();
        r.ctx().run.as_mut().unwrap().stats.ego = 20;
        r.run().session.trace = 99.99;
        r.wait(0.5);
        assert_eq!(r.app.screen.name(), "Death");
    }

    #[test]
    fn using_programs_spends_charges_in_the_run_deck() {
        let mut r = rig_with(0, |ctx| {
            let deck = &mut ctx.run.as_mut().unwrap().deck;
            deck.insert(Program::Ghost, 1);
            deck.insert(Program::Mandala, 1);
        });
        r.skip_login();
        r.ctx().run.as_mut().unwrap().stats.ego = 50;
        let view = player_view(&r.app.ctx);
        r.run().session.set_player(view);
        r.type_line("run ghost");
        r.type_line("run mandala");
        let run = r.ctx().run.as_ref().unwrap();
        assert_eq!(run.stats.ego, 80, "MANDALA restores 30");
        assert!(run.deck.is_empty(), "both charges are gone: {:?}", run.deck);
        assert!(r.run().session.ghost_active());
        assert!(r.text().contains("(GHOST running)"));
    }

    #[test]
    fn mandala_restores_at_most_to_the_ego_cap() {
        let mut r = rig_with(0, |ctx| {
            ctx.run.as_mut().unwrap().deck.insert(Program::Mandala, 1);
        });
        r.skip_login();
        let view = player_view(&r.app.ctx);
        r.run().session.set_player(view);
        r.type_line("run mandala");
        assert_eq!(ego(&mut r), 100);
    }

    #[test]
    fn wordlock_charges_are_written_back_as_they_are_used() {
        let mut r = rig_with(0, |ctx| {
            ctx.run.as_mut().unwrap().deck.insert(Program::Decrypt, 2);
        });
        r.press(press_char('1'));
        let run = r.ctx().run.as_ref().unwrap();
        assert_eq!(run.deck.get(&Program::Decrypt), Some(&1));
        r.press(press_char('1'));
        let run = r.ctx().run.as_ref().unwrap();
        assert_eq!(run.deck.get(&Program::Decrypt), None, "{:?}", run.deck);
        assert!(r.text().contains("[1] DECRYPT x0"));
    }

    #[test]
    fn get_runs_a_transfer_then_credits_the_file() {
        let mut r = rig(9);
        r.skip_login();
        let (idx, file) = {
            let s = &r.run().session;
            s.nodes()
                .iter()
                .enumerate()
                .find_map(|(i, n)| {
                    n.files
                        .iter()
                        .find(|f| !f.is_target && f.reward_kb > 0)
                        .map(|f| (i, f.name.clone()))
                })
                .expect("an archive file exists")
        };
        r.run().session.current = idx;
        let before = ratio(&mut r);
        r.type_line(&format!("get {file}"));
        assert!(matches!(r.run().mode, Mode::Transfer(_)));
        let t = r.text();
        assert!(t.contains(&format!("ZMODEM  Receiving {file}")), "{t}");
        assert!(t.contains("Bytes ") && t.contains("CPS "));
        assert_eq!(ratio(&mut r), before, "paid when the transfer ends");
        r.wait(1.0);
        assert!(r.in_shell());
        assert_eq!(ratio(&mut r), before + 8);
        assert!(r.text().contains("+8K credited."));
    }

    #[test]
    fn bounce_runs_the_router_and_esc_cancels_without_burning_it() {
        let mut r = rig(0);
        r.skip_login();
        r.type_line("bounce");
        let t = r.text();
        assert!(t.contains("ROUTER") && t.contains("TIME 45.0"), "{t}");
        r.press(press(KeyCode::Esc));
        assert!(r.in_shell());
        assert!(!r.run().session.bounce_used());
        assert!(r.text().contains("bounce cancelled."));
    }

    #[test]
    fn minigame_time_runs_the_trace_at_half_rate() {
        let mut r = rig(0);
        r.skip_login();
        let rate = params(1).trace_per_10s / 10.0;
        r.wait(2.0);
        let shell = r.run().session.trace();
        assert!((shell - rate * 2.0).abs() < 0.05, "{shell}");
        r.type_line("bounce");
        let start = r.run().session.trace();
        r.wait(2.0);
        let ice = r.run().session.trace() - start;
        assert!((ice - rate).abs() < 0.05, "{ice}");
    }

    #[test]
    fn integrating_the_shadow_raises_integration_and_moves_in() {
        let mut found = false;
        for depth in [9u32, 10, 11, 13, 14] {
            let mut r = rig_with(depth, |ctx| {
                ctx.run.as_mut().unwrap().exit_taken = Some(ExitDef {
                    kind: ExitKind::Mirror,
                    text: "x".into(),
                });
            });
            r.skip_login();
            if !connect_to_guard(&mut r, IceKindAt::Shadow) {
                continue;
            }
            found = true;
            for _ in 0..8 {
                if r.text().contains("[X] ACCEPT NOW") {
                    break;
                }
                r.press(press_char('d'));
            }
            assert!(r.text().contains("[X] ACCEPT NOW"));
            let ego_before = ego(&mut r);
            r.press(press_char('x'));
            assert!(r.in_shell());
            assert_eq!(r.ctx().run.as_ref().unwrap().stats.integration, 2);
            assert!(ego_before > 0);
            let id = r.run().session.current_node().id.clone();
            assert!(id.starts_with("mirror"), "{id}");
            assert_eq!(r.run().session.current_node().kind, NodeKind::Mirror);
            break;
        }
        assert!(found, "no mirror node generated");
    }

    #[test]
    fn duel_damage_lands_on_the_run_ego_immediately() {
        let mut r = rig(9);
        r.skip_login();
        if !connect_to_guard(&mut r, IceKindAt::Black) {
            // Not every seed guards with black ICE; use the vault of a deeper level.
            r = rig(14);
            r.skip_login();
            assert!(connect_to_guard(&mut r, IceKindAt::Black));
        }
        for _ in 0..3 {
            r.press(press_char('d'));
        }
        let lost = 100 - ego(&mut r);
        assert!(lost > 0, "defended turns still take chip damage");
        let t = r.text();
        assert!(t.contains(&format!("{}", 100 - lost)), "{t}");
    }

    #[test]
    fn side_panel_lists_map_objective_and_deck() {
        let mut r = rig_with(0, |ctx| {
            let deck = &mut ctx.run.as_mut().unwrap().deck;
            deck.insert(Program::Decrypt, 2);
            deck.insert(Program::IcePick, 1);
        });
        r.skip_login();
        let t = r.text();
        assert!(t.contains("─ MAP "), "{t}");
        assert!(t.contains("─ OBJECTIVE "));
        assert!(t.contains("[ ] PENDING"));
        assert!(t.contains("get WREN-00.LOG"), "{t}");
        assert!(t.contains("─ DECK "));
        assert!(t.contains("PAYLOAD.PIF"));
        assert!(t.contains("DECRYPT.COM x2") && t.contains("ICEPICK.EXE"));
        assert!(t.contains('☺'), "the mini map marks where you are");
        // The separator runs the height of the panes.
        for y in 1..=22 {
            assert_eq!(r.row(y).chars().nth(54), Some('│'), "row {y}");
        }
        r.type_line("xyzzy");
        r.app.apply_nav(Nav::Back);
        assert!(r.text().contains("[X] DONE"));
    }

    #[test]
    fn a_screen_without_a_run_leaves_on_its_first_tick() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = RunScreen::new(&mut ctx);
        assert!(matches!(s.tick(FRAME, &mut ctx), Nav::Back));
    }

    /// The README gallery: twelve screens rendered through `App::render` and written as SVG
    /// under `docs/screenshots` (or `$ABRAXAS_SHOTS_DIR`). Ignored by default; re-run with
    /// `cargo test --lib write_readme_screenshots -- --ignored` when a screen changes. The same
    /// command also runs the IRC gallery in `irc.rs`, which writes the `irc*.svg` frames.
    mod screenshots {
        use super::*;
        use crate::ice::duel::Intent;
        use crate::ui::screens::{
            BoardScreen, ChatScreen, DescentScreen, LevelCardScreen, MenuScreen, PsycheScreen,
        };
        use crate::ui::shots;
        use ratatui::crossterm::event::KeyEvent;
        use std::path::{Path, PathBuf};

        /// Depths tried, in order, when a shot needs a particular guard on the host.
        const DEPTHS: [u32; 14] = [9, 10, 11, 13, 14, 2, 6, 3, 5, 7, 12, 15, 4, 8];

        fn buffer(app: &mut App) -> Buffer {
            let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
            term.draw(|f| app.render(f)).unwrap();
            term.backend().buffer().clone()
        }

        /// Truecolor, and calm so menu glitches, dissolves and vision clocks are deterministic.
        fn truecolor(ctx: &mut Ctx) {
            ctx.palette = Palette::new(true);
            ctx.config.display.calm = true;
        }

        /// An app on `make`'s screen over a registered run at `depth`.
        fn screen_app(
            depth: u32,
            make: impl FnOnce(&mut Ctx) -> Screen,
        ) -> (tempfile::TempDir, App) {
            let dir = tempfile::tempdir().unwrap();
            let mut ctx = test_ctx(dir.path());
            install_test_run(&mut ctx, depth);
            truecolor(&mut ctx);
            let screen = make(&mut ctx);
            (dir, App::with_screen(ctx, screen))
        }

        fn step(app: &mut App, key: Option<KeyEvent>) {
            app.step(key.map(Event::Key), FRAME);
        }

        fn type_line(app: &mut App, text: &str) {
            for c in text.chars() {
                step(app, Some(press_char(c)));
            }
            step(app, Some(press(KeyCode::Enter)));
        }

        fn wait(app: &mut App, secs: f32) {
            let mut left = secs;
            while left > 0.0 {
                let dt = left.min(0.1);
                app.step(None, Duration::from_secs_f32(dt));
                left -= dt;
            }
        }

        /// A logged-in rig at the first of `DEPTHS` whose host guards with `want`, connected to
        /// that guard. `prep` runs on each candidate before the connect.
        fn rig_at_guard(want: IceKindAt, prep: impl Fn(&mut Rig)) -> Rig {
            for depth in DEPTHS {
                let mut r = rig_with(depth, truecolor);
                r.skip_login();
                prep(&mut r);
                if connect_to_guard(&mut r, want) {
                    return r;
                }
            }
            panic!("no depth in {DEPTHS:?} guards with {want:?}");
        }

        fn shot(dir: &Path, name: &str, app: &mut App) {
            shots::write(dir, name, &buffer(app));
        }

        #[test]
        #[ignore = "writes docs/screenshots; run on demand"]
        fn write_readme_screenshots() {
            let out = std::env::var_os("ABRAXAS_SHOTS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/screenshots"))
                });

            // Boot, dial NIGHT SHIFT, let the line cross: the logo and `Handle:`.
            {
                let dir = tempfile::tempdir().unwrap();
                let mut ctx = test_ctx(dir.path());
                truecolor(&mut ctx);
                let mut app = App::new(ctx);
                app.step(None, Duration::from_millis(500));
                step(&mut app, Some(press_char(' ')));
                assert_eq!(app.screen.name(), "Dialer");
                step(&mut app, Some(press(KeyCode::Enter)));
                assert_eq!(app.screen.name(), "Dialing");
                for _ in 0..200 {
                    if app.screen.name() == "Login" {
                        break;
                    }
                    step(&mut app, Some(press_char(' ')));
                }
                assert_eq!(app.screen.name(), "Login");
                step(&mut app, None);
                shot(&out, "login", &mut app);
            }

            {
                let (_dir, mut app) = screen_app(0, |c| Screen::LevelCard(LevelCardScreen::new(c)));
                wait(&mut app, 2.0);
                assert_eq!(app.screen.name(), "LevelCard");
                shot(&out, "level-card", &mut app);
            }

            {
                let (_dir, mut app) = screen_app(0, |c| Screen::Menu(MenuScreen::new(c)));
                step(&mut app, None);
                shot(&out, "menu", &mut app);
            }

            // The last post is the engine's `re: front door` clue.
            {
                let (_dir, mut app) = screen_app(0, |c| Screen::Board(BoardScreen::new(c)));
                step(&mut app, Some(press(KeyCode::End)));
                step(&mut app, Some(press(KeyCode::Enter)));
                step(&mut app, None);
                shot(&out, "board", &mut app);
            }

            // Paging ends after 1.5 s; the offline director answers at once.
            {
                let (_dir, mut app) = screen_app(0, |c| Screen::Chat(ChatScreen::new(c)));
                wait(&mut app, 2.0);
                type_line(&mut app, "who are you?");
                for _ in 0..100 {
                    if app.screen.in_text_input() {
                        break;
                    }
                    step(&mut app, None);
                }
                assert!(app.screen.in_text_input(), "the sysop answered");
                shot(&out, "chat", &mut app);
            }

            // The shell after `scan`: graded guard labels in the scrollback, the mini-map.
            {
                let mut r = rig_with(7, truecolor);
                r.skip_login();
                r.type_line("scan");
                r.press(press_char(' '));
                assert!(r.in_shell());
                shot(&out, "shell", &mut r.app);
            }

            // The portal's WORDLOCK, before login.
            {
                let mut r = rig_with(3, truecolor);
                r.press(press_char(' '));
                shot(&out, "wordlock", &mut r.app);
            }

            // BREACH with one code picked, so the clock runs.
            {
                let mut r = rig_at_guard(IceKindAt::Breach, |_| {});
                r.press(press(KeyCode::Enter));
                shot(&out, "breach", &mut r.app);
            }

            // The DAEMON console on unix after its first step; the `ps` card (the process
            // table) when any depth deals it, else the first console found.
            {
                let mut found: Option<Rig> = None;
                for depth in DEPTHS {
                    let mut r = rig_with(depth, truecolor);
                    r.skip_login();
                    r.run().session.os = HostOs::Unix;
                    if !connect_to_guard(&mut r, IceKindAt::Daemon) {
                        continue;
                    }
                    let ps = r
                        .run()
                        .daemon_solution()
                        .is_some_and(|s| s[0].starts_with("ps"));
                    if ps || found.is_none() {
                        found = Some(r);
                    }
                    if ps {
                        break;
                    }
                }
                let mut r = found.expect("a DAEMON gate at some depth");
                let first = r.run().daemon_solution().expect("daemon view")[0].clone();
                r.type_line(&first);
                shot(&out, "daemon", &mut r.app);
            }

            // A DUEL against EYE mid-GAZE: the beam is drawn only while the animation runs.
            {
                let mut r = rig_at_guard(IceKindAt::Black, |r| {
                    for node in &mut r.run().session.nodes {
                        if node.ice == Some(IceKindAt::Black) {
                            node.class = Some(BlackClass::Eye);
                        }
                    }
                });
                let Mode::Ice { view, .. } = &mut r.run().mode else {
                    panic!("the duel is running")
                };
                view.set_calm(false);
                view.duel_mut().expect("duel").set_intent(Intent::Gaze);
                r.press(press_char('a'));
                wait(&mut r.app, 0.4);
                shot(&out, "duel", &mut r.app);
            }

            // The fall between levels: the vision, the captions, the hexagram.
            {
                let dir = tempfile::tempdir().unwrap();
                let mut ctx = test_ctx(dir.path());
                install_test_run(&mut ctx, 3);
                truecolor(&mut ctx);
                let pack = ctx.pack.clone();
                let run = ctx.run.as_mut().unwrap();
                assert!(run.apply_choice(&pack, 0));
                assert!(run.apply_exit(1));
                let screen = Screen::Descent(DescentScreen::new(&mut ctx));
                let mut app = App::with_screen(ctx, screen);
                wait(&mut app, 2.3);
                assert_eq!(app.screen.name(), "Descent");
                shot(&out, "descent", &mut app);
            }

            {
                let (_dir, mut app) = screen_app(0, |c| Screen::Psyche(PsycheScreen::new(c)));
                step(&mut app, None);
                shot(&out, "psyche", &mut app);
            }
        }
    }
}
