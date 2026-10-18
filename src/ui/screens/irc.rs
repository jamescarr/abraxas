//! The IRC screen: a level's network. Join channels, read the chatter, list what the fserv bots
//! sell and hack the locked ones.
//!
//! The engine (`game::irc`) holds the network and what each fserv sells; this screen draws it in
//! the Run screen's wireframe (header with the trace meter, scrollback, side panel, prompt),
//! runs the slash commands, plays the ICE mini-games that guard locked fservs and moves RATIO,
//! EGO and the run's deck. Chatter comes from the pack's pool, so it is offline and
//! deterministic. The IRC trace meter belongs to the visit: at 100 the server K-lines you.

use std::ops::RangeInclusive;
use std::time::Duration;

use rand::RngExt;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::ice_view::{IceView, VIEW_ROWS, ice_input};
use super::run::line_style;
use super::{Nav, ScreenImpl};
use crate::game::danger;
use crate::game::host::{IceKindAt, RETREAT_TRACE};
use crate::game::irc::{
    self, Channel, FILE_KB, GenInput, Good, HACK_TRACE, IrcNet, KLINE_EGO, Lock, TEARDROP_TRACE,
};
use crate::game::programs::{self, Program};
use crate::game::shell::{LineStyle, SHELL_COLS, wrap_lines};
use crate::game::state::IrcFile;
use crate::ice::{IceOutcome, Loadout};
use crate::palette::{Dos, Palette};
use crate::rng::{Rng, substream};
use crate::ui::app::Ctx;
use crate::ui::canvas::{WIDTH, fill, put_centered, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::input::{InputEvent, TextInput};
use crate::ui::widgets::progress::{self, BarStyle};
use crate::ui::widgets::viewer::Viewer;

/// Seconds the K-LINED banner stays up.
const KLINE_SECS: f32 = 2.5;
/// Lines PgUp/PgDn scroll.
const PAGE: usize = 10;
/// Scrollback lines kept.
const SCROLL_CAP: usize = 400;
/// Longest line the prompt accepts.
const MAX_INPUT: usize = 120;
/// Seconds between chatter lines.
const CHATTER_SECS: RangeInclusive<f32> = 2.5..=6.0;
/// Every fourth line in a channel with an fserv is the bot advertising itself.
const ADVERT_EVERY: u32 = 4;
/// Column of the pane separator and where the side panel starts.
const SEP_X: u16 = SHELL_COLS as u16;
const SIDE_X: u16 = SEP_X + 1;
const SIDE_W: u16 = WIDTH - SIDE_X;
/// Rows of scrollback (canvas rows 1..=22) and the prompt row.
const PANE_ROWS: u16 = 22;
const PROMPT_ROW: u16 = 23;
/// Size of the file reader.
const TEXT_WIDTH: u16 = 76;
const TEXT_ROWS: usize = 17;

#[derive(Debug)]
enum Mode {
    /// Typing in the current channel.
    Chat,
    /// An ICE mini-game guards the fserv at this index of `IrcNet::fservs`.
    Hack { view: Box<IceView>, fserv: usize },
    /// Reading a pulled file.
    Read { viewer: Viewer, title: String },
    /// K-lined: the banner stays until `left` runs out or a key.
    Kline { left: f32 },
}

/// The IRC screen.
#[derive(Debug)]
pub struct IrcScreen {
    net: IrcNet,
    /// Index into `net.channels`.
    channel: usize,
    /// The fserv last listed by `/fserv` (what `/get` takes from).
    fserv: Option<usize>,
    run_danger: u8,
    log: Vec<(LineStyle, String)>,
    /// Lines scrolled up from the newest.
    view_offset: usize,
    input: TextInput,
    mode: Mode,
    /// 0..=100; never decays during a visit.
    trace: f32,
    /// Mini-games launched so far (part of each game's RNG stream).
    launches: u32,
    chatter_rng: Rng,
    next_chatter: f32,
    chatter_count: u32,
    /// The run or its place is missing; the screen leaves on its first tick.
    orphan: bool,
}

impl IrcScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let (Some(run), Some(level)) = (ctx.run.as_ref(), ctx.level()) else {
            return Self::orphan();
        };
        let Some(place) = level.place.as_ref() else {
            return Self::orphan();
        };
        let run_danger = danger::run_danger(
            level.seed.base_danger,
            run.exit_taken.as_ref().map(|e| e.kind),
        );
        let shadow = run.shadow();
        let net = irc::generate(&GenInput {
            place,
            depth: run.depth,
            run_danger,
            pool: &ctx.pack.pool,
            seed: run.seed,
            handle: &run.handle,
            shadow: &shadow,
            real_name: &run.real_name,
        });
        let mut chatter_rng = substream(run.seed, run.depth, "irc-chatter", 0);
        let next_chatter = chatter_rng.random_range(CHATTER_SECS);
        let handle = run.handle.clone();
        let mut screen = Self {
            net,
            channel: 0,
            fserv: None,
            run_danger,
            log: Vec::new(),
            view_offset: 0,
            input: TextInput::new("", MAX_INPUT),
            mode: Mode::Chat,
            trace: 0.0,
            launches: 0,
            chatter_rng,
            next_chatter,
            chatter_count: 0,
            orphan: false,
        };
        let server = screen.net.server.clone();
        screen.push(
            LineStyle::Dim,
            &format!("*** Connecting to {server} port 6667..."),
        );
        screen.push(
            LineStyle::Dim,
            &format!("*** Welcome to {server}, {handle}"),
        );
        screen.join_block();
        screen
    }

    /// A screen built without a run or a place (never reached in play): it leaves on its first
    /// tick.
    fn orphan() -> Self {
        Self {
            net: IrcNet::default(),
            channel: 0,
            fserv: None,
            run_danger: 1,
            log: Vec::new(),
            view_offset: 0,
            input: TextInput::new("", MAX_INPUT),
            mode: Mode::Chat,
            trace: 0.0,
            launches: 0,
            chatter_rng: substream(0, 0, "irc-chatter", 0),
            next_chatter: 0.0,
            chatter_count: 0,
            orphan: true,
        }
    }

    // ----- log -----------------------------------------------------------------------------

    fn push(&mut self, style: LineStyle, text: &str) {
        for line in wrap_lines(text, SHELL_COLS) {
            self.log.push((style, line));
            if self.log.len() > SCROLL_CAP {
                self.log.remove(0);
            }
            // A reader scrolled up keeps their place while new lines arrive.
            if self.view_offset > 0 {
                self.view_offset = (self.view_offset + 1).min(self.log.len());
            }
        }
    }

    fn info(&mut self, text: &str) {
        self.push(LineStyle::Dim, text);
    }

    fn error(&mut self, text: &str) {
        self.push(LineStyle::Error, text);
    }

    // ----- the network ---------------------------------------------------------------------

    fn channel(&self) -> Option<&Channel> {
        self.net.channels.get(self.channel)
    }

    fn channel_name(&self) -> &str {
        self.channel().map_or("", |c| c.name.as_str())
    }

    /// The channel's users as IRC lists them: the fserv bots as `@bot` first, then the nicks.
    fn users(&self, channel: &Channel) -> Vec<String> {
        let bots = channel
            .fservs
            .iter()
            .filter_map(|&i| self.net.fservs.get(i))
            .map(|f| format!("@{}", f.bot));
        let nicks = channel
            .nicks
            .iter()
            .filter_map(|&i| self.net.nicks.get(i))
            .cloned();
        bots.chain(nicks).collect()
    }

    /// What IRC prints on joining the current channel; also points the prompt at it.
    fn join_block(&mut self) {
        let (name, topic, users) = {
            let Some(c) = self.channel() else {
                return;
            };
            (c.name.clone(), c.topic.clone(), self.users(c).join(" "))
        };
        self.input.set_prompt(format!("[{name}] "));
        self.info(&format!("*** Now talking in {name}"));
        self.info(&format!("*** Topic: {topic}"));
        self.info(&format!("*** Users: {users}"));
    }

    fn is_cracked(&self, index: usize, ctx: &Ctx) -> bool {
        let (Some(run), Some(fserv)) = (ctx.run.as_ref(), self.net.fservs.get(index)) else {
            return false;
        };
        run.irc.cracked.contains(&fserv.key(run.depth))
    }

    /// The fserv `arg` names when it sits in the current channel, else why not.
    fn find_fserv(&self, arg: &str) -> Result<usize, String> {
        let Some(index) = self
            .net
            .fservs
            .iter()
            .position(|f| f.bot.eq_ignore_ascii_case(arg))
        else {
            return Err(format!("*** No such fserv: {arg}"));
        };
        let fserv = &self.net.fservs[index];
        if fserv.channel == self.channel {
            return Ok(index);
        }
        let other = self
            .net
            .channels
            .get(fserv.channel)
            .map_or("another channel", |c| c.name.as_str());
        Err(format!("*** {} is in {other}", fserv.bot))
    }

    /// The locked, uncracked fserv `arg` names with its lock; prints the reason when there is
    /// none.
    fn lockable(&mut self, arg: &str, ctx: &Ctx) -> Option<(usize, Lock)> {
        let index = match self.find_fserv(arg) {
            Ok(index) => index,
            Err(e) => {
                self.error(&e);
                return None;
            }
        };
        let (bot, lock) = {
            let f = &self.net.fservs[index];
            (f.bot.clone(), f.lock)
        };
        match lock {
            None => {
                self.info(&format!("*** {bot} is open. just /get."));
                None
            }
            Some(_) if self.is_cracked(index, ctx) => {
                self.info(&format!("*** {bot} is already cracked."));
                None
            }
            Some(lock) => Some((index, lock)),
        }
    }

    /// Record the fserv as cracked and save.
    fn crack(&mut self, index: usize, ctx: &mut Ctx) {
        let Some(fserv) = self.net.fservs.get(index) else {
            return;
        };
        if let Some(run) = ctx.run.as_mut() {
            run.irc.cracked.insert(fserv.key(run.depth));
        }
        ctx.autosave();
    }

    /// Add (or, when negative, remove) trace. At 100 the server K-lines the player: EGO falls
    /// and the banner takes over.
    fn bump(&mut self, delta: f32, ctx: &mut Ctx) {
        self.trace = (self.trace + delta).clamp(0.0, 100.0);
        if self.trace < 100.0 || matches!(self.mode, Mode::Kline { .. }) {
            return;
        }
        if let Some(run) = ctx.run.as_mut() {
            run.stats.add_ego(KLINE_EGO);
        }
        ctx.autosave();
        self.mode = Mode::Kline { left: KLINE_SECS };
        self.error("*** K-LINED: the trace found you.");
    }

    // ----- chatter -------------------------------------------------------------------------

    /// One line of channel life: usually a nick talking, every few lines a bot advertising.
    fn speak(&mut self, ctx: &Ctx) {
        let Some(run) = ctx.run.as_ref() else {
            return;
        };
        self.chatter_count += 1;
        let bots: Vec<usize> = self.channel().map(|c| c.fservs.clone()).unwrap_or_default();
        if !bots.is_empty() && self.chatter_count.is_multiple_of(ADVERT_EVERY) {
            let turn = (self.chatter_count / ADVERT_EVERY - 1) as usize;
            let index = bots[turn % bots.len()];
            if let Some(fserv) = self.net.fservs.get(index) {
                let cracked = run.irc.cracked.contains(&fserv.key(run.depth));
                let line = format!("<{}> {}", fserv.bot, fserv.advert(cracked));
                self.push(LineStyle::Highlight, &line);
                return;
            }
        }
        let shadow = run.shadow();
        let fill = irc::filler(&run.handle, &shadow, &run.real_name);
        let (nick, text) = irc::chatter(
            &self.net,
            self.channel,
            &ctx.pack.pool,
            &mut self.chatter_rng,
            fill,
        );
        self.push(LineStyle::Normal, &format!("<{nick}> {text}"));
    }

    // ----- commands ------------------------------------------------------------------------

    fn submit(&mut self, text: &str, ctx: &mut Ctx) -> Nav {
        let text = text.trim();
        if text.is_empty() {
            return Nav::Stay;
        }
        if text.starts_with('/') {
            return self.command(text, ctx);
        }
        let handle = ctx.run.as_ref().map_or("", |r| r.handle.as_str());
        let line = format!("<{handle}> {text}");
        self.push(LineStyle::Normal, &line);
        Nav::Stay
    }

    fn command(&mut self, text: &str, ctx: &mut Ctx) -> Nav {
        let mut words = text.trim_start_matches('/').split_whitespace();
        let verb = words.next().unwrap_or_default().to_ascii_lowercase();
        let arg = words.next().unwrap_or_default().to_string();
        let needs = match verb.as_str() {
            "join" => Some("#channel"),
            "fserv" | "hack" | "teardrop" => Some("<bot>"),
            "get" | "read" => Some("<n>"),
            _ => None,
        };
        if let Some(what) = needs.filter(|_| arg.is_empty()) {
            self.error(&format!("*** usage: /{verb} {what}"));
            return Nav::Stay;
        }
        match verb.as_str() {
            "help" => self.help(),
            "list" => self.list(),
            "join" => self.join(&arg),
            "names" => self.names(),
            "fserv" => self.show_fserv(&arg, ctx),
            "get" => self.get(&arg, ctx),
            "hack" => self.hack(&arg, ctx),
            "teardrop" => self.teardrop(&arg, ctx),
            "files" => self.files(ctx),
            "read" => self.read(&arg, ctx),
            "quit" => return Nav::Back,
            other => self.error(&format!("*** unknown command: /{other} (/help)")),
        }
        Nav::Stay
    }

    fn help(&mut self) {
        for line in [
            "*** /list              channels and their fservs",
            "*** /join #chan        change channel",
            "*** /names             who is here",
            "*** /fserv <bot>       what a bot sells",
            "*** /get <n>           download item n from that bot",
            "*** /hack <bot>        crack a locked fserv",
            "*** /teardrop <bot>    crash a locked fserv",
            "*** /files             files you have downloaded",
            "*** /read <n>          read a downloaded file",
            "*** /quit              leave IRC",
        ] {
            self.info(line);
        }
    }

    fn list(&mut self) {
        let rows: Vec<String> = self
            .net
            .channels
            .iter()
            .map(|c| {
                format!(
                    "{}  {} users  {} fservs  {}",
                    c.name,
                    c.nicks.len() + c.fservs.len(),
                    c.fservs.len(),
                    c.topic
                )
            })
            .collect();
        for row in rows {
            self.push(LineStyle::Highlight, &row);
        }
    }

    fn join(&mut self, arg: &str) {
        match self
            .net
            .channels
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(arg))
        {
            Some(index) => {
                self.channel = index;
                self.fserv = None;
                self.join_block();
            }
            None => self.error(&format!("*** No such channel: {arg}")),
        }
    }

    fn names(&mut self) {
        let Some(c) = self.channel() else {
            return;
        };
        let line = format!("*** Users in {}: {}", c.name, self.users(c).join(" "));
        self.info(&line);
    }

    fn show_fserv(&mut self, arg: &str, ctx: &Ctx) {
        let index = match self.find_fserv(arg) {
            Ok(index) => index,
            Err(e) => {
                self.error(&e);
                return;
            }
        };
        self.fserv = Some(index);
        let Some(run) = ctx.run.as_ref() else {
            return;
        };
        let fserv = &self.net.fservs[index];
        let cracked = run.irc.cracked.contains(&fserv.key(run.depth));
        let mut rows = vec![format!("<{}> {}", fserv.bot, fserv.status(cracked))];
        for (n, good) in fserv.stock.iter().enumerate() {
            let yours = match good {
                Good::Program(p) => p.is_persistent() && run.deck_owns(*p),
                Good::File(f) => run.irc.has_file(run.depth, &fserv.bot, &f.name),
            };
            let suffix = if yours { " (yours)" } else { "" };
            rows.push(format!("[{}] {}{suffix}", n + 1, good.label()));
        }
        let footer = if fserv.is_locked(cracked) {
            format!("*** locked. /hack {0} or /teardrop {0}", fserv.bot)
        } else if cracked {
            "*** cracked. /get <n> is free".to_string()
        } else {
            format!(
                "*** type /get <n> to download. your ratio: {}K",
                run.stats.ratio_kb
            )
        };
        for row in rows {
            self.push(LineStyle::Highlight, &row);
        }
        self.info(&footer);
    }

    fn get(&mut self, arg: &str, ctx: &mut Ctx) {
        let Some(index) = self.fserv else {
            self.error("*** list an fserv first: /fserv <bot>");
            return;
        };
        let Some(fserv) = self.net.fservs.get(index).cloned() else {
            return;
        };
        let Some(good) = arg
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| fserv.stock.get(n))
        else {
            self.error("*** no such file");
            return;
        };
        let bot = &fserv.bot;
        let Some(run) = ctx.run.as_mut() else {
            return;
        };
        let depth = run.depth;
        let cracked = run.irc.cracked.contains(&fserv.key(depth));
        if fserv.is_locked(cracked) {
            self.error(&format!("*** {bot}: access denied. it is locked."));
            return;
        }
        let price = |kb: u32| if cracked { 0 } else { kb };
        let name = good.name();
        let done = match good {
            Good::Program(p) => {
                if p.is_persistent() && run.deck_owns(*p) {
                    self.error(&format!("*** you already have {name}."));
                    return;
                }
                let cost = price(p.cost_kb());
                let bought = if cracked {
                    programs::add(&mut run.deck, *p)
                } else {
                    run.buy(*p)
                };
                if !bought {
                    let ratio = run.stats.ratio_kb;
                    self.error(&format!("*** {bot}: ratio too low ({ratio}K < {cost}K)."));
                    return;
                }
                format!(
                    "*** DCC SEND {name} from {bot} ({cost}K) ... complete. It is in your deck."
                )
            }
            Good::File(f) => {
                if run.irc.has_file(depth, bot, &f.name) {
                    self.error(&format!("*** you already have {name}."));
                    return;
                }
                let cost = price(FILE_KB);
                if run.stats.ratio_kb < cost {
                    let ratio = run.stats.ratio_kb;
                    self.error(&format!("*** {bot}: ratio too low ({ratio}K < {cost}K)."));
                    return;
                }
                run.stats.ratio_kb -= cost;
                run.irc.files.push(IrcFile {
                    depth,
                    bot: bot.clone(),
                    file: f.clone(),
                });
                format!(
                    "*** DCC SEND {name} from {bot} ({} bytes) ... complete. /read {}",
                    f.body.len(),
                    run.irc.files.len()
                )
            }
        };
        self.info(&done);
        ctx.autosave();
    }

    fn hack(&mut self, arg: &str, ctx: &mut Ctx) {
        let Some((index, lock)) = self.lockable(arg, ctx) else {
            return;
        };
        self.bump(HACK_TRACE, ctx);
        if matches!(self.mode, Mode::Kline { .. }) {
            return;
        }
        let Some(run) = ctx.run.as_ref() else {
            return;
        };
        let bot = self.net.fservs[index].bot.clone();
        let mut rng = substream(run.seed, run.depth, &format!("irc-{bot}"), self.launches);
        self.launches += 1;
        let loadout = Loadout::from_deck(&run.deck);
        let params = danger::params(self.run_danger).graded(lock.grade);
        // `generate` only rolls BREACH, PHREAK and DAEMON locks.
        let mut view = match lock.ice {
            IceKindAt::Phreak => IceView::phreak(&mut rng, &params, loadout),
            IceKindAt::Daemon => IceView::daemon(&mut rng, lock.os, lock.grade),
            _ => IceView::breach(&mut rng, &params, loadout),
        };
        view.set_calm(ctx.calm());
        self.mode = Mode::Hack {
            view: Box::new(view),
            fserv: index,
        };
        self.info(&format!("*** probing {bot}..."));
    }

    fn teardrop(&mut self, arg: &str, ctx: &mut Ctx) {
        let Some((index, _)) = self.lockable(arg, ctx) else {
            return;
        };
        let spent = ctx
            .run
            .as_mut()
            .is_some_and(|run| programs::spend(&mut run.deck, Program::Teardrop));
        if !spent {
            self.error("*** you have no TEARDROP.SH.");
            return;
        }
        let bot = self.net.fservs[index].bot.clone();
        let channel = self.channel_name().to_string();
        self.crack(index, ctx);
        self.info(&format!("*** teardrop.sh {bot}"));
        self.info(&format!("*** {bot} has quit (Connection reset by peer)"));
        self.info(&format!("*** {bot} has joined {channel}"));
        self.bump(TEARDROP_TRACE, ctx);
    }

    fn files(&mut self, ctx: &Ctx) {
        let rows: Vec<String> = ctx
            .run
            .as_ref()
            .map(|run| {
                run.irc
                    .files
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        format!(
                            "[{}] {}  L{:02}  {}",
                            i + 1,
                            f.file.name,
                            f.depth,
                            f.file.description
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.is_empty() {
            self.info("*** no files yet.");
        }
        for row in rows {
            self.push(LineStyle::Highlight, &row);
        }
    }

    fn read(&mut self, arg: &str, ctx: &Ctx) {
        let file = arg
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| ctx.run.as_ref()?.irc.files.get(n));
        let Some(file) = file else {
            self.error("*** no such file");
            return;
        };
        self.mode = Mode::Read {
            viewer: Viewer::from_text(&file.file.body, TEXT_WIDTH, TEXT_ROWS, &ctx.palette),
            title: format!("{}  {}", file.file.name, file.file.description),
        };
    }

    // ----- mini-games ----------------------------------------------------------------------

    /// Drain the running game's side effects and, once it has an outcome, apply it to the run
    /// and the trace meter (what `RunScreen::pump_ice` does with a host session).
    fn pump(&mut self, ctx: &mut Ctx) {
        let Mode::Hack { view, fserv } = &mut self.mode else {
            return;
        };
        let fserv = *fserv;
        let trace = view.take_trace();
        let ego = view.take_ego();
        let spent = view.take_spent();
        let (kb, wipe) = view.rewards();
        let outcome = view.outcome();
        if let Some(run) = ctx.run.as_mut() {
            if ego != 0 {
                run.stats.add_ego(ego);
            }
            for (program, n) in spent {
                for _ in 0..n {
                    programs::spend(&mut run.deck, program);
                }
            }
        }
        if trace > 0.0 {
            self.bump(trace, ctx);
        }
        let Some(outcome) = outcome else {
            return;
        };
        if matches!(self.mode, Mode::Kline { .. }) {
            return;
        }
        self.mode = Mode::Chat;
        let bot = self
            .net
            .fservs
            .get(fserv)
            .map(|f| f.bot.clone())
            .unwrap_or_default();
        if kb > 0 {
            if let Some(run) = ctx.run.as_mut() {
                run.stats.ratio_kb = run.stats.ratio_kb.saturating_add(kb);
            }
            self.info(&format!("*** +{kb}K credited."));
        }
        if wipe != 0.0 {
            self.bump(wipe, ctx);
        }
        match outcome {
            IceOutcome::Cleared | IceOutcome::Integrated | IceOutcome::Destroyed => {
                self.crack(fserv, ctx);
                self.info(&format!(
                    "*** {bot}: ICE down. its stock is yours: /fserv {bot}"
                ));
            }
            IceOutcome::Failed { trace, ego } => {
                if ego != 0
                    && let Some(run) = ctx.run.as_mut()
                {
                    run.stats.add_ego(-ego.abs());
                }
                self.error(&format!("*** {bot} kicked you: ICE holds."));
                self.bump(trace.max(0.0), ctx);
            }
            IceOutcome::Aborted { trace } => {
                self.info(&format!("*** you back off {bot}."));
                self.bump(trace.max(0.0), ctx);
            }
            IceOutcome::Retreated => {
                self.info(&format!("*** you back off {bot}."));
                self.bump(RETREAT_TRACE, ctx);
            }
        }
    }

    /// The DAEMON console's step solutions, when a DAEMON view is running (tests).
    #[cfg(test)]
    pub(crate) fn daemon_solution(&self) -> Option<Vec<String>> {
        match &self.mode {
            Mode::Hack { view, .. } => view.daemon_game().map(|g| g.solution()),
            _ => None,
        }
    }

    // ----- input ---------------------------------------------------------------------------

    fn chat_key(&mut self, key: &KeyEvent, ctx: &mut Ctx) -> Nav {
        match classify(key) {
            Key::PgUp => {
                self.view_offset = (self.view_offset + PAGE).min(self.log.len());
                Nav::Stay
            }
            Key::PgDn => {
                self.view_offset = self.view_offset.saturating_sub(PAGE);
                Nav::Stay
            }
            Key::Esc => {
                if self.input.is_empty() {
                    return Nav::Back;
                }
                self.input.clear();
                Nav::Stay
            }
            _ => match self.input.handle_key(key) {
                InputEvent::Submit(text) => {
                    self.view_offset = 0;
                    self.input.clear();
                    self.submit(&text, ctx)
                }
                _ => Nav::Stay,
            },
        }
    }
}

impl ScreenImpl for IrcScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match &mut self.mode {
            Mode::Kline { .. } => Nav::Back,
            Mode::Read { viewer, .. } => {
                if !viewer.handle_key(&key)
                    && matches!(classify(&key), Key::Esc | Key::Char('q' | 'Q'))
                {
                    self.mode = Mode::Chat;
                }
                Nav::Stay
            }
            Mode::Hack { view, .. } => {
                if let Some(input) = ice_input(classify(&key)) {
                    view.handle(input);
                }
                self.pump(ctx);
                Nav::Stay
            }
            Mode::Chat => self.chat_key(&key, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if self.orphan {
            return Nav::Back;
        }
        match &mut self.mode {
            Mode::Kline { left } => {
                *left -= dt.as_secs_f32();
                return if *left <= 0.0 { Nav::Back } else { Nav::Stay };
            }
            Mode::Hack { view, .. } => {
                view.tick(dt);
                self.pump(ctx);
            }
            Mode::Chat | Mode::Read { .. } => {}
        }
        // Chatter is content, so it runs in calm mode too; the pump above may have K-lined.
        if !matches!(self.mode, Mode::Kline { .. }) {
            self.next_chatter -= dt.as_secs_f32();
            if self.next_chatter <= 0.0 {
                self.speak(ctx);
                self.next_chatter = self.chatter_rng.random_range(CHATTER_SECS);
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        fill(buf, area, p.style(Dos::LightGray, Dos::Black));
        self.render_header(buf, area, ctx);
        match &self.mode {
            Mode::Hack { view, .. } => {
                view.render(buf, Rect::new(area.x, area.y + 1, WIDTH, VIEW_ROWS), p);
                return;
            }
            Mode::Read { viewer, title } => {
                put_str(
                    buf,
                    area.x + 1,
                    area.y + 2,
                    title,
                    WIDTH - 2,
                    p.style(Dos::Yellow, Dos::Black),
                );
                let rows = u16::try_from(TEXT_ROWS + 1).unwrap_or(18);
                viewer.render(buf, Rect::new(area.x + 2, area.y + 3, TEXT_WIDTH, rows), p);
                put_str(
                    buf,
                    area.x + 1,
                    area.y + PROMPT_ROW,
                    "Esc back",
                    WIDTH - 2,
                    p.style(Dos::DarkGray, Dos::Black),
                );
                return;
            }
            Mode::Chat | Mode::Kline { .. } => {}
        }
        self.render_log(buf, area, ctx);
        self.render_side(buf, area, ctx);
        self.input.render(
            buf,
            Rect::new(area.x, area.y + PROMPT_ROW, WIDTH, 1),
            ctx.fg(Dos::White),
        );
        if matches!(self.mode, Mode::Kline { .. }) {
            render_kline(buf, area, p);
        }
    }

    fn wants_status(&self) -> bool {
        true
    }

    fn in_text_input(&self) -> bool {
        match &self.mode {
            Mode::Chat => true,
            Mode::Hack { view, .. } => view.wants_text(),
            Mode::Read { .. } | Mode::Kline { .. } => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

impl IrcScreen {
    fn render_header(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let bar_bg = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, WIDTH, 1), bar_bg);
        let handle = ctx.run.as_ref().map_or("", |r| r.handle.as_str());
        let left = format!(" {} │ {} │ {handle}", self.net.server, self.channel_name());
        put_str(buf, area.x, area.y, &left, 54, bar_bg);
        let color = if self.trace >= 80.0 {
            Dos::LightRed
        } else if self.trace >= 50.0 {
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
            self.trace / 100.0,
            BarStyle {
                filled: p.style(color, Dos::Blue),
                empty: p.style(Dos::Cyan, Dos::Blue),
            },
            true,
        );
    }

    fn render_log(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let end = self.log.len().saturating_sub(self.view_offset);
        let start = end.saturating_sub(usize::from(PANE_ROWS));
        for (i, (style, text)) in self.log[start..end].iter().enumerate() {
            put_str(
                buf,
                area.x,
                area.y + 1 + u16::try_from(i).unwrap_or(0),
                text,
                SEP_X,
                line_style(p, *style),
            );
        }
        if self.view_offset > 0 {
            put_str(
                buf,
                area.x + SEP_X - 14,
                area.y + 1,
                "-- scrolled --",
                14,
                p.style(Dos::Black, Dos::LightGray),
            );
        }
    }

    fn render_side(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let sep = p.style(Dos::DarkGray, Dos::Black);
        for y in 1..=PANE_ROWS {
            put_str(buf, area.x + SEP_X, area.y + y, "│", 1, sep);
        }
        let Some(channel) = self.channel() else {
            return;
        };
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
        let bold = p.style(Dos::Yellow, Dos::Black);

        let mut y = 1;
        title(buf, y, "USERS");
        y += 1;
        for &i in &channel.fservs {
            if let Some(fserv) = self.net.fservs.get(i) {
                put_str(
                    buf,
                    sx + 1,
                    area.y + y,
                    &format!("@{}", fserv.bot),
                    SIDE_W - 1,
                    bold,
                );
                y += 1;
            }
        }
        for &i in &channel.nicks {
            if let Some(nick) = self.net.nicks.get(i) {
                put_str(buf, sx + 1, area.y + y, nick, SIDE_W - 1, text);
                y += 1;
            }
        }
        y += 1;
        title(buf, y, "FSERVS");
        y += 1;
        for &i in &channel.fservs {
            if let Some(fserv) = self.net.fservs.get(i) {
                let line = format!("{} {}", fserv.bot, fserv.state(self.is_cracked(i, ctx)));
                put_str(buf, sx + 1, area.y + y, &line, SIDE_W - 1, text);
                y += 1;
            }
        }
        y += 1;
        let files = ctx.run.as_ref().map_or(0, |r| r.irc.files.len());
        title(buf, y, &format!("FILES {files}"));
        put_str(
            buf,
            sx + 1,
            area.y + y + 1,
            "/files",
            SIDE_W - 1,
            p.style(Dos::DarkGray, Dos::Black),
        );
    }
}

fn render_kline(buf: &mut Buffer, area: Rect, p: &Palette) {
    let red = p.style(Dos::White, Dos::Red);
    // The whole log pane, so no fragment of the `***` lines behind it shows at the edges.
    let panel = Rect::new(area.x, area.y + 8, SEP_X, 7);
    fill(buf, panel, red);
    let rows = [
        "*** K-LINED ***".to_string(),
        String::new(),
        "The trace found you. The server drops you.".to_string(),
        format!("EGO {KLINE_EGO}"),
        String::new(),
        "press a key".to_string(),
    ];
    for (i, text) in rows.iter().enumerate() {
        let y = panel.y + 1 + u16::try_from(i).unwrap_or(0);
        put_centered(buf, panel, y, text, red);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::IceGrade;
    use crate::game::irc::CHANNELS;
    use crate::ice::daemon::TRACE_ON_LOCKOUT;
    use crate::pack::model::HostOs;
    use crate::ui::app::{App, install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use crate::ui::screens::Screen;
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
        let screen = Screen::Irc(IrcScreen::new(&mut ctx));
        Rig {
            _dir: dir,
            app: App::with_screen(ctx, screen),
        }
    }

    fn rig() -> Rig {
        rig_with(0, |_| {})
    }

    impl Rig {
        fn irc(&mut self) -> &mut IrcScreen {
            match &mut self.app.screen {
                Screen::Irc(s) => s,
                other => panic!("expected the Irc screen, found {}", other.name()),
            }
        }

        fn press(&mut self, ev: KeyEvent) {
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

        /// The scrollback as running text: wrapped lines are joined back with a space.
        fn log(&mut self) -> String {
            let lines: Vec<String> = self.irc().log.iter().map(|(_, l)| l.clone()).collect();
            lines.join(" ")
        }

        fn bot(&mut self, i: usize) -> String {
            self.irc().net.fservs[i].bot.clone()
        }

        fn channel_of(&mut self, i: usize) -> String {
            let net = &self.irc().net;
            net.channels[net.fservs[i].channel].name.clone()
        }

        fn join_for(&mut self, i: usize) {
            let channel = self.channel_of(i);
            self.type_line(&format!("/join {channel}"));
        }

        fn lock(&mut self, i: usize, ice: IceKindAt, grade: IceGrade) {
            self.irc().net.fservs[i].lock = Some(Lock {
                ice,
                grade,
                os: HostOs::Unix,
            });
        }
    }

    fn ratio(r: &mut Rig) -> u32 {
        r.ctx().run.as_ref().unwrap().stats.ratio_kb
    }

    fn ego(r: &mut Rig) -> i32 {
        r.ctx().run.as_ref().unwrap().stats.ego
    }

    fn left54(row: &str) -> String {
        row.chars().take(54).collect()
    }

    #[test]
    fn connecting_joins_the_first_channel_and_draws_the_wireframe() {
        let mut r = rig();
        let (server, channel) = {
            let net = &r.irc().net;
            (net.server.clone(), net.channels[0].name.clone())
        };
        let tagline = r.ctx().place().unwrap().tagline.clone();
        let log = r.log();
        assert!(log.contains(&format!("*** Connecting to {server} port 6667...")));
        assert!(log.contains(&format!("*** Welcome to {server}, Tester")));
        assert!(log.contains(&format!("*** Now talking in {channel}")));
        assert!(log.contains(&format!("*** Topic: {tagline}")));
        let header = r.row(0);
        assert!(header.contains(&server), "{header}");
        assert!(header.contains(&channel), "{header}");
        assert!(header.contains("Tester"), "{header}");
        assert!(
            header.contains("TRACE") && header.contains(" 0%"),
            "{header}"
        );
        let prompt = r.row(23);
        assert!(prompt.starts_with(&format!("[{channel}] ")), "{prompt}");
    }

    #[test]
    fn list_names_every_channel_and_the_side_panel_shows_users_and_fservs() {
        let mut r = rig();
        r.type_line("/list");
        let rows: Vec<String> = r
            .irc()
            .log
            .iter()
            .map(|(_, l)| l.clone())
            .filter(|l| l.starts_with('#'))
            .collect();
        assert_eq!(rows.len(), CHANNELS, "{rows:?}");
        let bot = r.bot(0);
        let t = r.text();
        assert!(t.contains("─ USERS ─"), "{t}");
        assert!(t.contains("─ FSERVS ─"), "{t}");
        assert!(t.contains("─ FILES 0 ─"), "{t}");
        assert!(t.contains(&format!("@{bot}")), "{t}");
        assert!(t.contains(&format!("{bot} open")), "{t}");
    }

    #[test]
    fn nicks_of_the_channel_talk_on_a_timer() {
        let mut r = rig();
        r.wait(7.0);
        let nicks: Vec<String> = {
            let net = &r.irc().net;
            net.channels[0]
                .nicks
                .iter()
                .map(|&i| net.nicks[i].clone())
                .collect()
        };
        let log = r.log();
        assert!(
            nicks.iter().any(|n| log.contains(&format!("<{n}> "))),
            "{log}"
        );
    }

    #[test]
    fn chatter_keeps_a_steady_pace_and_every_fourth_line_is_an_advert() {
        let mut r = rig();
        r.wait(30.0);
        let bot = r.bot(0);
        let events: Vec<(LineStyle, String)> = r
            .irc()
            .log
            .iter()
            .filter(|(_, l)| l.starts_with('<'))
            .cloned()
            .collect();
        assert!((5..=13).contains(&events.len()), "{} lines", events.len());
        let advert = format!("<{bot}> fserv online");
        let adverts: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, (_, l))| l.starts_with(&advert))
            .map(|(i, _)| i + 1)
            .collect();
        assert!(!adverts.is_empty(), "{events:?}");
        assert!(adverts.iter().all(|n| n % 4 == 0), "{adverts:?}");
        let bold = events
            .iter()
            .filter(|(s, _)| *s == LineStyle::Highlight)
            .count();
        assert_eq!(bold, adverts.len(), "only adverts are highlighted");
        let whole = format!("{advert} ─ /fserv {bot} ─ 3 files ─ open");
        assert!(r.log().contains(&whole), "{}", r.log());
    }

    #[test]
    fn chatter_is_deterministic_for_a_seed_and_runs_in_calm_mode() {
        let play = |calm: bool| {
            let mut r = rig_with(0, |ctx| ctx.config.display.calm = calm);
            r.wait(20.0);
            r.log()
        };
        let normal = play(false);
        assert_eq!(normal, play(false));
        assert_eq!(normal, play(true), "calm mode only freezes animation");
    }

    #[test]
    fn a_scrolled_view_keeps_its_place_while_chatter_arrives() {
        let mut r = rig();
        for _ in 0..3 {
            r.type_line("/help");
        }
        r.press(press(KeyCode::PageUp));
        let before: Vec<String> = (1..=22).map(|y| left54(&r.row(y))).collect();
        assert!(before[0].contains("-- scrolled --"), "{before:?}");
        let offset = r.irc().view_offset;
        r.wait(10.0);
        let after: Vec<String> = (1..=22).map(|y| left54(&r.row(y))).collect();
        assert_eq!(before, after);
        assert!(r.irc().view_offset > offset, "new lines pushed the offset");
        r.type_line("hello there");
        assert_eq!(r.irc().view_offset, 0);
        assert!(r.text().contains("<Tester> hello there"));
    }

    #[test]
    fn the_open_fserv_sells_teardrop_and_zines_for_ratio() {
        let mut r = rig();
        let bot = r.bot(0);
        r.type_line(&format!("/fserv {bot}"));
        let log = r.log();
        assert!(log.contains(&format!("<{bot}> open")), "{log}");
        assert!(log.contains("[1] TEARDROP.SH  64K"), "{log}");
        assert!(
            log.contains("*** type /get <n> to download. your ratio: 96K"),
            "{log}"
        );
        r.type_line("/get 3");
        assert_eq!(ratio(&mut r), 96 - FILE_KB);
        let file = r.ctx().run.as_ref().unwrap().irc.files[0].clone();
        assert_eq!((file.depth, file.bot.as_str()), (0, bot.as_str()));
        let log = r.log();
        let sent = format!(
            "*** DCC SEND {} from {bot} ({} bytes) ... complete. /read 1",
            file.file.name,
            file.file.body.len()
        );
        assert!(log.contains(&sent), "{log}");
        r.type_line("/files");
        let listed = format!("[1] {}  L00  {}", file.file.name, file.file.description);
        assert!(r.log().contains(&listed), "{}", r.log());
        r.type_line("/read 1");
        let t = r.text();
        let first = file.file.body.lines().next().unwrap();
        assert!(t.contains(&file.file.name), "{t}");
        assert!(t.contains(first), "{t}");
        assert!(t.contains("Esc back"), "{t}");
        r.press(press(KeyCode::Esc));
        assert!(matches!(r.irc().mode, Mode::Chat));
        r.type_line(&format!("/fserv {bot}"));
        let row = format!(
            "[3] {}  {FILE_KB}K  {} (yours)",
            file.file.name, file.file.description
        );
        assert!(r.log().contains(&row), "{}", r.log());
    }

    #[test]
    fn a_low_ratio_refuses_the_download() {
        let mut r = rig_with(0, |ctx| ctx.run.as_mut().unwrap().stats.ratio_kb = 0);
        let bot = r.bot(0);
        r.type_line(&format!("/fserv {bot}"));
        r.type_line("/get 3");
        let log = r.log();
        assert!(
            log.contains(&format!("*** {bot}: ratio too low (0K < 8K).")),
            "{log}"
        );
        assert!(r.ctx().run.as_ref().unwrap().irc.files.is_empty());
        r.type_line("/files");
        assert!(r.log().contains("*** no files yet."));
    }

    #[test]
    fn programs_go_to_the_deck_and_persistent_ones_are_not_sold_twice() {
        let mut r = rig();
        r.irc().net.fservs[0].stock = vec![
            Good::Program(Program::IcePick),
            Good::Program(Program::Decrypt),
        ];
        let bot = r.bot(0);
        r.type_line(&format!("/fserv {bot}"));
        r.type_line("/get 1");
        assert!(r.ctx().run.as_ref().unwrap().deck_owns(Program::IcePick));
        assert_eq!(ratio(&mut r), 96 - 80);
        let sent =
            format!("*** DCC SEND ICEPICK.EXE from {bot} (80K) ... complete. It is in your deck.");
        assert!(r.log().contains(&sent), "{}", r.log());
        r.type_line("/get 1");
        assert!(r.log().contains("*** you already have ICEPICK.EXE."));
        assert_eq!(ratio(&mut r), 16);
        r.type_line(&format!("/fserv {bot}"));
        assert!(
            r.log()
                .contains("[1] ICEPICK.EXE  80K  DUEL: attack +6 (yours)")
        );
        r.type_line("/get 2");
        assert!(
            r.log()
                .contains(&format!("*** {bot}: ratio too low (16K < 48K)."))
        );
        // Consumables stack.
        r.ctx().run.as_mut().unwrap().stats.ratio_kb = 100;
        r.type_line("/get 2");
        r.type_line("/get 2");
        let run = r.ctx().run.as_ref().unwrap();
        assert_eq!(programs::charges(&run.deck, Program::Decrypt), 4);
        assert_eq!(run.stats.ratio_kb, 4);
    }

    #[test]
    fn get_needs_a_listed_fserv_and_a_real_item() {
        let mut r = rig();
        r.type_line("/get 1");
        assert!(r.log().contains("*** list an fserv first: /fserv <bot>"));
        let bot = r.bot(0);
        r.type_line(&format!("/fserv {bot}"));
        for bad in ["9", "0", "zero"] {
            r.type_line(&format!("/get {bad}"));
        }
        let log = r.log();
        assert_eq!(log.matches("*** no such file").count(), 3, "{log}");
        r.type_line("/get");
        assert!(r.log().contains("*** usage: /get <n>"));
        assert_eq!(ratio(&mut r), 96);
    }

    #[test]
    fn a_locked_fserv_sells_nothing_until_cracked() {
        let mut r = rig();
        let (bot, channel) = (r.bot(1), r.channel_of(1));
        r.type_line(&format!("/fserv {bot}"));
        assert!(r.log().contains(&format!("*** {bot} is in {channel}")));
        r.type_line("/fserv ghostbot");
        assert!(r.log().contains("*** No such fserv: ghostbot"));
        r.join_for(1);
        r.type_line(&format!("/fserv {bot}"));
        let log = r.log();
        assert!(log.contains(&format!("<{bot}> locked (")), "{log}");
        assert!(
            log.contains(&format!("*** locked. /hack {bot} or /teardrop {bot}")),
            "{log}"
        );
        r.type_line("/get 1");
        assert!(
            r.log()
                .contains(&format!("*** {bot}: access denied. it is locked."))
        );
        assert_eq!(ratio(&mut r), 96);
        assert!(r.text().contains(&format!("{bot} locked")));
    }

    #[test]
    fn join_switches_channel_prompt_and_panel() {
        let mut r = rig();
        let (channel, nick) = {
            let net = &r.irc().net;
            let c = &net.channels[1];
            (c.name.clone(), net.nicks[c.nicks[0]].clone())
        };
        r.type_line(&format!("/join {}", channel.to_uppercase()));
        assert!(
            r.row(23).starts_with(&format!("[{channel}] ")),
            "{}",
            r.row(23)
        );
        assert!(r.row(0).contains(&channel), "{}", r.row(0));
        assert!(r.text().contains(&nick), "{}", r.text());
        assert!(r.log().contains(&format!("*** Now talking in {channel}")));
        r.type_line("/names");
        assert!(r.log().contains(&format!("*** Users in {channel}: ")));
        r.type_line("/join #nosuch");
        assert!(r.log().contains("*** No such channel: #nosuch"));
        assert_eq!(r.irc().channel, 1, "a failed join stays put");
    }

    #[test]
    fn hacking_a_locked_fserv_starts_its_ice_and_esc_backs_off() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.type_line(&format!("/hack {bot}"));
        let t = r.text();
        assert!(
            t.contains("BREACH PROTOCOL")
                || t.contains("PHREAK  RELAY LINE")
                || t.contains("CONSOLE"),
            "{t}"
        );
        assert!(r.row(0).contains(" 5%"), "{}", r.row(0));
        r.press(press(KeyCode::Esc));
        assert!(matches!(r.irc().mode, Mode::Chat));
        assert!(r.irc().trace > HACK_TRACE);
        assert!(r.log().contains(&format!("*** you back off {bot}.")));
        assert!(
            !r.ctx()
                .run
                .as_ref()
                .unwrap()
                .irc
                .cracked
                .contains(&format!("0:{bot}"))
        );
    }

    #[test]
    fn hacking_an_open_fserv_costs_nothing() {
        let mut r = rig();
        let bot = r.bot(0);
        r.type_line(&format!("/hack {bot}"));
        assert!(r.log().contains(&format!("*** {bot} is open. just /get.")));
        r.type_line(&format!("/teardrop {bot}"));
        assert_eq!(r.log().matches("is open. just /get.").count(), 2);
        assert_eq!(r.irc().trace, 0.0);
        assert!(matches!(r.irc().mode, Mode::Chat));
    }

    #[test]
    fn each_lock_kind_starts_its_own_game() {
        for (ice, label) in [
            (IceKindAt::Breach, "BREACH PROTOCOL"),
            (IceKindAt::Phreak, "PHREAK  RELAY LINE"),
            (IceKindAt::Daemon, "CONSOLE"),
        ] {
            let mut r = rig();
            r.join_for(1);
            let bot = r.bot(1);
            r.lock(1, ice, IceGrade::Standard);
            r.type_line(&format!("/hack {bot}"));
            let t = r.text();
            assert!(t.contains(label), "{label}: {t}");
        }
    }

    #[test]
    fn solving_the_console_cracks_the_fserv_and_its_stock_is_free() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.lock(1, IceKindAt::Daemon, IceGrade::Soft);
        r.type_line(&format!("/hack {bot}"));
        for step in r.irc().daemon_solution().expect("a console") {
            r.type_line(&step);
        }
        assert!(matches!(r.irc().mode, Mode::Chat));
        let log = r.log();
        assert!(
            log.contains(&format!(
                "*** {bot}: ICE down. its stock is yours: /fserv {bot}"
            )),
            "{log}"
        );
        assert!(
            r.ctx()
                .run
                .as_ref()
                .unwrap()
                .irc
                .cracked
                .contains(&format!("0:{bot}"))
        );
        r.type_line(&format!("/fserv {bot}"));
        let log = r.log();
        assert!(log.contains(&format!("<{bot}> cracked")), "{log}");
        assert!(log.contains("*** cracked. /get <n> is free"), "{log}");
        let before = ratio(&mut r);
        r.type_line("/get 1");
        assert_eq!(ratio(&mut r), before, "a cracked fserv sells for free");
        assert!(r.log().contains("(0K) ... complete. It is in your deck."));
        assert!(r.text().contains(&format!("{bot} cracked")));
    }

    #[test]
    fn locking_out_of_the_console_kicks_you_and_costs_trace() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.lock(1, IceKindAt::Daemon, IceGrade::Standard);
        r.type_line(&format!("/hack {bot}"));
        for _ in 0..20 {
            if !matches!(r.irc().mode, Mode::Hack { .. }) {
                break;
            }
            r.type_line("zzz");
        }
        assert!(matches!(r.irc().mode, Mode::Chat));
        assert!(
            r.log()
                .contains(&format!("*** {bot} kicked you: ICE holds."))
        );
        let trace = r.irc().trace;
        assert!(
            (trace - (HACK_TRACE + TRACE_ON_LOCKOUT)).abs() < 0.01,
            "{trace}"
        );
        assert!(
            !r.ctx()
                .run
                .as_ref()
                .unwrap()
                .irc
                .cracked
                .contains(&format!("0:{bot}"))
        );
        r.type_line(&format!("/fserv {bot}"));
        assert!(r.log().contains(&format!("<{bot}> locked (")));
    }

    #[test]
    fn teardrop_crashes_a_locked_fserv_for_trace_and_one_charge() {
        let mut r = rig();
        r.join_for(1);
        let (bot, channel) = (r.bot(1), r.channel_of(1));
        r.type_line(&format!("/teardrop {bot}"));
        assert!(r.log().contains("*** you have no TEARDROP.SH."));
        assert_eq!(r.irc().trace, 0.0);
        r.ctx()
            .run
            .as_mut()
            .unwrap()
            .deck
            .insert(Program::Teardrop, 1);
        r.type_line(&format!("/teardrop {bot}"));
        let log = r.log();
        assert!(
            log.contains(&format!("*** {bot} has quit (Connection reset by peer)")),
            "{log}"
        );
        assert!(
            log.contains(&format!("*** {bot} has joined {channel}")),
            "{log}"
        );
        assert!(!r.ctx().run.as_ref().unwrap().deck_owns(Program::Teardrop));
        assert!((r.irc().trace - TEARDROP_TRACE).abs() < 0.01);
        assert!(r.row(0).contains("15%"), "{}", r.row(0));
        r.type_line(&format!("/fserv {bot}"));
        assert!(r.log().contains(&format!("<{bot}> cracked")));
        let before = ratio(&mut r);
        r.type_line("/get 1");
        assert_eq!(ratio(&mut r), before);
        r.type_line(&format!("/hack {bot}"));
        assert!(r.log().contains(&format!("*** {bot} is already cracked.")));
    }

    #[test]
    fn a_full_trace_klines_you_and_the_menu_takes_over() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.irc().trace = 99.0;
        r.type_line(&format!("/hack {bot}"));
        assert!(r.log().contains("*** K-LINED: the trace found you."));
        assert_eq!(ego(&mut r), 100 + KLINE_EGO);
        assert!(matches!(r.irc().mode, Mode::Kline { .. }));
        let t = r.text();
        assert!(
            t.contains("*** K-LINED ***") && t.contains("EGO -20"),
            "{t}"
        );
        r.wait(KLINE_SECS + 0.1);
        assert_eq!(r.app.screen.name(), "Menu");
    }

    #[test]
    fn any_key_dismisses_the_kline_banner() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.irc().trace = 99.0;
        r.type_line(&format!("/hack {bot}"));
        r.press(press_char('x'));
        assert_eq!(r.app.screen.name(), "Menu");
        assert_eq!(ego(&mut r), 80);
    }

    #[test]
    fn backing_off_into_a_kline_logs_the_retreat_before_the_kline() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.irc().trace = 90.0;
        r.type_line(&format!("/hack {bot}"));
        r.press(press(KeyCode::Esc));
        assert!(matches!(r.irc().mode, Mode::Kline { .. }));
        let last: Vec<String> = r
            .irc()
            .log
            .iter()
            .rev()
            .take(2)
            .map(|(_, l)| l.clone())
            .collect();
        assert_eq!(last[0], "*** K-LINED: the trace found you.");
        assert_eq!(last[1], format!("*** you back off {bot}."));
    }

    #[test]
    fn a_trace_spike_inside_the_game_also_klines() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.lock(1, IceKindAt::Daemon, IceGrade::Standard);
        r.irc().trace = 90.0;
        r.type_line(&format!("/hack {bot}"));
        for _ in 0..20 {
            if !matches!(r.irc().mode, Mode::Hack { .. }) {
                break;
            }
            r.type_line("zzz");
        }
        assert!(matches!(r.irc().mode, Mode::Kline { .. }));
        assert_eq!(ego(&mut r), 80);
        assert_eq!(r.irc().trace, 100.0);
    }

    #[test]
    fn teardrop_at_a_high_trace_still_crashes_the_bot_but_klines_you() {
        let mut r = rig();
        r.join_for(1);
        let bot = r.bot(1);
        r.ctx()
            .run
            .as_mut()
            .unwrap()
            .deck
            .insert(Program::Teardrop, 1);
        r.irc().trace = 90.0;
        r.type_line(&format!("/teardrop {bot}"));
        assert!(matches!(r.irc().mode, Mode::Kline { .. }));
        assert_eq!(ego(&mut r), 80);
        assert!(
            r.ctx()
                .run
                .as_ref()
                .unwrap()
                .irc
                .cracked
                .contains(&format!("0:{bot}"))
        );
    }

    #[test]
    fn downloads_and_cracks_are_saved() {
        let mut r = rig();
        let bot = r.bot(0);
        r.type_line(&format!("/fserv {bot}"));
        r.type_line("/get 3");
        r.join_for(1);
        let locked = r.bot(1);
        r.ctx()
            .run
            .as_mut()
            .unwrap()
            .deck
            .insert(Program::Teardrop, 1);
        r.type_line(&format!("/teardrop {locked}"));
        let live = r.ctx().run.as_ref().unwrap().irc.clone();
        assert_eq!((live.files.len(), live.cracked.len()), (1, 1));
        let saved = r.ctx().store.load("Tester").unwrap().unwrap();
        assert_eq!(saved.irc, live);
    }

    #[test]
    fn esc_clears_the_prompt_first_and_quit_leaves() {
        let mut r = rig();
        for c in "hello".chars() {
            r.press(press_char(c));
        }
        r.press(press(KeyCode::Esc));
        assert_eq!(r.app.screen.name(), "Irc");
        assert!(r.irc().input.is_empty());
        r.press(press(KeyCode::Esc));
        assert_eq!(r.app.screen.name(), "Menu");
        let mut r = rig();
        r.type_line("/quit");
        assert_eq!(r.app.screen.name(), "Menu");
    }

    #[test]
    fn unknown_commands_and_missing_arguments_say_so() {
        let mut r = rig();
        r.type_line("/frobnicate now");
        r.type_line("/hack");
        r.type_line("/join");
        r.type_line("/help");
        let log = r.log();
        assert!(
            log.contains("*** unknown command: /frobnicate (/help)"),
            "{log}"
        );
        assert!(log.contains("*** usage: /hack <bot>"), "{log}");
        assert!(log.contains("*** usage: /join #channel"), "{log}");
        assert!(log.contains("/teardrop <bot>"), "{log}");
        r.type_line("/READ 7");
        assert!(r.log().contains("*** no such file"));
    }

    #[test]
    fn a_third_fserv_appears_at_run_danger_three() {
        let mut r = rig_with(8, |_| {});
        assert_eq!(r.irc().run_danger, 3);
        let net = &r.irc().net;
        assert_eq!(net.fservs.len(), 3);
        assert!(net.channels.iter().all(|c| c.fservs.len() == 1));
        assert_eq!(net.fservs[2].channel, 2);
        let channel = net.channels[2].name.clone();
        r.type_line(&format!("/join {channel}"));
        let bot = r.bot(2);
        r.type_line(&format!("/fserv {bot}"));
        assert!(r.log().contains(&format!("<{bot}> ")));
    }

    #[test]
    fn without_a_run_the_screen_leaves_on_its_first_tick() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let screen = Screen::Irc(IrcScreen::new(&mut ctx));
        let mut app = App::with_screen(ctx, screen);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        app.step(None, FRAME);
        assert_ne!(app.screen.name(), "Irc");
    }

    #[test]
    fn a_level_without_its_place_is_an_orphan_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 0);
        ctx.run.as_mut().unwrap().level_mut().unwrap().place = None;
        let screen = IrcScreen::new(&mut ctx);
        assert!(screen.orphan);
    }

    /// The README's IRC gallery: five frames of one visit to the level-3 network, rendered
    /// through `App::render` and written as SVG next to the host screenshots (the host ones
    /// come from `write_readme_screenshots` in `run.rs`). Ignored by default; re-run both with
    /// `cargo test --lib write_readme_screenshots -- --ignored` when a screen changes.
    mod screenshots {
        use super::*;
        use crate::ui::shots;
        use std::path::{Path, PathBuf};

        /// The level whose second fserv is locked with PHREAK, the one game the host gallery has
        /// no frame of, and whose password (RAVENS) the `.NFO` half-reveals.
        const STORY_DEPTH: u32 = 3;

        /// Truecolor, and calm so the frames are deterministic.
        fn truecolor(ctx: &mut Ctx) {
            ctx.palette = Palette::new(true);
            ctx.config.display.calm = true;
        }

        fn story() -> Rig {
            let mut r = rig_with(STORY_DEPTH, truecolor);
            let lock = r.irc().net.fservs[1].lock.map(|l| l.ice);
            assert_eq!(
                lock,
                Some(IceKindAt::Phreak),
                "STORY_DEPTH must lock fserv 1 with PHREAK"
            );
            r
        }

        fn shot(dir: &Path, name: &str, r: &mut Rig) {
            let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
            term.draw(|f| r.app.render(f)).unwrap();
            shots::write(dir, name, term.backend().buffer());
        }

        #[test]
        #[ignore = "writes docs/screenshots; run on demand"]
        fn write_readme_screenshots() {
            let out = std::env::var_os("ABRAXAS_SHOTS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/screenshots"))
                });

            // Join, some chatter (every fourth line a bot advertising), an open fserv's stock
            // and the purchase of TEARDROP.SH.
            let mut r = story();
            let (open, locked) = (r.bot(0), r.bot(1));
            r.wait(10.0);
            r.type_line(&format!("/fserv {open}"));
            r.type_line("/get 1");
            assert!(r.log().contains("TEARDROP.SH from"), "{}", r.log());
            shot(&out, "irc", &mut r);

            // `/hack` on the locked fserv in the next channel: the PHREAK keypad, one tone lit,
            // the header's TRACE meter already at 5%.
            r.join_for(1);
            r.type_line(&format!("/fserv {locked}"));
            r.type_line(&format!("/hack {locked}"));
            r.wait(0.2);
            assert!(r.text().contains("PHREAK  RELAY LINE"), "{}", r.text());
            shot(&out, "irc-hack", &mut r);

            // Back off (TRACE 10%), then crash it with TEARDROP.SH (+15): the stock is free.
            r.press(press(KeyCode::Esc));
            r.type_line(&format!("/teardrop {locked}"));
            r.type_line(&format!("/fserv {locked}"));
            let t = r.text();
            assert!(t.contains("has quit (Connection reset by peer)"), "{t}");
            assert!(t.contains("cracked. /get <n> is free"), "{t}");
            shot(&out, "irc-teardrop", &mut r);

            // The cracked fserv hands over its `.NFO`: half of the front-door password.
            r.type_line("/get 2");
            r.type_line("/read 1");
            let t = r.text();
            assert!(t.contains("front door at") && t.contains("RAV___"), "{t}");
            shot(&out, "irc-nfo", &mut r);

            // Keep probing and the meter fills: K-LINED.
            let mut r = story();
            r.join_for(1);
            r.type_line(&format!("/fserv {locked}"));
            for _ in 0..10 {
                r.type_line(&format!("/hack {locked}"));
                r.press(press(KeyCode::Esc));
            }
            assert!(matches!(r.irc().mode, Mode::Kline { .. }));
            shot(&out, "irc-kline", &mut r);
        }
    }
}
