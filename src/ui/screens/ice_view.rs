//! Full-canvas views for the five ICE mini-games.
//!
//! [`IceView`] owns one running game, forwards keys and time to it and draws it into the 80x23
//! area below the Run screen's trace header. The games themselves are pure logic (`crate::ice`);
//! this module only translates input, drains their side effects (trace, EGO) for the Run screen to
//! apply and renders their state.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::game::danger::{DangerParams, IceGrade};
use crate::game::programs::Program;
use crate::game::shell::wrap_lines;
use crate::ice::breach::{Axis, Breach, DaemonKind};
use crate::ice::daemon;
use crate::ice::duel::{BlackClass, Duel, DuelKind, Intent, REGEN_PCT};
use crate::ice::phreak::{self, Phreak};
use crate::ice::router::Router;
use crate::ice::wordlock::{self, Wordlock};
use crate::ice::{IceGame, IceInput, IceOutcome, Loadout};
use crate::pack::model::HostOs;
use crate::palette::{Dos, Palette};
use crate::rng::Rng;
use crate::ui::canvas::put_str;
use crate::ui::keys::Key;
use crate::ui::widgets::progress;

const ART_WOLF: &str = include_str!("../../../assets/ice-wolf.txt");
const ART_EYE: &str = include_str!("../../../assets/ice-eye.txt");
const ART_HYDRA: &str = include_str!("../../../assets/ice-hydra.txt");

/// Stand-in Shadow figure when the pack ships no art for it.
const SHADOW_FALLBACK: &str =
    "   ___\n  /   \\\n |  .  |\n  \\___/\n /|   |\\\n/ |   | \\\n  |   |\n  /   \\";

/// Rows of the area the views draw into.
pub const VIEW_ROWS: u16 = 23;

/// A styled art cell.
type ArtRow = Vec<(char, Style)>;

/// Map a key press to the input the games take. Space is `IceInput::Space`; anything the games
/// never use yields `None`.
pub fn ice_input(key: Key) -> Option<IceInput> {
    match key {
        Key::Up => Some(IceInput::Up),
        Key::Down => Some(IceInput::Down),
        Key::Left => Some(IceInput::Left),
        Key::Right => Some(IceInput::Right),
        Key::Enter => Some(IceInput::Enter),
        Key::Esc => Some(IceInput::Esc),
        Key::Tab => Some(IceInput::Tab),
        Key::Backspace => Some(IceInput::Backspace),
        Key::Char(' ') => Some(IceInput::Space),
        Key::Char(c) => Some(IceInput::Char(c)),
        _ => None,
    }
}

#[derive(Debug)]
enum Game {
    Wordlock(Box<Wordlock>),
    Breach(Box<Breach>),
    Phreak(Box<Phreak>),
    Router(Box<Router>),
    Daemon(Box<daemon::Daemon>),
    Duel(Box<Duel>, Vec<ArtRow>),
}

/// How long a duel attack animation runs.
const ANIM_SECS: f32 = 0.9;

/// One running duel attack animation (pure display state, advanced in `tick`).
#[derive(Debug, Clone, Copy)]
struct AttackAnim {
    intent: Intent,
    reflected: bool,
    t: f32,
}

/// How a duel's opponent art is drawn: the palette and the pack's Shadow art (mirrored).
#[derive(Debug, Clone, Copy)]
pub struct DuelArt<'a> {
    pub palette: &'a Palette,
    pub shadow_art: Option<&'a str>,
}

/// One running mini-game and its view.
#[derive(Debug)]
pub struct IceView {
    game: Game,
    /// Charges already reported through `take_spent`.
    accounted: Loadout,
    ego_seen: i32,
    /// The duel attack animation in flight, if any.
    anim: Option<AttackAnim>,
    /// `--calm`: no animation runs and none is drawn.
    calm: bool,
}

impl IceView {
    /// A WORDLOCK board (`words` and `password` from the level's place).
    pub fn wordlock(
        rng: &mut Rng,
        params: &DangerParams,
        words: &[String],
        password: &str,
        loadout: Loadout,
        grade: IceGrade,
    ) -> Self {
        let game = Wordlock::generate(rng, params, words, password, 0, loadout, grade);
        Self::with(Game::Wordlock(Box::new(game)), loadout)
    }

    pub fn breach(rng: &mut Rng, params: &DangerParams, loadout: Loadout) -> Self {
        let game = Breach::generate(rng, params, loadout);
        Self::with(Game::Breach(Box::new(game)), loadout)
    }

    pub fn phreak(rng: &mut Rng, params: &DangerParams, loadout: Loadout) -> Self {
        let game = Phreak::generate(rng, params, loadout);
        Self::with(Game::Phreak(Box::new(game)), loadout)
    }

    pub fn router(rng: &mut Rng, params: &DangerParams) -> Self {
        let game = Router::generate(rng, params);
        Self::with(Game::Router(Box::new(game)), Loadout::default())
    }

    /// A DAEMON console on the host's OS (`grade` picks the card length, clock and budget).
    pub fn daemon(rng: &mut Rng, os: HostOs, grade: IceGrade) -> Self {
        let game = daemon::Daemon::generate(rng, os, grade);
        Self::with(Game::Daemon(Box::new(game)), Loadout::default())
    }

    /// A duel. `art.palette` colours the pack's pipe-coded Shadow art (mirrored for the
    /// opponent); `class` names the black ICE creature (ignored for the Shadow).
    pub fn duel(
        rng: &mut Rng,
        params: &DangerParams,
        kind: DuelKind,
        loadout: Loadout,
        ego: i32,
        art: DuelArt,
        class: BlackClass,
    ) -> Self {
        let game = Duel::generate(rng, params, kind, loadout, ego, class);
        let rendered = duel_art(game.class(), art.shadow_art, art.palette);
        Self::with(Game::Duel(Box::new(game), rendered), loadout)
    }

    fn with(game: Game, start: Loadout) -> Self {
        Self {
            game,
            accounted: start,
            ego_seen: 0,
            anim: None,
            calm: false,
        }
    }

    /// `--calm` mode: skip (and never start) attack animations.
    pub fn set_calm(&mut self, calm: bool) {
        self.calm = calm;
    }

    /// True for the ROUTER (`bounce`) puzzle, which reports through `apply_bounce_outcome`.
    pub fn is_router(&self) -> bool {
        matches!(self.game, Game::Router(_))
    }

    pub fn handle(&mut self, input: IceInput) {
        match &mut self.game {
            Game::Wordlock(g) => g.handle(input),
            Game::Breach(g) => g.handle(input),
            Game::Phreak(g) => g.handle(input),
            Game::Router(g) => g.handle(input),
            Game::Daemon(g) => g.handle(input),
            Game::Duel(g, _) => {
                g.handle(input);
                // Drain every turn (calm mode included) so the game's hit list never grows.
                let hit = g.take_hits().pop();
                if !self.calm
                    && let Some(hit) = hit
                {
                    self.anim = Some(AttackAnim {
                        intent: hit.intent,
                        reflected: hit.reflected,
                        t: 0.0,
                    });
                }
            }
        }
    }

    pub fn tick(&mut self, dt: Duration) {
        match &mut self.game {
            Game::Wordlock(g) => g.tick(dt),
            Game::Breach(g) => g.tick(dt),
            Game::Phreak(g) => g.tick(dt),
            Game::Router(g) => g.tick(dt),
            Game::Daemon(g) => g.tick(dt),
            Game::Duel(g, _) => g.tick(dt),
        }
        if let Some(anim) = &mut self.anim {
            anim.t += dt.as_secs_f32();
            if anim.t >= ANIM_SECS {
                self.anim = None;
            }
        }
    }

    pub fn outcome(&self) -> Option<IceOutcome> {
        match &self.game {
            Game::Wordlock(g) => g.outcome(),
            Game::Breach(g) => g.outcome(),
            Game::Phreak(g) => g.outcome(),
            Game::Router(g) => g.outcome(),
            Game::Daemon(g) => g.outcome(),
            Game::Duel(g, _) => g.outcome(),
        }
    }

    /// Trace penalties incurred since the last call. A duel retreat is not drained: its
    /// `Retreated` outcome already carries the penalty when it is applied.
    pub fn take_trace(&mut self) -> f32 {
        let retreated = self.outcome() == Some(IceOutcome::Retreated);
        let trace = match &mut self.game {
            Game::Wordlock(g) => g.take_trace(),
            Game::Phreak(g) => g.take_trace(),
            Game::Daemon(g) => g.take_trace(),
            Game::Duel(g, _) => g.take_trace(),
            Game::Breach(_) | Game::Router(_) => 0.0,
        };
        if retreated { 0.0 } else { trace }
    }

    /// EGO lost in a duel since the last call (zero or negative).
    pub fn take_ego(&mut self) -> i32 {
        let Game::Duel(g, _) = &self.game else {
            return 0;
        };
        let total = g.ego_delta();
        let fresh = total - self.ego_seen;
        self.ego_seen = total;
        fresh
    }

    /// `(KB, trace delta)` earned from optional BREACH daemons.
    pub fn rewards(&self) -> (u32, f32) {
        match &self.game {
            Game::Breach(g) => g.rewards(),
            _ => (0, 0.0),
        }
    }

    /// Program charges the game consumed since the last call.
    pub fn take_spent(&mut self) -> Vec<(Program, u8)> {
        let now = match &self.game {
            Game::Wordlock(g) => g.loadout(),
            Game::Duel(g, _) => g.loadout(),
            Game::Breach(g) => g.loadout(),
            Game::Phreak(g) => g.loadout(),
            Game::Router(_) | Game::Daemon(_) => self.accounted,
        };
        let before = self.accounted;
        self.accounted = now;
        [
            (Program::Decrypt, before.decrypt, now.decrypt),
            (Program::Sniffer, before.sniffer, now.sniffer),
            (Program::Mirror, before.mirror, now.mirror),
        ]
        .into_iter()
        .filter_map(|(p, was, is)| {
            let used = was.saturating_sub(is);
            (used > 0).then_some((p, used))
        })
        .collect()
    }

    /// The BREACH board, for tests that solve it.
    #[cfg(test)]
    pub(crate) fn breach_game(&self) -> Option<&Breach> {
        match &self.game {
            Game::Breach(g) => Some(g),
            _ => None,
        }
    }

    /// The WORDLOCK board, for tests that need to read its layout.
    #[cfg(test)]
    pub(crate) fn wordlock_game(&self) -> Option<&Wordlock> {
        match &self.game {
            Game::Wordlock(g) => Some(g),
            _ => None,
        }
    }

    /// The running duel, for tests that force an intent.
    #[cfg(test)]
    pub(crate) fn duel_mut(&mut self) -> Option<&mut Duel> {
        match &mut self.game {
            Game::Duel(g, _) => Some(g),
            _ => None,
        }
    }

    /// The DAEMON console, for tests that read its solution.
    #[cfg(test)]
    pub(crate) fn daemon_game(&self) -> Option<&daemon::Daemon> {
        match &self.game {
            Game::Daemon(g) => Some(g),
            _ => None,
        }
    }

    /// Draw into `area` (80 columns, [`VIEW_ROWS`] rows).
    pub fn render(&self, buf: &mut Buffer, area: Rect, p: &Palette) {
        match &self.game {
            Game::Wordlock(g) => render_wordlock(g, buf, area, p),
            Game::Breach(g) => render_breach(g, buf, area, p),
            Game::Phreak(g) => render_phreak(g, buf, area, p),
            Game::Router(g) => render_router(g, buf, area, p),
            Game::Daemon(g) => render_daemon(g, buf, area, p),
            Game::Duel(g, art) => render_duel(g, art, self.anim.as_ref(), buf, area, p),
        }
    }

    /// True when this game wants raw typed input (the Run screen then keeps `?` as a
    /// character instead of opening Help).
    pub fn wants_text(&self) -> bool {
        matches!(self.game, Game::Daemon(_))
    }
}

// ---------------------------------------------------------------------------------------------
// Shared drawing helpers
// ---------------------------------------------------------------------------------------------

fn fg(p: &Palette, c: Dos) -> Style {
    p.style(c, Dos::Black)
}

/// `─[ TITLE ]───...` across the first row of the area.
fn title_rule(buf: &mut Buffer, area: Rect, title: &str, p: &Palette) {
    let rule = fg(p, Dos::DarkGray);
    put_str(
        buf,
        area.x,
        area.y,
        &"─".repeat(usize::from(area.width)),
        area.width,
        rule,
    );
    put_str(buf, area.x + 2, area.y, "[ ", 2, rule);
    let used = put_str(
        buf,
        area.x + 4,
        area.y,
        title,
        area.width - 4,
        fg(p, Dos::LightCyan),
    );
    put_str(buf, area.x + 4 + used, area.y, " ]", 2, rule);
}

fn put_cell(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_char(ch);
        c.set_style(style);
    }
}

/// Clamp a count into `u16` for coordinates.
fn u(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

fn footer(buf: &mut Buffer, area: Rect, text: &str, p: &Palette) {
    put_str(
        buf,
        area.x + 1,
        area.y + area.height - 2,
        text,
        area.width - 1,
        fg(p, Dos::LightGray),
    );
}

// ---------------------------------------------------------------------------------------------
// WORDLOCK
// ---------------------------------------------------------------------------------------------

fn render_wordlock(g: &Wordlock, buf: &mut Buffer, area: Rect, p: &Palette) {
    title_rule(buf, area, "WORDLOCK", p);
    put_str(
        buf,
        area.x + 2,
        area.y + 1,
        "ENTER PASSWORD NOW",
        30,
        fg(p, Dos::White),
    );
    let mut left = String::from("ATTEMPT(S) LEFT:");
    for i in 0..g.max_attempts {
        left.push(' ');
        left.push(if i < g.attempts { '■' } else { ' ' });
    }
    put_str(
        buf,
        area.x + 2,
        area.y + 2,
        &left,
        40,
        fg(p, Dos::LightGreen),
    );

    let group = g.highlight();
    let cursor = g.cursor;
    let letters = fg(p, Dos::White);
    let junk = fg(p, Dos::LightGray);
    let tried = fg(p, Dos::Brown);
    let in_group = p.style(Dos::Black, Dos::Green);
    let on_cursor_group = p.style(Dos::Black, Dos::LightGreen);
    let on_cursor = p.style(Dos::Black, Dos::LightGray);
    for col in 0..2usize {
        let x = area.x + 2 + u(col) * 22;
        for row in 0..wordlock::ROWS {
            let line = col * wordlock::ROWS + row;
            let y = area.y + 4 + u(row);
            put_str(
                buf,
                x,
                y,
                &format!("0x{:04X}", Wordlock::address(line)),
                6,
                fg(p, Dos::DarkGray),
            );
            for (i, &ch) in g.lines[line].iter().enumerate() {
                let is_tried = g.words.iter().any(|w| {
                    w.tried
                        && w.line == line
                        && i >= w.start
                        && i < w.start + w.text.chars().count()
                });
                let grouped =
                    group.is_some_and(|h| h.line == line && i >= h.start && i < h.start + h.len);
                let at_cursor = cursor.line() == line && cursor.ch == i;
                let style = match (at_cursor, grouped) {
                    (true, true) => on_cursor_group,
                    (true, false) => on_cursor,
                    (false, true) => in_group,
                    _ if is_tried => tried,
                    _ if ch.is_alphabetic() => letters,
                    _ => junk,
                };
                put_cell(buf, x + 7 + u(i), y, ch, style);
            }
        }
    }

    let lx = area.x + 48;
    if g.locked() {
        put_str(
            buf,
            lx,
            area.y + 4,
            "TERMINAL LOCKED",
            31,
            fg(p, Dos::LightRed),
        );
        put_str(
            buf,
            lx,
            area.y + 5,
            "REBOOTING TERMINAL...",
            31,
            fg(p, Dos::DarkGray),
        );
    } else {
        let shown = g.log.len().saturating_sub(wordlock::ROWS);
        for (i, line) in g.log[shown..].iter().enumerate() {
            put_str(buf, lx, area.y + 4 + u(i), line, 31, fg(p, Dos::LightGreen));
        }
    }

    let l = g.loadout();
    let dim = |n: u8| {
        if n == 0 {
            Dos::DarkGray
        } else {
            Dos::LightGray
        }
    };
    let y = area.y + area.height - 2;
    let mut x = area.x + 1;
    for (text, color) in [
        (
            "[arrows] move  [Enter] select  ".to_string(),
            Dos::LightGray,
        ),
        (format!("[1] DECRYPT x{}  ", l.decrypt), dim(l.decrypt)),
        (format!("[2] SNIFFER x{}  ", l.sniffer), dim(l.sniffer)),
        ("[Esc] abort".to_string(), Dos::LightGray),
    ] {
        x += put_str(buf, x, y, &text, area.x + area.width - x, fg(p, color));
    }
}

// ---------------------------------------------------------------------------------------------
// BREACH
// ---------------------------------------------------------------------------------------------

/// How many leading codes of `seq` the buffer currently ends with (the whole sequence once it
/// appears anywhere in the buffer).
fn daemon_progress(seq: &[u8], buffer: &[u8]) -> usize {
    if seq.is_empty() {
        return 0;
    }
    if buffer.windows(seq.len()).any(|w| w == seq) {
        return seq.len();
    }
    (1..seq.len())
        .rev()
        .find(|&k| buffer.ends_with(&seq[..k]))
        .unwrap_or(0)
}

fn render_breach(g: &Breach, buf: &mut Buffer, area: Rect, p: &Palette) {
    title_rule(buf, area, "BREACH PROTOCOL", p);
    if g.started {
        let secs = g.time_left().as_secs_f32();
        let color = if secs <= 5.0 {
            Dos::LightRed
        } else if secs <= 10.0 {
            Dos::Yellow
        } else {
            Dos::White
        };
        put_str(
            buf,
            area.x + 2,
            area.y + 1,
            &format!("TIME {secs:>5.1}"),
            14,
            fg(p, color),
        );
    } else {
        put_str(
            buf,
            area.x + 2,
            area.y + 1,
            "TIMER STARTS AT YOUR FIRST PICK",
            40,
            fg(p, Dos::DarkGray),
        );
    }

    // Buffer slots.
    put_str(
        buf,
        area.x + 2,
        area.y + 3,
        "BUFFER",
        6,
        fg(p, Dos::LightGray),
    );
    let codes = g.buffer_codes();
    for i in 0..g.buffer {
        let x = area.x + 9 + u(i) * 4;
        match codes.get(i) {
            Some(&c) => {
                let text = format!("[{}]", crate::ice::breach::CODES[usize::from(c)]);
                put_str(buf, x, area.y + 3, &text, 4, fg(p, Dos::LightGreen));
            }
            None => {
                put_str(buf, x, area.y + 3, "[  ]", 4, fg(p, Dos::DarkGray));
            }
        }
    }
    let (axis_name, axis_no) = match g.axis {
        Axis::Row(r) => ("ROW", r + 1),
        Axis::Column(c) => ("COLUMN", c + 1),
    };
    put_str(
        buf,
        area.x + 2,
        area.y + 5,
        &format!("PICK FROM {axis_name} {axis_no}"),
        30,
        fg(p, Dos::Yellow),
    );

    // Grid.
    let active = p.style(Dos::White, Dos::Blue);
    let used = fg(p, Dos::DarkGray);
    let plain = fg(p, Dos::LightGray);
    let cursor = p.style(Dos::Black, Dos::LightCyan);
    for row in 0..g.size {
        for col in 0..g.size {
            let in_line = match g.axis {
                Axis::Row(r) => row == r,
                Axis::Column(c) => col == c,
            };
            let style = if g.cursor == (row, col) {
                cursor
            } else if g.is_used(row, col) {
                used
            } else if in_line {
                active
            } else {
                plain
            };
            let x = area.x + 3 + u(col) * 5;
            let y = area.y + 7 + u(row) * 2;
            put_str(buf, x, y, &format!(" {} ", g.code(row, col)), 4, style);
        }
    }

    // Daemons.
    let dx = area.x + 46;
    put_str(buf, dx, area.y + 3, "DAEMONS", 30, fg(p, Dos::LightCyan));
    for (i, d) in g.daemons.iter().enumerate() {
        let y = area.y + 5 + u(i) * 3;
        let done = g.completed(i);
        let name_color = if done { Dos::LightGreen } else { Dos::White };
        put_str(buf, dx, y, d.name, 30, fg(p, name_color));
        let progress = daemon_progress(&d.seq, &codes);
        let mut x = dx;
        for (k, &c) in d.seq.iter().enumerate() {
            let color = if k < progress {
                Dos::LightGreen
            } else {
                Dos::LightGray
            };
            x += put_str(
                buf,
                x,
                y + 1,
                &format!("{} ", crate::ice::breach::CODES[usize::from(c)]),
                3,
                fg(p, color),
            );
        }
        let effect = match d.kind {
            DaemonKind::Required => "clears the ICE".to_string(),
            DaemonKind::DataMine => format!("+{}KB", crate::ice::breach::DATAMINE_KB),
            DaemonKind::TraceWipe => format!("trace {}", crate::ice::breach::TRACE_WIPE_DELTA),
        };
        put_str(buf, x + 1, y + 1, &effect, 20, fg(p, Dos::DarkGray));
    }
    footer(
        buf,
        area,
        "[arrows] move  [Enter/Space] pick  [Esc] abort",
        p,
    );
}

// ---------------------------------------------------------------------------------------------
// PHREAK
// ---------------------------------------------------------------------------------------------

fn render_phreak(g: &Phreak, buf: &mut Buffer, area: Rect, p: &Palette) {
    title_rule(buf, area, "PHREAK  RELAY LINE", p);
    let lit = g.lit();
    for (i, &key) in phreak::KEYS.iter().enumerate() {
        let x = area.x + 6 + u(i % 3) * 6;
        let y = area.y + 3 + u(i / 3) * 3;
        let on = lit == Some(key);
        let border = if on {
            p.style(Dos::Black, Dos::Yellow)
        } else {
            fg(p, Dos::LightGray)
        };
        let face = if on { border } else { fg(p, Dos::White) };
        put_str(buf, x, y, "┌───┐", 5, border);
        put_str(buf, x, y + 1, "│   │", 5, border);
        put_str(buf, x, y + 2, "└───┘", 5, border);
        put_cell(buf, x + 2, y + 1, key, face);
    }

    let rx = area.x + 32;
    put_str(
        buf,
        rx,
        area.y + 3,
        &format!("ROUND {}/{}", g.round, g.rounds),
        20,
        fg(p, Dos::White),
    );
    put_str(
        buf,
        rx,
        area.y + 4,
        &format!("{} tones", g.round_len(g.round)),
        20,
        fg(p, Dos::LightGray),
    );
    let (state, color) = match g.phase {
        phreak::Phase::Flashing => ("WATCH THE LINE", Dos::Yellow),
        phreak::Phase::Input => ("REPEAT THE SEQUENCE", Dos::LightGreen),
    };
    put_str(buf, rx, area.y + 6, state, 30, fg(p, color));
    if let Some((lo, hi)) = lit.and_then(phreak::dtmf) {
        put_str(
            buf,
            rx,
            area.y + 8,
            &format!("{} Hz + {hi} Hz", lo),
            30,
            fg(p, Dos::LightCyan),
        );
    }
    let expected = g.round_len(g.round);
    let mut dots = String::new();
    for i in 0..expected {
        dots.push(if i < g.entered.len() { '•' } else { '○' });
        dots.push(' ');
    }
    put_str(buf, rx, area.y + 10, &dots, 40, fg(p, Dos::White));
    let typed: String = g.entered.iter().flat_map(|&c| [c, ' ']).collect();
    put_str(
        buf,
        rx,
        area.y + 11,
        &format!("YOU: {typed}"),
        40,
        fg(p, Dos::LightGray),
    );
    put_str(
        buf,
        rx,
        area.y + 13,
        &format!("ATTEMPT {}", g.attempt + 1),
        20,
        fg(p, Dos::DarkGray),
    );
    if g.loadout().metronom {
        let c = if g.can_replay() {
            Dos::LightGray
        } else {
            Dos::DarkGray
        };
        put_str(
            buf,
            rx,
            area.y + 15,
            "[R] REPLAY (METRONOM.EXE)",
            30,
            fg(p, c),
        );
    }
    footer(buf, area, "[0-9 * #] key  [Esc] abort", p);
}

// ---------------------------------------------------------------------------------------------
// ROUTER
// ---------------------------------------------------------------------------------------------

fn render_router(g: &Router, buf: &mut Buffer, area: Rect, p: &Palette) {
    title_rule(buf, area, "ROUTER  BOUNCE THE TRACE", p);
    let secs = g.time_left().as_secs_f32();
    put_str(
        buf,
        area.x + 2,
        area.y + 1,
        &format!("TIME {secs:>4.1}"),
        14,
        fg(
            p,
            if secs <= 10.0 {
                Dos::LightRed
            } else {
                Dos::White
            },
        ),
    );
    let gx = area.x + 8;
    let gy = area.y + 3;
    let connected = fg(p, Dos::LightGreen);
    let loose = fg(p, Dos::LightGray);
    let cursor = p.style(Dos::Black, Dos::LightCyan);
    for (r, line) in g.render_rows().iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            let (tx, ty) = (c / 3, r / 3);
            let style = if g.cursor == (tx, ty) {
                cursor
            } else if g.connected(tx, ty) {
                connected
            } else {
                loose
            };
            put_cell(buf, gx + u(c), gy + u(r), ch, style);
        }
    }
    let mark = fg(p, Dos::Yellow);
    put_str(buf, gx - 5, gy + u(g.entry_row) * 3 + 1, "IN ►", 5, mark);
    put_str(
        buf,
        gx + u(g.width) * 3,
        gy + u(g.exit_row) * 3 + 1,
        "► OUT",
        6,
        mark,
    );

    let rx = area.x + 46;
    put_str(
        buf,
        rx,
        area.y + 3,
        "Route the line from IN to OUT.",
        33,
        fg(p, Dos::LightGray),
    );
    put_str(
        buf,
        rx,
        area.y + 4,
        "Connected pipes light up.",
        33,
        fg(p, Dos::LightGray),
    );
    put_str(
        buf,
        rx,
        area.y + 6,
        "Solved: trace -30.",
        33,
        fg(p, Dos::LightGreen),
    );
    put_str(
        buf,
        rx,
        area.y + 7,
        "Time out: trace +10.",
        33,
        fg(p, Dos::LightRed),
    );
    if g.solved() {
        put_str(
            buf,
            rx,
            area.y + 9,
            "LINK ESTABLISHED",
            33,
            fg(p, Dos::LightGreen),
        );
    }
    footer(
        buf,
        area,
        "[arrows] move  [Space/Enter] rotate  [Esc] cancel",
        p,
    );
}

// ---------------------------------------------------------------------------------------------
// DUEL
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Duel attack animations
// ---------------------------------------------------------------------------------------------

/// Animation progress 0..=1.
fn anim_progress(a: &AttackAnim) -> f32 {
    (a.t / ANIM_SECS).clamp(0.0, 1.0)
}

/// Horizontal art offset for a lunge or a bite-strike shake.
fn anim_offset(a: &AttackAnim) -> u16 {
    let p = anim_progress(a);
    let phase = std::f32::consts::PI * p;
    match a.intent {
        Intent::Lunge => (8.0 * phase.sin()).round() as u16,
        Intent::Strike | Intent::Probe => (4.0 * phase.sin()).round() as u16,
        _ => 0,
    }
}

/// Overlay cells for the current animation, in view-area coordinates (x, y, char, color).
/// `art_w`/`art_rows` are the art box dimensions; the status panel starts at x = 44.
fn anim_cells(a: &AttackAnim, art_w: u16, art_rows: u16) -> Vec<(u16, u16, char, Dos)> {
    let p = anim_progress(a);
    let art_x = 2 + art_w;
    let span = (42u16).saturating_sub(art_w).max(1);
    match a.intent {
        Intent::Gaze => {
            // A reflected gaze goes out, then comes back at the art in the mirror's colour.
            let (reach, returning) = match (a.reflected, p < 0.5) {
                (true, true) => (2.0 * p, false),
                (true, false) => (2.0 * (1.0 - p), true),
                (false, _) => (p, false),
            };
            let end = art_x + (f32::from(span) * reach).round() as u16;
            if end <= art_x {
                return Vec::new();
            }
            let (ch, color, head) = if returning {
                ('\u{2550}', Dos::LightCyan, '\u{25C4}')
            } else {
                ('\u{2550}', Dos::LightMagenta, '\u{25BA}')
            };
            let row = 2 + art_rows / 2;
            let mut cells: Vec<(u16, u16, char, Dos)> =
                (art_x + 1..end).map(|x| (x, row, ch, color)).collect();
            cells.push((end, row, head, color));
            cells
        }
        Intent::Bite => {
            let mut cells = Vec::new();
            for j in 0..3u16 {
                let pj = ((p - 0.15 * f32::from(j)) / 0.55).clamp(0.0, 1.0);
                if pj <= 0.0 {
                    continue;
                }
                let row = 2 + art_rows * (2 * j + 1) / 6;
                let end = art_x + (f32::from(span) * pj).round() as u16;
                cells.extend((art_x + 1..end).map(|x| (x, row, '~', Dos::LightGreen)));
                cells.push((end, row, '\u{00BB}', Dos::Yellow));
            }
            cells
        }
        Intent::Crush => {
            let x = 2 + (44.0 * p).round() as u16;
            (2..2 + art_rows)
                .map(|y| (x.min(43), y, '\u{2588}', Dos::LightRed))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Green tint over the art while a head regrows.
fn anim_tint(a: &AttackAnim) -> Option<Dos> {
    (a.intent == Intent::Regrow && anim_progress(a) < 0.5).then_some(Dos::LightGreen)
}

/// True while the EGO row should flash (the back half of a damaging hit).
fn anim_ego_flash(a: &AttackAnim) -> bool {
    let damaging = matches!(
        a.intent,
        Intent::Probe
            | Intent::Strike
            | Intent::Crush
            | Intent::Lunge
            | Intent::Gaze
            | Intent::Bite
            | Intent::Mirror
    );
    damaging && !a.reflected && (0.55..=0.95).contains(&anim_progress(a))
}

fn mirror_char(c: char) -> char {
    match c {
        '/' => '\\',
        '\\' => '/',
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '▌' => '▐',
        '▐' => '▌',
        '◄' => '►',
        '►' => '◄',
        '┌' => '┐',
        '┐' => '┌',
        '└' => '┘',
        '┘' => '└',
        '├' => '┤',
        '┤' => '├',
        '╔' => '╗',
        '╗' => '╔',
        '╚' => '╝',
        '╝' => '╚',
        '╠' => '╣',
        '╣' => '╠',
        other => other,
    }
}

/// The opponent's artwork as styled cells: black ICE art in red, the Shadow's art mirrored
/// horizontally (every row padded to the widest first so it mirrors as a block).
fn duel_art(avatar: Option<BlackClass>, shadow_art: Option<&str>, p: &Palette) -> Vec<ArtRow> {
    match avatar {
        Some(a) => {
            let text = match a {
                BlackClass::Wolf => ART_WOLF,
                BlackClass::Eye => ART_EYE,
                BlackClass::Hydra => ART_HYDRA,
            };
            let style = fg(p, Dos::LightRed);
            text.lines()
                .map(|l| l.chars().map(|c| (c, style)).collect())
                .collect()
        }
        None => {
            let blank = fg(p, Dos::LightGray);
            let mut rows: Vec<ArtRow> = p
                .pipe_lines(shadow_art.unwrap_or(SHADOW_FALLBACK))
                .into_iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
                        .collect()
                })
                .collect();
            let width = rows.iter().map(Vec::len).max().unwrap_or(0);
            for row in &mut rows {
                row.resize(width, (' ', blank));
                row.reverse();
                for cell in row.iter_mut() {
                    cell.0 = mirror_char(cell.0);
                }
            }
            rows
        }
    }
}

/// The last `rows` wrapped rows of a log, oldest first. Walks the log from the newest entry
/// and stops once enough rows are collected, so a full log is not re-wrapped every frame.
fn wrap_tail<'a, K: Copy>(
    items: impl DoubleEndedIterator<Item = (K, &'a str)>,
    width: usize,
    rows: usize,
) -> Vec<(K, String)> {
    let mut out: Vec<(K, String)> = Vec::with_capacity(rows + 2);
    for (kind, text) in items.rev() {
        out.extend(wrap_lines(text, width).into_iter().rev().map(|l| (kind, l)));
        if out.len() >= rows {
            break;
        }
    }
    out.truncate(rows);
    out.reverse();
    out
}

/// The DAEMON console: title, briefing, status row, a scrolling log and the prompt line.
fn render_daemon(g: &daemon::Daemon, buf: &mut Buffer, area: Rect, p: &Palette) {
    title_rule(buf, area, &format!("DAEMON  {} CONSOLE", g.os_name()), p);
    for (i, line) in wrap_lines(&g.brief, 76).into_iter().take(2).enumerate() {
        put_str(
            buf,
            area.x + 2,
            area.y + 1 + u(i),
            &line,
            76,
            fg(p, Dos::LightCyan),
        );
    }
    let secs = g.time_left().as_secs();
    let time_color = if secs < 15 {
        Dos::LightRed
    } else {
        Dos::Yellow
    };
    put_str(
        buf,
        area.x + 2,
        area.y + 3,
        &format!(
            "STEP {}/{}   WRONG {}/{}   HINTS {}",
            g.step_no(),
            g.steps_total(),
            g.wrong,
            g.wrong_max,
            g.hints_left
        ),
        50,
        fg(p, Dos::Yellow),
    );
    put_str(
        buf,
        area.x + 52,
        area.y + 3,
        &format!("TIME {secs}s"),
        12,
        fg(p, time_color),
    );
    let rows = 15usize;
    let wrapped = wrap_tail(g.log.iter().map(|(k, l)| (*k, l.as_str())), 76, rows);
    for (i, (kind, line)) in wrapped.iter().enumerate() {
        let style = match kind {
            daemon::LogKind::Echo => fg(p, Dos::LightGreen),
            daemon::LogKind::Output => fg(p, Dos::LightGray),
            daemon::LogKind::Error => fg(p, Dos::LightRed),
            daemon::LogKind::Hint => fg(p, Dos::LightCyan),
        };
        put_str(buf, area.x + 2, area.y + 5 + u(i), line, 76, style);
    }
    let prompt = format!("{}{}\u{2588}", g.prompt(), g.input);
    put_str(buf, area.x + 2, area.y + 20, &prompt, 76, fg(p, Dos::White));
    footer(
        buf,
        area,
        "[Enter] run   [Tab] hint (+4 trace)   [Esc] abort",
        p,
    );
}

fn intent_text(g: &Duel, intent: Intent) -> String {
    match intent {
        Intent::Charge => "CHARGE (gathering power)".to_string(),
        Intent::Stalk => "STALK (circling)".to_string(),
        Intent::Regrow => format!("REGROW (heals {})", g.max_strength * REGEN_PCT / 100),
        Intent::Reach => "REACH (it holds out a hand)".to_string(),
        other => format!("{} ({})", other.label(), g.intent_damage(other)),
    }
}

fn render_duel(
    g: &Duel,
    art: &[ArtRow],
    anim: Option<&AttackAnim>,
    buf: &mut Buffer,
    area: Rect,
    p: &Palette,
) {
    let shadow = g.kind == DuelKind::Shadow;
    let title = if shadow {
        "DUEL  YOUR SHADOW".to_string()
    } else {
        format!(
            "DUEL  BLACK ICE  {}",
            g.class().unwrap_or(BlackClass::Wolf).label()
        )
    };
    title_rule(buf, area, &title, p);
    let art_w = (art.iter().map(Vec::len).max().unwrap_or(0) as u16).min(40);
    let art_rows = (art.len() as u16).min(area.height.saturating_sub(4));
    let offset = anim.map(anim_offset).unwrap_or(0);
    let tint = anim.and_then(anim_tint);
    for (r, row) in art.iter().take(usize::from(area.height) - 4).enumerate() {
        for (c, &(ch, style)) in row.iter().take(40).enumerate() {
            if ch != ' ' {
                let x = area.x + 2 + u(c) + offset;
                if x < area.x + 44 {
                    let style = match tint {
                        Some(color) => fg(p, color),
                        None => style,
                    };
                    put_cell(buf, x, area.y + 2 + u(r), ch, style);
                }
            }
        }
    }
    if let Some(a) = anim {
        for (dx, dy, ch, color) in anim_cells(a, art_w, art_rows) {
            if area.x + dx < area.x + 44 {
                put_cell(buf, area.x + dx, area.y + dy, ch, fg(p, color));
            }
        }
    }

    let rx = area.x + 44;
    let bar = |frac: f32, color: Dos| (progress::bar(frac, 18), fg(p, color));
    let frac = |n: i32, max: i32| {
        if max <= 0 {
            0.0
        } else {
            n.max(0) as f32 / max as f32
        }
    };
    put_str(
        buf,
        rx,
        area.y + 2,
        if shadow { "SHADOW" } else { "ICE" },
        7,
        fg(p, Dos::LightRed),
    );
    let (b, s) = bar(frac(g.strength, g.max_strength), Dos::LightRed);
    let x = rx + 7 + put_str(buf, rx + 7, area.y + 2, &b, 18, s);
    put_str(
        buf,
        x + 1,
        area.y + 2,
        &format!("{}/{}", g.strength.max(0), g.max_strength),
        8,
        s,
    );
    put_str(buf, rx, area.y + 3, "EGO", 7, fg(p, Dos::LightGreen));
    let (b, s) = bar(frac(g.ego, 100), Dos::LightGreen);
    let x = rx + 7 + put_str(buf, rx + 7, area.y + 3, &b, 18, s);
    put_str(buf, x + 1, area.y + 3, &format!("{}", g.ego.max(0)), 4, s);
    if anim.is_some_and(anim_ego_flash) {
        let flash = p.style(Dos::White, Dos::Red);
        put_str(buf, rx, area.y + 3, "EGO", 7, flash);
        put_str(buf, rx + 7, area.y + 3, &b, 18, flash);
        put_str(
            buf,
            x + 1,
            area.y + 3,
            &format!("{}", g.ego.max(0)),
            4,
            flash,
        );
    }

    put_str(buf, rx, area.y + 5, "NOW :", 5, fg(p, Dos::LightGray));
    put_str(
        buf,
        rx + 6,
        area.y + 5,
        &intent_text(g, g.current_intent()),
        29,
        fg(p, Dos::Yellow),
    );
    put_str(buf, rx, area.y + 6, "NEXT:", 5, fg(p, Dos::LightGray));
    put_str(
        buf,
        rx + 6,
        area.y + 6,
        &intent_text(g, g.next_intent()),
        29,
        fg(p, Dos::DarkGray),
    );
    if g.mirror_armed {
        put_str(
            buf,
            rx,
            area.y + 7,
            "MIRROR.EXE ARMED",
            20,
            fg(p, Dos::LightCyan),
        );
    }
    if shadow && g.current_intent() == Intent::Reach {
        put_str(
            buf,
            rx,
            area.y + 8,
            "[X] ACCEPT NOW",
            20,
            fg(p, Dos::LightGreen),
        );
    }

    let wrapped = wrap_tail(g.log().iter().map(|l| ((), l.as_str())), 34, 9);
    for (i, (_, line)) in wrapped.iter().enumerate() {
        put_str(buf, rx, area.y + 10 + u(i), line, 35, fg(p, Dos::LightGray));
    }

    let mut keys = format!(
        "[A]ttack [S]leaze [D]efend [P]rogram(MIRROR x{}) [R]etreat",
        g.loadout().mirror
    );
    if shadow {
        keys.push_str(" [X] Accept");
    }
    footer(buf, area, &keys, p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::params;
    use crate::rng::substream;
    use crate::ui::canvas::row_text;

    fn words(len: usize, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                (0..len)
                    .map(|j| char::from(b'A' + ((i * 7 + j * 3 + i * j) % 26) as u8))
                    .collect::<String>()
            })
            .collect()
    }

    fn area() -> Rect {
        Rect::new(0, 1, 80, VIEW_ROWS)
    }

    fn draw(view: &IceView) -> (Buffer, Rect) {
        let a = area();
        let pal = Palette::new(false);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 25));
        let full = buf.area;
        crate::ui::canvas::fill(&mut buf, full, pal.style(Dos::LightGray, Dos::Black));
        view.render(&mut buf, a, &pal);
        (buf, a)
    }

    fn screen_text(buf: &Buffer) -> String {
        (0..25)
            .map(|y| row_text(buf, buf.area, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rng(tag: &str) -> Rng {
        substream(7, 0, tag, 0)
    }

    #[test]
    fn keys_map_to_ice_inputs() {
        assert_eq!(ice_input(Key::Up), Some(IceInput::Up));
        assert_eq!(ice_input(Key::Enter), Some(IceInput::Enter));
        assert_eq!(ice_input(Key::Esc), Some(IceInput::Esc));
        assert_eq!(ice_input(Key::Char(' ')), Some(IceInput::Space));
        assert_eq!(ice_input(Key::Char('x')), Some(IceInput::Char('x')));
        assert_eq!(ice_input(Key::F(1)), None);
        assert_eq!(ice_input(Key::Backspace), Some(IceInput::Backspace));
    }

    #[test]
    fn wordlock_view_shows_dump_attempts_log_and_program_keys() {
        let p = params(1);
        let w = words(p.wordlock_len, 2 * p.wordlock_words);
        let loadout = Loadout {
            decrypt: 2,
            sniffer: 1,
            ..Loadout::default()
        };
        let mut view =
            IceView::wordlock(&mut rng("wl"), &p, &w, &w[0], loadout, IceGrade::Standard);
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("WORDLOCK"), "{t}");
        assert!(t.contains("ENTER PASSWORD NOW"));
        assert!(t.contains("ATTEMPT(S) LEFT: ■ ■ ■ ■"), "{t}");
        assert!(t.contains("0xF4A0"));
        assert!(t.contains(&format!("0x{:04X}", 0xF4A0 + 12 * 31)));
        assert!(t.contains("[1] DECRYPT x2") && t.contains("[2] SNIFFER x1"));
        // Pick a wrong word: the log shows it and an attempt is gone.
        let wrong = match &view.game {
            Game::Wordlock(g) => g
                .words
                .iter()
                .find(|s| !s.is_password)
                .map(|s| (s.line, s.start)),
            _ => None,
        };
        let (line, start) = wrong.expect("a dud word exists");
        if let Game::Wordlock(g) = &mut view.game {
            g.cursor.col = line / 16;
            g.cursor.row = line % 16;
            g.cursor.ch = start;
        }
        view.handle(IceInput::Enter);
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("ATTEMPT(S) LEFT: ■ ■ ■  "), "{t}");
        assert!(t.contains(">Entry denied."), "{t}");
        assert!(t.contains(">Likeness="));
    }

    #[test]
    fn wordlock_group_under_the_cursor_is_highlighted() {
        let p = params(1);
        let w = words(p.wordlock_len, 2 * p.wordlock_words);
        let mut view = IceView::wordlock(
            &mut rng("wl"),
            &p,
            &w,
            &w[0],
            Loadout::default(),
            IceGrade::Standard,
        );
        let (line, start, len) = match &view.game {
            Game::Wordlock(g) => {
                let s = &g.words[0];
                (s.line, s.start, s.text.chars().count())
            }
            _ => unreachable!(),
        };
        if let Game::Wordlock(g) = &mut view.game {
            g.cursor.col = line / 16;
            g.cursor.row = line % 16;
            g.cursor.ch = start + 1;
        }
        let (buf, a) = draw(&view);
        let pal = Palette::new(false);
        let x0 = a.x + 2 + u(line / 16) * 22 + 7;
        let y = a.y + 4 + u(line % 16);
        // First and last cells of the word carry the group background, the cursor cell the
        // brighter one; the cell after the word is plain.
        assert_eq!(buf[(x0 + u(start), y)].bg, pal.color(Dos::Green));
        assert_eq!(buf[(x0 + u(start + len - 1), y)].bg, pal.color(Dos::Green));
        assert_eq!(buf[(x0 + u(start + 1), y)].bg, pal.color(Dos::LightGreen));
        assert_eq!(buf[(x0 + u(start + len), y)].bg, pal.color(Dos::Black));
    }

    #[test]
    fn breach_view_shows_grid_buffer_timer_and_daemons() {
        let p = params(3);
        let mut view = IceView::breach(&mut rng("br"), &p, Loadout::default());
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("BREACH PROTOCOL"), "{t}");
        assert!(t.contains("TIMER STARTS AT YOUR FIRST PICK"));
        assert!(t.contains("PICK FROM ROW 1"));
        assert!(t.contains("DAEMONS"));
        assert!(t.contains("[  ][  ]"));
        view.handle(IceInput::Enter);
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("TIME  30.0") || t.contains("TIME  29."), "{t}");
        assert!(t.contains("PICK FROM COLUMN"));
        assert!(
            !t.contains("[  ][  ][  ][  ][  ][  ][  ]"),
            "first slot filled"
        );
    }

    #[test]
    fn daemon_progress_counts_the_matching_suffix() {
        assert_eq!(daemon_progress(&[1, 2, 3], &[]), 0);
        assert_eq!(daemon_progress(&[1, 2, 3], &[9, 1]), 1);
        assert_eq!(daemon_progress(&[1, 2, 3], &[1, 2]), 2);
        assert_eq!(daemon_progress(&[1, 2, 3], &[1, 2, 9]), 0);
        assert_eq!(daemon_progress(&[1, 2, 3], &[9, 1, 2, 3, 9]), 3);
    }

    #[test]
    fn phreak_view_flashes_the_lit_key_with_its_dtmf_pair() {
        let p = params(1);
        let mut view = IceView::phreak(
            &mut rng("ph"),
            &p,
            Loadout {
                metronom: true,
                ..Loadout::default()
            },
        );
        let first = match &view.game {
            Game::Phreak(g) => g.expected()[0],
            _ => unreachable!(),
        };
        view.tick(Duration::from_millis(10));
        let (buf, a) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("PHREAK"), "{t}");
        assert!(t.contains("ROUND 1/3"));
        assert!(t.contains("WATCH THE LINE"));
        let (lo, hi) = phreak::dtmf(first).unwrap();
        assert!(t.contains(&format!("{lo} Hz + {hi} Hz")), "{t}");
        assert!(t.contains("[R] REPLAY (METRONOM.EXE)"));
        for key in ['1', '5', '0', '#', '*'] {
            assert!(t.contains(&format!("│ {key} │")), "keypad shows {key}");
        }
        // The lit key is drawn inverse (black on yellow).
        let i = phreak::KEYS.iter().position(|&k| k == first).unwrap();
        let pal = Palette::new(false);
        let cell = &buf[(a.x + 6 + u(i % 3) * 6 + 2, a.y + 3 + u(i / 3) * 3 + 1)];
        assert_eq!(cell.symbol().chars().next(), Some(first));
        assert_eq!(cell.bg, pal.color(Dos::Yellow));
        // After the flash ends the view asks for the sequence.
        view.tick(Duration::from_secs(10));
        let (buf, _) = draw(&view);
        assert!(screen_text(&buf).contains("REPEAT THE SEQUENCE"));
    }

    #[test]
    fn router_view_draws_the_pipe_grid_in_and_out_marks_and_timer() {
        let p = params(1);
        let view = IceView::router(&mut rng("rt"), &p);
        let (buf, a) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("ROUTER"), "{t}");
        assert!(t.contains("TIME 45.0"), "{t}");
        assert!(t.contains("IN ►") && t.contains("► OUT"));
        assert!(t.contains("Space/Enter] rotate"));
        // Every tile row of the real puzzle appears verbatim in the buffer.
        if let Game::Router(g) = &view.game {
            for (r, line) in g.render_rows().iter().enumerate() {
                let row = row_text(&buf, buf.area, a.y + 3 + u(r));
                assert!(row.contains(line.as_str()), "row {r}: {row:?} vs {line:?}");
            }
        }
    }

    #[test]
    fn lunge_offset_peaks_mid_animation_and_returns_to_zero() {
        let anim = |t: f32| AttackAnim {
            intent: Intent::Lunge,
            reflected: false,
            t,
        };
        assert_eq!(anim_offset(&anim(0.0)), 0);
        assert_eq!(anim_offset(&anim(0.45)), 8);
        assert_eq!(anim_offset(&anim(0.9)), 0);
        assert_eq!(
            anim_cells(&anim(0.5), 27, 12).len(),
            0,
            "a lunge is an offset, not cells"
        );
    }

    #[test]
    fn gaze_beam_grows_with_progress_and_reverses_when_reflected() {
        let early = AttackAnim {
            intent: Intent::Gaze,
            reflected: false,
            t: 0.0,
        };
        assert!(anim_cells(&early, 27, 12).is_empty());
        let mid = AttackAnim {
            intent: Intent::Gaze,
            reflected: false,
            t: 0.45,
        };
        let cells = anim_cells(&mid, 27, 12);
        assert!(cells.iter().any(|c| c.3 == Dos::LightMagenta), "{cells:?}");
        assert!(cells.iter().any(|c| c.2 == '\u{25BA}'), "{cells:?}");
        let bounced = AttackAnim {
            intent: Intent::Gaze,
            reflected: true,
            t: 0.45,
        };
        let cells = anim_cells(&bounced, 27, 12);
        assert!(cells.iter().any(|c| c.3 == Dos::LightCyan), "{cells:?}");
        assert!(cells.iter().any(|c| c.2 == '\u{25C4}'), "{cells:?}");
    }

    #[test]
    fn a_hit_animates_then_clears_and_calm_shows_nothing() {
        let p = params(3);
        let mut view = IceView::duel(
            &mut rng("du"),
            &p,
            DuelKind::Black,
            Loadout::default(),
            100,
            DuelArt {
                palette: &Palette::new(false),
                shadow_art: None,
            },
            BlackClass::Eye,
        );
        view.duel_mut().unwrap().set_intent(Intent::Gaze);
        view.handle(IceInput::Char('a'));
        // No beam at t=0, a beam mid-flight, nothing after it ends.
        view.tick(Duration::from_millis(100));
        let (buf, _) = draw(&view);
        assert!(screen_text(&buf).contains("hits you"));
        view.tick(Duration::from_millis(350));
        let (buf, _) = draw(&view);
        assert!(
            screen_text(&buf).contains('\u{2550}'),
            "beam visible mid-animation"
        );
        view.tick(Duration::from_millis(600));
        let (buf, _) = draw(&view);
        assert!(!screen_text(&buf).contains('\u{2550}'), "animation over");
        // Calm mode never starts one.
        let mut calm = IceView::duel(
            &mut rng("du"),
            &p,
            DuelKind::Black,
            Loadout::default(),
            100,
            DuelArt {
                palette: &Palette::new(false),
                shadow_art: None,
            },
            BlackClass::Eye,
        );
        calm.set_calm(true);
        calm.duel_mut().unwrap().set_intent(Intent::Gaze);
        calm.handle(IceInput::Char('a'));
        calm.tick(Duration::from_millis(450));
        let (buf, _) = draw(&calm);
        let t = screen_text(&buf);
        assert!(!t.contains('\u{2550}'), "{t}");
        assert!(t.contains("hits you"), "the log still shows the hit");
    }

    #[test]
    fn daemon_console_takes_typed_spaces_through_the_key_path() {
        let mut view = IceView::daemon(&mut rng("dm"), HostOs::Unix, IceGrade::Standard);
        // A fixed line with a space, so the test cannot pass on a one-word card.
        let first = "ls -a";
        for c in first.chars() {
            view.handle(ice_input(Key::Char(c)).expect("typed input maps"));
        }
        assert_eq!(
            view.daemon_game().unwrap().input,
            first,
            "the space reached the buffer"
        );
        // The live prompt row (row 20 of the view, one above the footer) shows what is typed.
        let typed = screen_text(&draw(&view).0);
        let prompt_row = typed.lines().nth(21).unwrap_or("");
        assert!(
            prompt_row.contains(&format!("% {first}")),
            "prompt row shows the buffer: {prompt_row:?}"
        );
        let footer_row = typed.lines().nth(22).unwrap_or("");
        assert!(
            footer_row.contains("[Enter] run"),
            "the footer stays on its own row: {footer_row:?}"
        );
        view.handle(ice_input(Key::Enter).unwrap());
        let t = screen_text(&draw(&view).0);
        assert!(t.contains(&format!("% {first}")), "{t}");
        assert!(view.daemon_game().unwrap().log.iter().any(|(k, l)| {
            *k == crate::ice::daemon::LogKind::Echo && l == &format!("% {first}")
        }));
    }

    #[test]
    fn black_ice_duel_view_has_avatar_bars_intents_and_keys() {
        let p = params(2);
        let loadout = Loadout {
            mirror: 1,
            ..Loadout::default()
        };
        let mut view = IceView::duel(
            &mut rng("du"),
            &p,
            DuelKind::Black,
            loadout,
            100,
            DuelArt {
                palette: &Palette::new(false),
                shadow_art: None,
            },
            BlackClass::Hydra,
        );
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(t.contains("DUEL  BLACK ICE"), "{t}");
        let Game::Duel(g, _) = &view.game else {
            unreachable!()
        };
        let full = format!("{}/{}", g.max_strength, g.max_strength);
        assert!(t.contains("ICE") && t.contains(&full), "{t}");
        assert!(t.contains("EGO") && t.contains("100"));
        assert!(t.contains("NOW :") && t.contains("NEXT:"));
        assert!(t.contains("[A]ttack [S]leaze [D]efend [P]rogram(MIRROR x1) [R]etreat"));
        assert!(!t.contains("[X] Accept"), "black ICE has nothing to accept");
        assert!(t.contains("It intends:"));
        // Ice art is on screen: one of the three avatars' distinctive rows.
        let art = match g.class() {
            Some(BlackClass::Wolf) => ART_WOLF,
            Some(BlackClass::Eye) => ART_EYE,
            _ => ART_HYDRA,
        };
        for line in art.lines().filter(|l| !l.trim().is_empty()) {
            assert!(t.contains(line.trim_end()), "missing art row {line:?}\n{t}");
        }
        view.handle(IceInput::Char('a'));
        let (buf, _) = draw(&view);
        assert!(screen_text(&buf).contains("You strike for"));
    }

    #[test]
    fn shadow_duel_offers_accept_and_mirrors_the_art() {
        let p = params(2);
        let art = "ab(\n/c";
        let mut view = IceView::duel(
            &mut rng("du"),
            &p,
            DuelKind::Shadow,
            Loadout::default(),
            100,
            DuelArt {
                palette: &Palette::new(false),
                shadow_art: Some(art),
            },
            BlackClass::Hydra,
        );
        let rows: Vec<String> = match &view.game {
            Game::Duel(_, art) => art
                .iter()
                .map(|r| r.iter().map(|c| c.0).collect())
                .collect(),
            _ => unreachable!(),
        };
        assert_eq!(rows, vec![")ba".to_string(), " c\\".to_string()]);
        let (buf, _) = draw(&view);
        let t = screen_text(&buf);
        assert!(
            t.contains("DUEL  YOUR SHADOW") && t.contains("[X] Accept"),
            "{t}"
        );
        // Wait for the REACH intent by acting safely (DEFEND) until it shows.
        for _ in 0..8 {
            let (buf, _) = draw(&view);
            if screen_text(&buf).contains("[X] ACCEPT NOW") {
                break;
            }
            view.handle(IceInput::Char('d'));
        }
        let (buf, _) = draw(&view);
        assert!(screen_text(&buf).contains("[X] ACCEPT NOW"));
        view.handle(IceInput::Char('x'));
        assert_eq!(view.outcome(), Some(IceOutcome::Integrated));
    }

    #[test]
    fn duel_retreat_trace_is_left_to_the_outcome_and_ego_is_drained_once() {
        let p = params(3);
        let mut view = IceView::duel(
            &mut rng("du"),
            &p,
            DuelKind::Black,
            Loadout::default(),
            100,
            DuelArt {
                palette: &Palette::new(false),
                shadow_art: None,
            },
            BlackClass::Hydra,
        );
        // Defend until the ICE has hurt us at least once, then check the drain.
        for _ in 0..4 {
            view.handle(IceInput::Char('d'));
        }
        let first = view.take_ego();
        assert!(first < 0, "four defended turns include a hit");
        assert_eq!(view.take_ego(), 0, "already drained");
        view.handle(IceInput::Char('r'));
        assert_eq!(view.outcome(), Some(IceOutcome::Retreated));
        assert_eq!(
            view.take_trace(),
            0.0,
            "retreat penalty comes from the outcome"
        );
    }

    #[test]
    fn take_spent_reports_each_consumed_charge_once() {
        let p = params(1);
        let w = words(p.wordlock_len, 2 * p.wordlock_words);
        let loadout = Loadout {
            decrypt: 2,
            ..Loadout::default()
        };
        let mut view =
            IceView::wordlock(&mut rng("wl"), &p, &w, &w[0], loadout, IceGrade::Standard);
        assert!(view.take_spent().is_empty());
        view.handle(IceInput::Char('1'));
        assert_eq!(view.take_spent(), vec![(Program::Decrypt, 1)]);
        assert!(view.take_spent().is_empty(), "reported once");
        view.handle(IceInput::Char('1'));
        assert_eq!(view.take_spent(), vec![(Program::Decrypt, 1)]);
    }
}
