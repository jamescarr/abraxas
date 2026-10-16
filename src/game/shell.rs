//! The host shell: command execution and the line editor.
//!
//! `execute` mutates the [`HostSession`] (position, trace, objective, ghost, ...) and returns
//! [`ShellEffect`]s for the UI. Session state is already updated when the effects are returned;
//! effects only tell the UI what to draw or which outside state (EGO, RATIO, deck, screens) to
//! change.

use crate::game::host::{HostSession, IceKindAt, NodeKind};
use crate::game::programs::{self, Program, ProgramKind};
use crate::pack::model::{HostOs, ObjectiveKind};

/// Width of the shell pane.
pub const SHELL_COLS: usize = 54;
/// Lines kept by the line editor's history.
pub const HISTORY_LEN: usize = 50;
const MAX_INPUT: usize = 120;
/// Seconds a `get` transfer takes.
pub const TRANSFER_SECS: f32 = 2.0;
const MANDALA_EGO: i32 = 30;

/// Every command the shell knows (completion list and `help`).
pub const COMMANDS: [&str; 18] = [
    "help", "scan", "ls", "cat", "connect", "cd", "get", "put", "ps", "kill", "bounce", "run",
    "deck", "whoami", "date", "clear", "logout", "exit",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    Normal,
    Error,
    Dim,
    Highlight,
}

/// What the UI must do after a command. Trace, position and objective state are already applied
/// to the session.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEffect {
    /// One line of scrollback (at most 54 columns).
    Print { style: LineStyle, text: String },
    /// Clear the scrollback.
    Clear,
    /// Open the ICE mini-game guarding `node`; report back with `apply_ice_outcome(node, ..)`.
    StartIce { node: usize, kind: IceKindAt },
    /// Draw the node map (`HostSession::map_layout`).
    Scan,
    /// Informational: the trace changed by this much (already applied).
    TraceDelta(f32),
    /// Add KB to RATIO.
    RatioReward(u32),
    /// Add to EGO (clamp to 0..=100).
    EgoDelta(i32),
    /// Add to INTEGRATION (clamped by the run state).
    IntegrationDelta(i32),
    /// The objective is complete (session state already `Done`).
    ObjectiveComplete,
    /// Trace hit 100: the UI returns to the BBS menu (EGO -20 arrives as its own `EgoDelta`).
    DumpShock,
    /// Leave the host. `success` is true iff the objective is done.
    Logout { success: bool },
    /// Show a transfer for `secs`, then interpret `then`.
    StartTransfer {
        file: String,
        bytes: usize,
        secs: f32,
        then: Vec<ShellEffect>,
    },
    /// Start the ROUTER puzzle; report with `apply_bounce_outcome`.
    Bounce,
    /// Spend one charge of this program in the saved deck.
    UseProgram(Program),
    /// The current node changed (read `HostSession::current`).
    Moved { node: usize },
}

impl ShellEffect {
    pub fn line(text: impl Into<String>) -> Self {
        Self::Print {
            style: LineStyle::Normal,
            text: text.into(),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self::Print {
            style: LineStyle::Error,
            text: text.into(),
        }
    }

    pub fn dim(text: impl Into<String>) -> Self {
        Self::Print {
            style: LineStyle::Dim,
            text: text.into(),
        }
    }

    pub fn highlight(text: impl Into<String>) -> Self {
        Self::Print {
            style: LineStyle::Highlight,
            text: text.into(),
        }
    }
}

/// Greedy word wrap to `width` characters, keeping existing line breaks; over-long words break.
pub fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut len = 0;
        for word in raw.split(' ') {
            let mut word: Vec<char> = word.chars().collect();
            let wl = word.len();
            if len > 0 && len + 1 + wl > width {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            while word.len() > width {
                let rest = word.split_off(width);
                if len > 0 {
                    out.push(std::mem::take(&mut line));
                }
                out.push(word.into_iter().collect());
                word = rest;
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.extend(word.iter());
            len += word.len();
        }
        out.push(line);
    }
    out
}

fn say(out: &mut Vec<ShellEffect>, text: &str) {
    for l in wrap_lines(text, SHELL_COLS) {
        out.push(ShellEffect::line(l));
    }
}

fn err(out: &mut Vec<ShellEffect>, text: &str) {
    for l in wrap_lines(text, SHELL_COLS) {
        out.push(ShellEffect::error(l));
    }
}

/// Run one command line.
pub fn execute(session: &mut HostSession, line: &str) -> Vec<ShellEffect> {
    let mut out = Vec::new();
    if session.dumped {
        return out;
    }
    let line = line.trim();
    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    if cmd.is_empty() {
        return out;
    }
    let cmd = cmd.to_ascii_lowercase();
    match cmd.as_str() {
        "help" | "?" => help(&mut out),
        "scan" => {
            if session.bump(0.5, true, &mut out) {
                let cur = session.current_node();
                let adj: Vec<&str> = cur
                    .neighbors
                    .iter()
                    .map(|&i| session.nodes[i].id.as_str())
                    .collect();
                say(&mut out, &format!("adjacent: {}", adj.join(" ")));
                out.push(ShellEffect::Scan);
            }
        }
        "ls" | "dir" => {
            if session.bump(0.5, true, &mut out) {
                ls(session, &mut out);
            }
        }
        "cat" | "type" => cat(session, rest, &mut out),
        "connect" | "cd" => connect(session, rest, &mut out),
        "get" => get(session, rest, &mut out),
        "put" => put(session, rest, &mut out),
        "ps" => ps(session, &mut out),
        "kill" => kill(session, rest, &mut out),
        "bounce" => {
            if session.bounce_used {
                err(&mut out, "bounce: the route is already burned.");
            } else {
                out.push(ShellEffect::Bounce);
            }
        }
        "run" => run(session, rest, &mut out),
        "deck" => deck(session, &mut out),
        "whoami" => {
            if session.whoami_seen {
                say(&mut out, &session.player.handle.clone());
            } else {
                session.whoami_seen = true;
                say(&mut out, &session.player.shadow.clone());
                say(&mut out, &format!("no. {}", session.player.handle));
            }
        }
        "date" => {
            let clock = session.player.clock.clone();
            say(&mut out, &clock);
        }
        "clear" | "cls" => out.push(ShellEffect::Clear),
        "logout" | "exit" | "jackout" => out.push(ShellEffect::Logout {
            success: session.objective_done(),
        }),
        "xyzzy" => {
            if session.player.dev {
                complete(session, &mut out);
                say(&mut out, "a hollow voice says: plugh.");
            } else {
                say(&mut out, "Nothing happens.");
            }
        }
        _ => unknown(session.os, &cmd, &mut out),
    }
    out
}

fn unknown(os: HostOs, cmd: &str, out: &mut Vec<ShellEffect>) {
    let text = match os {
        HostOs::Unix => format!("{cmd}: command not found"),
        HostOs::Vax => format!("%DCL-W-IVVERB, unrecognized command verb - {cmd}"),
        HostOs::Mainframe => "?SYNTAX ERROR".to_string(),
        HostOs::Dos => "Bad command or file name".to_string(),
    };
    err(out, &text);
}

fn help(out: &mut Vec<ShellEffect>) {
    for l in [
        "help               this list",
        "scan               draw the node map",
        "ls                 list files here",
        "cat <file>         print a file",
        "connect <node>     move to an adjacent node (cd)",
        "get <file>         download a file",
        "put <file>         upload a file",
        "ps / kill <pid>    list / kill processes",
        "bounce             re-route the trace (once)",
        "run <program>      ghost, mandala",
        "deck               list your programs",
        "whoami, date, clear",
        "logout             jack out (exit, jackout)",
    ] {
        out.push(ShellEffect::line(l));
    }
}

fn ls(session: &HostSession, out: &mut Vec<ShellEffect>) {
    let node = session.current_node();
    if node.files.is_empty() {
        out.push(ShellEffect::dim("(no files)"));
        return;
    }
    for f in &node.files {
        let desc: String = f.description.chars().take(24).collect();
        out.push(ShellEffect::line(format!(
            "{:<12} {:>5}  {}",
            f.name,
            f.size(),
            desc
        )));
    }
}

fn cat(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    if rest.is_empty() {
        err(out, "usage: cat <file>");
        return;
    }
    let Some(i) = session.find_file(session.current, rest) else {
        err(out, &format!("cat: {rest}: no such file"));
        return;
    };
    if !session.bump(0.5, true, out) {
        return;
    }
    let body = session.nodes[session.current].files[i].body.clone();
    say(out, &body);
}

fn node_blurb(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Portal => "the front door",
        NodeKind::Gate => "a gateway",
        NodeKind::Relay => "a phone relay",
        NodeKind::Archive => "an archive",
        NodeKind::Vault => "a vault",
        NodeKind::Core => "a core system",
        NodeKind::Mirror => "a room with a mirror in it",
    }
}

fn connect(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    if rest.is_empty() {
        err(out, "usage: connect <node>");
        return;
    }
    let Some(target) = session.find_node(rest) else {
        err(out, &format!("no route to {rest}"));
        return;
    };
    if target == session.current {
        say(out, &format!("already at {}", session.nodes[target].id));
        return;
    }
    if !session.current_node().neighbors.contains(&target) {
        err(out, &format!("no route to {rest}"));
        return;
    }
    if session.current == 0 && session.needs_login() {
        // The front door is still locked: nothing beyond it is reachable until the WORDLOCK
        // falls, and trying again re-opens the password prompt.
        err(out, "ACCESS DENIED: the front door is locked.");
        out.push(ShellEffect::highlight("ENTER PASSWORD NOW"));
        out.push(ShellEffect::StartIce {
            node: 0,
            kind: IceKindAt::Wordlock,
        });
        return;
    }
    if !session.bump(2.0, true, out) {
        return;
    }
    let node = &session.nodes[target];
    if let (true, Some(kind)) = (node.guarded(), node.ice) {
        out.push(ShellEffect::highlight(format!(
            "{} [{}]: {} holds the way.",
            node.id,
            node.display,
            kind.label()
        )));
        out.push(ShellEffect::StartIce { node: target, kind });
        return;
    }
    session.current = target;
    out.push(ShellEffect::Moved { node: target });
    let node = &session.nodes[target];
    say(
        out,
        &format!(
            "connected to {} [{}], {}.",
            node.id,
            node.display,
            node_blurb(node.kind)
        ),
    );
}

fn complete(session: &mut HostSession, out: &mut Vec<ShellEffect>) {
    if !session.objective_done() {
        session.objective = crate::game::host::ObjectiveState::Done;
        out.push(ShellEffect::ObjectiveComplete);
    }
}

fn get(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    if rest.is_empty() {
        err(out, "usage: get <file>");
        return;
    }
    let cur = session.current;
    let Some(i) = session.find_file(cur, rest) else {
        err(out, &format!("get: {rest}: no such file"));
        return;
    };
    if session.nodes[cur].files[i].fetched {
        say(out, &format!("get: {rest}: already downloaded"));
        return;
    }
    if !session.bump(5.0, true, out) {
        return;
    }
    let (name, bytes, is_target, reward) = {
        let f = &mut session.nodes[cur].files[i];
        f.fetched = true;
        (f.name.clone(), f.size(), f.is_target, f.reward_kb)
    };
    let mut then = Vec::new();
    if is_target {
        complete(session, &mut then);
    } else if reward > 0 {
        then.push(ShellEffect::RatioReward(reward));
        then.push(ShellEffect::line(format!("+{reward}K credited.")));
    }
    out.push(ShellEffect::StartTransfer {
        file: name,
        bytes,
        secs: TRANSFER_SECS,
        then,
    });
}

fn put(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    if rest.is_empty() {
        err(out, "usage: put <file>");
        return;
    }
    if session.objective_kind != ObjectiveKind::Plant {
        err(out, "put: this system accepts no uploads.");
        return;
    }
    if !rest.eq_ignore_ascii_case(&session.target_file) {
        err(out, &format!("put: {rest}: not in your deck"));
        return;
    }
    if session.current != session.objective_node {
        err(out, "put: nothing here can run it.");
        return;
    }
    if !session.bump(5.0, true, out) {
        return;
    }
    let mut then = Vec::new();
    let was_done = session.objective_done();
    complete(session, &mut then);
    if !was_done {
        then.insert(0, ShellEffect::line("payload accepted. it is running."));
    }
    out.push(ShellEffect::StartTransfer {
        file: session.target_file.clone(),
        bytes: 4096,
        secs: TRANSFER_SECS,
        then,
    });
}

fn at_crash_core(session: &HostSession) -> bool {
    session.objective_kind == ObjectiveKind::Crash && session.current == session.objective_node
}

fn ps(session: &mut HostSession, out: &mut Vec<ShellEffect>) {
    if !at_crash_core(session) {
        say(out, "ps: nothing of interest is running here.");
        return;
    }
    if !session.bump(3.0, true, out) {
        return;
    }
    out.push(ShellEffect::highlight("  PID NAME"));
    for p in &session.processes {
        out.push(ShellEffect::line(format!("{:>5} {}", p.pid, p.name)));
    }
}

fn kill(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    let Ok(pid) = rest.trim().parse::<u32>() else {
        err(out, "usage: kill <pid>");
        return;
    };
    let Some(idx) = session.processes.iter().position(|p| p.pid == pid) else {
        err(out, &format!("kill: {pid}: no such process"));
        return;
    };
    if !at_crash_core(session) {
        err(out, &format!("kill: {pid}: no such process"));
        return;
    }
    if !session.bump(3.0, true, out) {
        return;
    }
    let p = session.processes.remove(idx);
    if p.is_target {
        say(
            out,
            &format!("{} ({}) terminated. core dumped.", p.name, p.pid),
        );
        complete(session, out);
    } else {
        say(out, "you killed something that mattered to someone.");
        session.bump(10.0, true, out);
    }
}

fn run(session: &mut HostSession, rest: &str, out: &mut Vec<ShellEffect>) {
    let name = rest.trim();
    if name.is_empty() {
        err(out, "usage: run <program>");
        return;
    }
    let stem = name.split('.').next().unwrap_or(name);
    let program = Program::from_file_name(name).or_else(|| {
        Program::ALL.into_iter().find(|p| {
            p.file_name()
                .split('.')
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        })
    });
    let Some(program) = program else {
        err(out, &format!("run: {name}: not found"));
        return;
    };
    match program {
        Program::Ghost | Program::Mandala => {}
        _ => {
            err(
                out,
                &format!("run: {} does not run here.", program.file_name()),
            );
            return;
        }
    }
    if program == Program::Ghost && session.ghost {
        say(out, "GHOST.COM is already running.");
        return;
    }
    if !programs::spend(&mut session.player.deck, program) {
        err(
            out,
            &format!("run: {}: no charges left", program.file_name()),
        );
        return;
    }
    out.push(ShellEffect::UseProgram(program));
    if program == Program::Ghost {
        session.ghost = true;
        say(out, "GHOST.COM loaded. your trace rate drops.");
    } else {
        out.push(ShellEffect::EgoDelta(MANDALA_EGO));
        say(
            out,
            "MANDALA.SYS loaded. you remember who you are. +30 EGO.",
        );
    }
}

fn deck(session: &HostSession, out: &mut Vec<ShellEffect>) {
    out.push(ShellEffect::line("PAYLOAD.PIF   installed"));
    for (program, &n) in &session.player.deck {
        if n == 0 {
            continue;
        }
        let state = match program.kind() {
            ProgramKind::Persistent => "installed".to_string(),
            ProgramKind::Charges(_) => format!("{n} charge{}", if n == 1 { "" } else { "s" }),
        };
        out.push(ShellEffect::line(format!(
            "{:<13} {}",
            program.file_name(),
            state
        )));
    }
}

/// Keys the line editor understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Char(char),
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    Up,
    Down,
    Tab,
    Enter,
    /// Ctrl+L: `handle` returns `Some("clear")` and leaves the buffer untouched.
    ClearScreen,
}

#[derive(Debug, Clone)]
struct Completion {
    /// Char index where the completed word starts.
    start: usize,
    /// Char length of the text currently inserted at `start`.
    len: usize,
    matches: Vec<String>,
    next: usize,
}

/// A single-line editor with history and Tab completion.
#[derive(Debug, Clone, Default)]
pub struct LineEditor {
    buf: String,
    /// Cursor position in characters.
    cursor: usize,
    history: Vec<String>,
    /// Index into `history` while walking it; `draft` holds the line being typed.
    walk: Option<usize>,
    draft: String,
    completion: Option<Completion>,
}

impl LineEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.buf
    }

    /// Cursor position in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.walk = None;
        self.completion = None;
    }

    fn char_len(&self) -> usize {
        self.buf.chars().count()
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.buf
            .char_indices()
            .nth(chars)
            .map_or(self.buf.len(), |(b, _)| b)
    }

    fn set_text(&mut self, text: &str) {
        self.buf = text.to_string();
        self.cursor = self.char_len();
    }

    /// Feed one key. Returns the finished line on Enter (also for an empty line); Ctrl+L returns
    /// `Some("clear")` without touching the buffer or history.
    pub fn handle(&mut self, key: EditKey, session: &HostSession) -> Option<String> {
        if key != EditKey::Tab {
            self.completion = None;
        }
        match key {
            EditKey::Char(c) => {
                if !c.is_control() && self.char_len() < MAX_INPUT {
                    let at = self.byte_at(self.cursor);
                    self.buf.insert(at, c);
                    self.cursor += 1;
                    self.walk = None;
                }
            }
            EditKey::Left => self.cursor = self.cursor.saturating_sub(1),
            EditKey::Right => self.cursor = (self.cursor + 1).min(self.char_len()),
            EditKey::Home => self.cursor = 0,
            EditKey::End => self.cursor = self.char_len(),
            EditKey::Backspace => {
                if self.cursor > 0 {
                    let at = self.byte_at(self.cursor - 1);
                    self.buf.remove(at);
                    self.cursor -= 1;
                    self.walk = None;
                }
            }
            EditKey::Delete => {
                if self.cursor < self.char_len() {
                    let at = self.byte_at(self.cursor);
                    self.buf.remove(at);
                    self.walk = None;
                }
            }
            EditKey::Up => {
                if self.history.is_empty() {
                    return None;
                }
                let idx = match self.walk {
                    None => {
                        self.draft = self.buf.clone();
                        self.history.len() - 1
                    }
                    Some(i) => i.saturating_sub(1),
                };
                self.walk = Some(idx);
                let line = self.history[idx].clone();
                self.set_text(&line);
            }
            EditKey::Down => {
                if let Some(i) = self.walk {
                    if i + 1 < self.history.len() {
                        self.walk = Some(i + 1);
                        let line = self.history[i + 1].clone();
                        self.set_text(&line);
                    } else {
                        self.walk = None;
                        let draft = std::mem::take(&mut self.draft);
                        self.set_text(&draft);
                    }
                }
            }
            EditKey::Tab => self.complete(session),
            EditKey::Enter => {
                let line = std::mem::take(&mut self.buf);
                self.cursor = 0;
                self.walk = None;
                self.draft.clear();
                if !line.trim().is_empty() && self.history.last() != Some(&line) {
                    self.history.push(line.clone());
                    if self.history.len() > HISTORY_LEN {
                        self.history.remove(0);
                    }
                }
                return Some(line);
            }
            EditKey::ClearScreen => return Some("clear".to_string()),
        }
        None
    }

    fn complete(&mut self, session: &HostSession) {
        if let Some(c) = &mut self.completion {
            if c.matches.is_empty() {
                return;
            }
            let pick = c.matches[c.next % c.matches.len()].clone();
            c.next += 1;
            let (start, len) = (c.start, c.len);
            c.len = pick.chars().count();
            let new_len = c.len;
            self.replace_range(start, len, &pick);
            self.cursor = start + new_len;
            return;
        }
        // Word under the cursor.
        let before: Vec<char> = self.buf.chars().take(self.cursor).collect();
        let start = before
            .iter()
            .rposition(|c| c.is_whitespace())
            .map_or(0, |i| i + 1);
        let prefix: String = before[start..].iter().collect();
        let head: String = before[..start].iter().collect();
        let first_word = head.trim().is_empty();
        let matches = candidates(session, first_word, head.trim(), &prefix);
        if matches.is_empty() {
            return;
        }
        let pick = matches[0].clone();
        let len = prefix.chars().count();
        self.completion = Some(Completion {
            start,
            len: pick.chars().count(),
            matches,
            next: 1,
        });
        self.replace_range(start, len, &pick);
        self.cursor = start + pick.chars().count();
    }

    fn replace_range(&mut self, start: usize, len: usize, with: &str) {
        let a = self.byte_at(start);
        let b = self.byte_at(start + len);
        self.buf.replace_range(a..b, with);
    }
}

fn candidates(session: &HostSession, first_word: bool, head: &str, prefix: &str) -> Vec<String> {
    let mut pool: Vec<String> = Vec::new();
    if first_word {
        pool.extend(COMMANDS.iter().map(|c| (*c).to_string()));
        pool.push("jackout".to_string());
    } else if head.eq_ignore_ascii_case("run") {
        pool.extend(["ghost".to_string(), "mandala".to_string()]);
    } else {
        pool.extend(session.nodes.iter().map(|n| n.id.clone()));
        pool.extend(session.current_node().files.iter().map(|f| f.name.clone()));
    }
    let lower = prefix.to_ascii_lowercase();
    let mut matches: Vec<String> = pool
        .into_iter()
        .filter(|c| c.to_ascii_lowercase().starts_with(&lower))
        .collect();
    matches.sort();
    matches.dedup();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::host::{ObjectiveState, PlayerView, PromptColor};
    use crate::game::state::{HostInfo, Level, LevelSeed, Place, Sysop};
    use crate::ice::IceOutcome;
    use crate::pack::model::{ExitKind, FileDef, ObjectiveIce};
    use std::time::Duration;

    fn file(name: &str) -> FileDef {
        FileDef {
            name: name.into(),
            description: format!("{name} desc"),
            body: format!("body of {name}"),
        }
    }

    fn level(
        kind: ObjectiveKind,
        ice: Option<ObjectiveIce>,
        nodes: Option<u8>,
        os: HostOs,
    ) -> Level {
        let seed = LevelSeed {
            depth: 5,
            act_id: "albedo".into(),
            anchor: None,
            base_danger: 2,
            omen: 1,
            motifs: vec![],
            room: String::new(),
            focus: None,
            objective_kind: kind,
            target_daemon: (kind == ObjectiveKind::Crash).then(|| "pleroma".to_string()),
            wordlock_len: 6,
        };
        let target = match kind {
            ObjectiveKind::Retrieve => "WREN-01.LOG",
            ObjectiveKind::Plant => "PAYLOAD.PIF",
            ObjectiveKind::Crash => "",
        };
        let mut level = Level::new(seed, vec![]);
        level.place = Some(Place {
            title: "T".into(),
            bbs_name: "B".into(),
            tagline: String::new(),
            level_card: String::new(),
            sysop: Sysop {
                handle: "X".into(),
                cast_id: None,
            },
            host: HostInfo {
                name: "ODS TEST".into(),
                os,
                banner: "hello".into(),
                objective: "do the thing".into(),
                target_file: target.into(),
                objective_ice: ice,
                nodes,
                node_names: (0..8).map(|i| format!("NODE {i}")).collect(),
                ice_names: vec![],
            },
            posts: vec![],
            text_files: vec![],
            host_files: vec![file("A.TXT"), file("B.TXT"), file("C.TXT")],
            words: vec![],
            password: String::new(),
        });
        level
    }

    fn daemons() -> Vec<String> {
        ["sendmail", "cron", "lpd", "pleroma", "nfsd"]
            .map(String::from)
            .to_vec()
    }

    fn gen_with(l: &Level, rd: u8, exit: Option<ExitKind>, seed: u64) -> HostSession {
        let mut s = HostSession::generate(l, rd, exit, 0, seed, &daemons());
        s.set_player(PlayerView {
            handle: "ZED".into(),
            shadow: "dez".into(),
            dev: false,
            clock: "12/31/99 23:04".into(),
            ..PlayerView::default()
        });
        s
    }

    fn mk(kind: ObjectiveKind, rd: u8, exit: Option<ExitKind>, seed: u64) -> HostSession {
        gen_with(&level(kind, None, None, HostOs::Unix), rd, exit, seed)
    }

    fn flat(effects: &[ShellEffect]) -> Vec<ShellEffect> {
        let mut v = Vec::new();
        for e in effects {
            v.push(e.clone());
            if let ShellEffect::StartTransfer { then, .. } = e {
                v.extend(flat(then));
            }
        }
        v
    }

    fn text(effects: &[ShellEffect]) -> String {
        flat(effects)
            .iter()
            .filter_map(|e| match e {
                ShellEffect::Print { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn has(effects: &[ShellEffect], want: &ShellEffect) -> bool {
        flat(effects).contains(want)
    }

    fn all_cases() -> Vec<(u64, u8, Option<ExitKind>, ObjectiveKind)> {
        let mut v = Vec::new();
        for seed in 0..40 {
            for rd in 1..=5 {
                for exit in [
                    None,
                    Some(ExitKind::Calm),
                    Some(ExitKind::Deep),
                    Some(ExitKind::Mirror),
                ] {
                    for kind in [
                        ObjectiveKind::Retrieve,
                        ObjectiveKind::Plant,
                        ObjectiveKind::Crash,
                    ] {
                        v.push((seed, rd, exit, kind));
                    }
                }
            }
        }
        v
    }

    fn bfs_ignoring_mirror(s: &HostSession) -> Vec<usize> {
        let n = s.nodes().len();
        let mut dist = vec![usize::MAX; n];
        dist[0] = 0;
        let mut q = std::collections::VecDeque::from([0]);
        while let Some(u) = q.pop_front() {
            for &v in &s.nodes()[u].neighbors {
                if s.nodes()[v].kind != NodeKind::Mirror && dist[v] == usize::MAX {
                    dist[v] = dist[u] + 1;
                    q.push_back(v);
                }
            }
        }
        dist
    }

    #[test]
    fn graph_is_connected_symmetric_and_degree_capped() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let want = usize::from(crate::game::danger::params(rd).nodes)
                + usize::from(exit == Some(ExitKind::Mirror));
            assert_eq!(s.nodes().len(), want);
            let dist = bfs_ignoring_mirror(&s);
            for (i, n) in s.nodes().iter().enumerate() {
                assert!(
                    n.neighbors.len() <= 3,
                    "degree of {} case {seed}/{rd}",
                    n.id
                );
                assert!(n.kind == NodeKind::Mirror || dist[i] != usize::MAX);
                for &j in &n.neighbors {
                    assert!(s.nodes()[j].neighbors.contains(&i));
                    assert_ne!(i, j);
                }
                assert_eq!(n.id, format!("{}{}", n.kind.label(), i));
            }
            if want - usize::from(exit == Some(ExitKind::Mirror)) >= 6 {
                let edges = s.edges().len();
                let tree = s.nodes().len() - 1;
                assert_eq!(edges, tree + 1, "extra edge expected {seed}/{rd}");
            }
        }
    }

    #[test]
    fn objective_is_farthest_non_mirror_node_with_lowest_index_tie() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let dist = bfs_ignoring_mirror(&s);
            let obj = s.objective_node();
            let max = dist
                .iter()
                .filter(|&&d| d != usize::MAX)
                .max()
                .copied()
                .unwrap_or(0);
            assert_eq!(dist[obj], max);
            assert!((1..obj).all(|i| dist[i] < max), "a lower index ties");
            assert_ne!(s.nodes()[obj].kind, NodeKind::Mirror);
        }
    }

    #[test]
    fn mirror_node_only_on_mirror_exit_and_is_a_leaf() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let mirrors: Vec<_> = s
                .nodes()
                .iter()
                .enumerate()
                .filter(|(_, n)| n.kind == NodeKind::Mirror)
                .collect();
            if exit == Some(ExitKind::Mirror) {
                assert_eq!(mirrors.len(), 1);
                let (i, n) = mirrors[0];
                assert_eq!(n.neighbors.len(), 1);
                assert_eq!(n.ice, Some(IceKindAt::Shadow));
                assert_eq!(i, s.nodes().len() - 1);
                assert_ne!(n.neighbors[0], s.objective_node());
            } else {
                assert!(mirrors.is_empty());
            }
        }
    }

    #[test]
    fn objective_node_kind_and_ice_follow_danger_rules() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let n = &s.nodes()[s.objective_node()];
            let want = match kind {
                ObjectiveKind::Retrieve if rd >= 3 => (NodeKind::Vault, IceKindAt::Black),
                ObjectiveKind::Retrieve => (NodeKind::Archive, IceKindAt::Breach),
                ObjectiveKind::Plant => (NodeKind::Core, IceKindAt::Breach),
                ObjectiveKind::Crash if rd >= 2 => (NodeKind::Core, IceKindAt::Black),
                ObjectiveKind::Crash => (NodeKind::Core, IceKindAt::Breach),
            };
            assert_eq!((n.kind, n.ice.unwrap()), want, "{kind:?} D{rd}");
        }
    }

    #[test]
    fn anchor_objective_ice_overrides_and_node_count_overrides() {
        for ice in [
            ObjectiveIce::Breach,
            ObjectiveIce::Black,
            ObjectiveIce::Shadow,
        ] {
            let l = level(ObjectiveKind::Retrieve, Some(ice), Some(3), HostOs::Dos);
            let s = gen_with(&l, 5, None, 9);
            assert_eq!(s.nodes().len(), 3);
            let n = &s.nodes()[s.objective_node()];
            let want = match ice {
                ObjectiveIce::Breach => IceKindAt::Breach,
                ObjectiveIce::Black => IceKindAt::Black,
                ObjectiveIce::Shadow => IceKindAt::Shadow,
            };
            assert_eq!(n.ice, Some(want));
        }
    }

    #[test]
    fn other_nodes_follow_the_kind_cycle_and_relay_vanishes_at_d1() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let mut counts = std::collections::HashMap::new();
            for (i, n) in s.nodes().iter().enumerate() {
                if i == 0 || i == s.objective_node() || n.kind == NodeKind::Mirror {
                    continue;
                }
                *counts.entry(n.kind).or_insert(0) += 1;
                match n.kind {
                    NodeKind::Archive => assert_eq!(n.ice, None),
                    NodeKind::Gate => assert_eq!(n.ice, Some(IceKindAt::Breach)),
                    NodeKind::Relay => {
                        assert_ne!(rd, 1);
                        assert_eq!(n.ice, Some(IceKindAt::Phreak));
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert!(counts.get(&NodeKind::Archive).copied().unwrap_or(0) >= 1);
            if rd == 1 {
                assert!(!counts.contains_key(&NodeKind::Relay));
            }
        }
        let s = mk(ObjectiveKind::Plant, 5, None, 1);
        // 8 nodes, 6 fillers: archive gate relay gate archive gate.
        assert_eq!(
            s.nodes()
                .iter()
                .filter(|n| n.kind == NodeKind::Relay)
                .count(),
            1
        );
    }

    #[test]
    fn files_are_placed_round_robin_and_target_sits_at_objective() {
        for (seed, rd, exit, kind) in all_cases() {
            let s = mk(kind, rd, exit, seed);
            let total: usize = s.nodes().iter().map(|n| n.files.len()).sum();
            let targets: Vec<_> = s
                .nodes()
                .iter()
                .enumerate()
                .flat_map(|(i, n)| n.files.iter().map(move |f| (i, f)))
                .filter(|(_, f)| f.is_target)
                .collect();
            if kind == ObjectiveKind::Retrieve {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].0, s.objective_node());
                assert_eq!(targets[0].1.name, "WREN-01.LOG");
                assert_eq!(total, 4);
            } else {
                assert!(targets.is_empty());
                assert_eq!(total, 3);
            }
            for n in s.nodes() {
                for f in n.files.iter().filter(|f| !f.is_target) {
                    assert_eq!(n.kind, NodeKind::Archive);
                    assert_ne!(n.id, s.nodes()[s.objective_node()].id);
                    assert_eq!(f.reward_kb, 8);
                }
            }
        }
    }

    #[test]
    fn generation_is_deterministic_and_attempt_changes_it() {
        let l = level(ObjectiveKind::Crash, None, None, HostOs::Vax);
        let a = HostSession::generate(&l, 4, Some(ExitKind::Mirror), 0, 77, &daemons());
        let b = HostSession::generate(&l, 4, Some(ExitKind::Mirror), 0, 77, &daemons());
        assert_eq!(a.nodes(), b.nodes());
        assert_eq!(a.processes(), b.processes());
        assert_eq!(a.objective_node(), b.objective_node());
        let mut differs = false;
        for attempt in 1..8 {
            let c = HostSession::generate(&l, 4, Some(ExitKind::Mirror), attempt, 77, &daemons());
            differs |= c.nodes() != a.nodes() || c.processes() != a.processes();
        }
        assert!(differs, "retry hosts should differ");
    }

    /// Walk the shell to the objective node, clearing ICE on the way.
    fn go_to_objective(s: &mut HostSession) {
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        // Path via BFS parents.
        let n = s.nodes().len();
        let mut prev = vec![usize::MAX; n];
        let mut q = std::collections::VecDeque::from([0usize]);
        prev[0] = 0;
        while let Some(u) = q.pop_front() {
            for &v in &s.nodes()[u].neighbors {
                if prev[v] == usize::MAX {
                    prev[v] = u;
                    q.push_back(v);
                }
            }
        }
        let mut path = vec![s.objective_node()];
        while *path.last().unwrap() != 0 {
            path.push(prev[*path.last().unwrap()]);
        }
        path.reverse();
        for &i in &path[1..] {
            let id = s.nodes()[i].id.clone();
            let fx = execute(s, &format!("connect {id}"));
            if let Some(ShellEffect::StartIce { node, .. }) = fx
                .iter()
                .find(|e| matches!(e, ShellEffect::StartIce { .. }))
            {
                s.apply_ice_outcome(*node, IceOutcome::Cleared);
            }
            assert_eq!(s.current(), i);
        }
    }

    #[test]
    fn a_locked_front_door_blocks_connect_and_reoffers_the_wordlock() {
        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 3);
        assert!(s.needs_login());
        let adj = s.nodes()[0].neighbors[0];
        let id = s.nodes()[adj].id.clone();
        let fx = execute(&mut s, &format!("connect {id}"));
        assert!(has(
            &fx,
            &ShellEffect::StartIce {
                node: 0,
                kind: IceKindAt::Wordlock
            }
        ));
        assert_eq!(s.current(), 0);
        assert_eq!(s.trace(), 0.0, "a refused connect costs nothing");
        // Read-only commands still work while locked out.
        assert!(!text(&execute(&mut s, "ls")).is_empty());
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let fx = execute(&mut s, &format!("connect {id}"));
        assert!(
            !has(
                &fx,
                &ShellEffect::StartIce {
                    node: 0,
                    kind: IceKindAt::Wordlock
                }
            ),
            "after login the front door no longer blocks"
        );
    }

    #[test]
    fn connect_rules_ice_and_errors() {
        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 3);
        assert!(s.needs_login());
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        assert!(!s.needs_login());
        let far = (1..s.nodes().len())
            .find(|i| !s.nodes()[0].neighbors.contains(i))
            .unwrap();
        let far_id = s.nodes()[far].id.clone();
        let fx = execute(&mut s, &format!("connect {far_id}"));
        assert_eq!(text(&fx), format!("no route to {far_id}"));
        assert_eq!(s.current(), 0);
        assert_eq!(s.trace(), 0.0);

        // Adjacent guarded node starts ICE instead of moving; clearing completes the move.
        let adj = s.nodes()[0].neighbors[0];
        let id = s.nodes()[adj].id.clone();
        let guarded = s.nodes()[adj].ice;
        let fx = execute(&mut s, &format!("cd {id}"));
        if let Some(kind) = guarded {
            assert!(has(&fx, &ShellEffect::StartIce { node: adj, kind }));
            assert_eq!(s.current(), 0);
            assert!((s.trace() - 2.0).abs() < 1e-4);
            s.apply_ice_outcome(adj, IceOutcome::Cleared);
        }
        assert_eq!(s.current(), adj);
        // Display names also resolve.
        let back = s.nodes()[0].display.clone();
        execute(&mut s, &format!("connect {back}"));
        assert_eq!(s.current(), 0);
    }

    #[test]
    fn get_target_completes_retrieve_and_other_files_pay_once() {
        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 5);
        go_to_objective(&mut s);
        let before = s.trace();
        let fx = execute(&mut s, "get wren-01.log");
        assert!(has(&fx, &ShellEffect::ObjectiveComplete));
        assert!(
            matches!(&fx[fx.len() - 1], ShellEffect::StartTransfer { file, .. } if file == "WREN-01.LOG")
        );
        assert!((s.trace() - before - 5.0).abs() < 1e-4);
        assert_eq!(s.objective_state(), ObjectiveState::Done);
        let again = execute(&mut s, "get WREN-01.LOG");
        assert!(!has(&again, &ShellEffect::ObjectiveComplete));
        assert!(matches!(
            execute(&mut s, "logout").as_slice(),
            [ShellEffect::Logout { success: true }]
        ));

        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 5);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let (node, name) = s
            .nodes()
            .iter()
            .enumerate()
            .find_map(|(i, n)| {
                n.files
                    .first()
                    .filter(|f| !f.is_target)
                    .map(|f| (i, f.name.clone()))
            })
            .unwrap();
        s.current = node;
        let fx = execute(&mut s, &format!("get {name}"));
        assert!(has(&fx, &ShellEffect::RatioReward(8)));
        assert!(!s.objective_done());
        let fx = execute(&mut s, &format!("get {name}"));
        assert!(!has(&fx, &ShellEffect::RatioReward(8)));
        assert!(matches!(
            execute(&mut s, "exit").as_slice(),
            [ShellEffect::Logout { success: false }]
        ));
        assert!(text(&execute(&mut s, "get NOPE.TXT")).contains("no such file"));
    }

    #[test]
    fn target_file_only_completes_at_the_objective_node() {
        let s = mk(ObjectiveKind::Retrieve, 2, None, 5);
        let mut other = s.clone();
        other.apply_ice_outcome(0, IceOutcome::Cleared);
        // The target is not visible from node 0.
        let fx = execute(&mut other, "get WREN-01.LOG");
        assert!(!has(&fx, &ShellEffect::ObjectiveComplete));
    }

    #[test]
    fn plant_requires_the_payload_at_the_objective_node() {
        let mut s = mk(ObjectiveKind::Plant, 3, None, 2);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        assert!(text(&execute(&mut s, "put PAYLOAD.PIF")).contains("nothing here"));
        go_to_objective(&mut s);
        assert!(text(&execute(&mut s, "put OTHER.TXT")).contains("not in your deck"));
        let fx = execute(&mut s, "put payload.pif");
        assert!(has(&fx, &ShellEffect::ObjectiveComplete));
        assert!(s.objective_done());
        let mut g = mk(ObjectiveKind::Retrieve, 3, None, 2);
        assert!(text(&execute(&mut g, "put PAYLOAD.PIF")).contains("no uploads"));
    }

    #[test]
    fn crash_lists_four_processes_and_kill_rules() {
        for seed in 0..30 {
            let mut s = mk(ObjectiveKind::Crash, 3, None, seed);
            assert_eq!(s.processes().len(), 4);
            assert_eq!(s.processes().iter().filter(|p| p.is_target).count(), 1);
            let names: std::collections::HashSet<_> =
                s.processes().iter().map(|p| p.name.clone()).collect();
            assert_eq!(names.len(), 4);
            assert!(names.contains("pleroma"));
            go_to_objective(&mut s);
            let listing = text(&execute(&mut s, "ps"));
            assert!(listing.contains("pleroma"));
            let before = s.trace();
            let wrong = s.processes().iter().find(|p| !p.is_target).unwrap().pid;
            let fx = execute(&mut s, &format!("kill {wrong}"));
            assert!(text(&fx).contains("you killed something that mattered to someone."));
            assert!(
                (s.trace() - before - 13.0).abs() < 1e-3,
                "3 base + 10 penalty"
            );
            assert!(!s.objective_done());
            assert!(text(&execute(&mut s, &format!("kill {wrong}"))).contains("no such process"));
            let target = s.processes().iter().find(|p| p.is_target).unwrap().pid;
            let fx = execute(&mut s, &format!("kill {target}"));
            assert!(has(&fx, &ShellEffect::ObjectiveComplete));
        }
    }

    #[test]
    fn kill_away_from_core_is_refused() {
        let mut s = mk(ObjectiveKind::Crash, 3, None, 1);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let pid = s.processes()[0].pid;
        let fx = execute(&mut s, &format!("kill {pid}"));
        assert!(text(&fx).contains("no such process"));
        assert_eq!(s.trace(), 0.0);
        assert_eq!(s.processes().len(), 4);
    }

    #[test]
    fn trace_reaching_100_dumps_exactly_once() {
        let mut s = mk(ObjectiveKind::Retrieve, 1, None, 4);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let mut shocks = 0;
        let mut egos = Vec::new();
        for _ in 0..200 {
            for e in execute(&mut s, "ls") {
                shocks += usize::from(e == ShellEffect::DumpShock);
            }
            for e in s.tick(Duration::from_secs(1), false) {
                match e {
                    ShellEffect::DumpShock => shocks += 1,
                    ShellEffect::EgoDelta(d) => egos.push(d),
                    _ => {}
                }
            }
        }
        assert_eq!(shocks, 1);
        assert_eq!(egos, vec![-20]);
        assert!(s.is_dumped());
        assert_eq!(s.trace(), 100.0);
        assert!(execute(&mut s, "ls").is_empty());
        assert!(s.tick(Duration::from_secs(5), false).is_empty());
    }

    #[test]
    fn dump_during_get_aborts_the_objective() {
        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 5);
        go_to_objective(&mut s);
        s.trace = 97.0;
        let fx = execute(&mut s, "get WREN-01.LOG");
        assert!(has(&fx, &ShellEffect::DumpShock));
        assert!(!has(&fx, &ShellEffect::ObjectiveComplete));
        assert!(!s.objective_done());
    }

    #[test]
    fn time_trace_rate_scales_with_danger_minigame_and_ghost() {
        let rate = |rd: u8, mini: bool, ghost: bool| {
            let mut s = mk(ObjectiveKind::Retrieve, rd, None, 1);
            s.ghost = ghost;
            s.tick(Duration::from_secs(10), mini);
            s.trace()
        };
        assert!((rate(1, false, false) - 2.0).abs() < 1e-4);
        assert!((rate(5, false, false) - 6.0).abs() < 1e-4);
        assert!((rate(3, true, false) - 2.0).abs() < 1e-4);
        assert!((rate(3, false, true) - 2.4).abs() < 1e-4);
        assert!((rate(3, true, true) - 1.2).abs() < 1e-4);
    }

    #[test]
    fn run_ghost_spends_a_charge_and_slows_trace() {
        let mut s = mk(ObjectiveKind::Retrieve, 3, None, 1);
        let mut deck = crate::game::programs::Deck::new();
        crate::game::programs::add(&mut deck, Program::Ghost);
        let mut view = s.player().clone();
        view.deck = deck;
        s.set_player(view);
        let fx = execute(&mut s, "run ghost");
        assert!(has(&fx, &ShellEffect::UseProgram(Program::Ghost)));
        assert!(s.ghost_active());
        let fx = execute(&mut s, "run ghost.com");
        assert!(!has(&fx, &ShellEffect::UseProgram(Program::Ghost)));
        s.tick(Duration::from_secs(10), false);
        assert!((s.trace() - 2.4).abs() < 1e-4);
        assert!(text(&execute(&mut s, "run mandala")).contains("no charges left"));
        assert!(text(&execute(&mut s, "deck")).contains("PAYLOAD.PIF"));
    }

    #[test]
    fn mandala_restores_ego() {
        let mut s = mk(ObjectiveKind::Retrieve, 3, None, 1);
        let mut view = s.player().clone();
        crate::game::programs::add(&mut view.deck, Program::Mandala);
        s.set_player(view);
        let fx = execute(&mut s, "run mandala");
        assert!(has(&fx, &ShellEffect::EgoDelta(30)));
        assert!(has(&fx, &ShellEffect::UseProgram(Program::Mandala)));
        assert!(!s.ghost_active());
        assert!(text(&execute(&mut s, "deck")).lines().count() == 1);
    }

    #[test]
    fn bounce_is_single_use_and_cancel_does_not_burn_it() {
        let mut s = mk(ObjectiveKind::Retrieve, 3, None, 1);
        s.trace = 50.0;
        assert!(has(&execute(&mut s, "bounce"), &ShellEffect::Bounce));
        s.apply_bounce_outcome(IceOutcome::Aborted { trace: 0.0 });
        assert!(!s.bounce_used());
        assert!(has(&execute(&mut s, "bounce"), &ShellEffect::Bounce));
        s.apply_bounce_outcome(IceOutcome::Cleared);
        assert!((s.trace() - 20.0).abs() < 1e-4);
        assert!(s.bounce_used());
        let fx = execute(&mut s, "bounce");
        assert!(!has(&fx, &ShellEffect::Bounce));
        s.trace = 10.0;
        let mut t = mk(ObjectiveKind::Retrieve, 3, None, 1);
        t.trace = 10.0;
        t.apply_bounce_outcome(IceOutcome::Cleared);
        assert_eq!(t.trace(), 0.0, "floor at 0");
    }

    #[test]
    fn xyzzy_completes_only_in_dev_mode() {
        let mut s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        let fx = execute(&mut s, "xyzzy");
        assert_eq!(text(&fx), "Nothing happens.");
        assert!(!s.objective_done());
        let mut view = s.player().clone();
        view.dev = true;
        s.set_player(view);
        assert!(has(
            &execute(&mut s, "xyzzy"),
            &ShellEffect::ObjectiveComplete
        ));
        assert!(s.objective_done());
    }

    #[test]
    fn whoami_first_prints_shadow_then_corrects() {
        let mut s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        let first = text(&execute(&mut s, "whoami"));
        assert_eq!(first, "dez\nno. ZED");
        assert_eq!(text(&execute(&mut s, "whoami")), "ZED");
        assert_eq!(text(&execute(&mut s, "date")), "12/31/99 23:04");
    }

    #[test]
    fn prompts_per_os() {
        let want = [
            (HostOs::Unix, "portal0% ", PromptColor::LightGreen, None),
            (HostOs::Vax, "$ ", PromptColor::Yellow, None),
            (
                HostOs::Mainframe,
                "",
                PromptColor::WhiteOnBlue,
                Some("READY"),
            ),
            (HostOs::Dos, "C:\\PORTAL0>", PromptColor::LightGray, None),
        ];
        for (os, prompt, color, header) in want {
            let s = gen_with(&level(ObjectiveKind::Retrieve, None, None, os), 1, None, 1);
            assert_eq!(s.prompt(), (prompt.to_string(), color));
            assert_eq!(s.prompt_header(), header);
        }
    }

    #[test]
    fn scan_ls_cat_charge_half_a_point_and_output_fits_the_pane() {
        let mut s = mk(ObjectiveKind::Retrieve, 2, None, 6);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        assert!(has(&execute(&mut s, "scan"), &ShellEffect::Scan));
        execute(&mut s, "ls");
        assert!((s.trace() - 1.0).abs() < 1e-4);
        let node = s
            .nodes()
            .iter()
            .position(|n| n.files.iter().any(|f| !f.is_target))
            .unwrap();
        s.current = node;
        let name = s.nodes()[node]
            .files
            .iter()
            .find(|f| !f.is_target)
            .unwrap()
            .name
            .clone();
        let t = s.trace();
        let fx = execute(&mut s, &format!("cat {name}"));
        assert!(text(&fx).contains("body of"));
        assert!((s.trace() - t - 0.5).abs() < 1e-4);
        for cmd in ["help", "ls", "scan", "frobnicate", "deck"] {
            for e in execute(&mut s, cmd) {
                if let ShellEffect::Print { text, .. } = e {
                    assert!(text.chars().count() <= SHELL_COLS, "{cmd}: {text}");
                }
            }
        }
    }

    #[test]
    fn wrap_breaks_long_words_and_keeps_newlines() {
        let lines = wrap_lines(&format!("ab {}\ncd", "x".repeat(60)), 54);
        assert!(lines.iter().all(|l| l.chars().count() <= 54));
        assert_eq!(lines.last().unwrap(), "cd");
        assert_eq!(lines[0], "ab");
    }

    #[test]
    fn ice_outcomes_adjust_trace_ego_and_position() {
        let mut s = mk(ObjectiveKind::Retrieve, 4, None, 2);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let adj = s.nodes()[0].neighbors[0];
        let fx = s.apply_ice_outcome(
            adj,
            IceOutcome::Failed {
                trace: 15.0,
                ego: 5,
            },
        );
        assert!(has(&fx, &ShellEffect::EgoDelta(-5)));
        assert_eq!(s.current(), 0);
        assert!((s.trace() - 15.0).abs() < 1e-4);
        s.apply_ice_outcome(adj, IceOutcome::Aborted { trace: 5.0 });
        s.apply_ice_outcome(adj, IceOutcome::Retreated);
        assert!((s.trace() - 30.0).abs() < 1e-4);
        assert!(!s.nodes()[adj].cleared);
        let fx = s.apply_ice_outcome(adj, IceOutcome::Integrated);
        assert!(has(&fx, &ShellEffect::IntegrationDelta(2)));
        assert_eq!(s.current(), adj);
        let fx = s.apply_ice_outcome(0, IceOutcome::Destroyed);
        assert!(has(&fx, &ShellEffect::IntegrationDelta(-1)));
        assert!(text(&fx).contains("you cut it away. it grows back."));
    }

    #[test]
    fn map_layout_gives_bfs_layers_and_rows() {
        for (seed, rd, exit, kind) in all_cases().into_iter().take(200) {
            let s = mk(kind, rd, exit, seed);
            let map = s.map_layout();
            assert_eq!(map.len(), s.nodes().len());
            assert_eq!((map[0].layer, map[0].row), (0, 0));
            assert!(map[0].is_current);
            for m in &map {
                for &j in &m.neighbors {
                    assert!(map[j].layer.abs_diff(m.layer) <= 1);
                }
                let same: Vec<_> = map.iter().filter(|o| o.layer == m.layer).collect();
                assert!(same.iter().filter(|o| o.row == m.row).count() == 1);
            }
        }
    }

    fn edit(ed: &mut LineEditor, s: &HostSession, keys: &[EditKey]) -> Vec<Option<String>> {
        keys.iter().map(|&k| ed.handle(k, s)).collect()
    }

    fn typed(t: &str) -> Vec<EditKey> {
        t.chars().map(EditKey::Char).collect()
    }

    #[test]
    fn editor_cursor_editing() {
        let s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        let mut ed = LineEditor::new();
        edit(&mut ed, &s, &typed("lsx"));
        edit(&mut ed, &s, &[EditKey::Left, EditKey::Backspace]);
        assert_eq!((ed.text(), ed.cursor()), ("lx", 1));
        edit(&mut ed, &s, &[EditKey::Delete]);
        assert_eq!(ed.text(), "l");
        edit(
            &mut ed,
            &s,
            &[
                EditKey::Home,
                EditKey::Char('c'),
                EditKey::End,
                EditKey::Char('s'),
            ],
        );
        assert_eq!(ed.text(), "cls");
        edit(
            &mut ed,
            &s,
            &[EditKey::Home, EditKey::Backspace, EditKey::Left],
        );
        assert_eq!((ed.text(), ed.cursor()), ("cls", 0));
        let out = edit(&mut ed, &s, &[EditKey::Enter]);
        assert_eq!(out, vec![Some("cls".to_string())]);
        assert_eq!(ed.text(), "");
        assert_eq!(
            ed.handle(EditKey::ClearScreen, &s),
            Some("clear".to_string())
        );
    }

    #[test]
    fn editor_history_walks_and_caps_at_fifty() {
        let s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        let mut ed = LineEditor::new();
        for i in 0..60 {
            edit(&mut ed, &s, &typed(&format!("ls {i}")));
            ed.handle(EditKey::Enter, &s);
        }
        assert_eq!(ed.history().len(), 50);
        assert_eq!(ed.history()[0], "ls 10");
        edit(&mut ed, &s, &typed("dra"));
        ed.handle(EditKey::Up, &s);
        assert_eq!(ed.text(), "ls 59");
        ed.handle(EditKey::Up, &s);
        assert_eq!(ed.text(), "ls 58");
        ed.handle(EditKey::Down, &s);
        ed.handle(EditKey::Down, &s);
        assert_eq!(ed.text(), "dra", "draft restored");
        assert_eq!(ed.cursor(), 3);
        for _ in 0..70 {
            ed.handle(EditKey::Up, &s);
        }
        assert_eq!(ed.text(), "ls 10");
    }

    #[test]
    fn editor_tab_cycles_commands_nodes_and_files() {
        let mut s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        s.apply_ice_outcome(0, IceOutcome::Cleared);
        let mut ed = LineEditor::new();
        edit(&mut ed, &s, &typed("c"));
        let mut seen = Vec::new();
        for _ in 0..4 {
            ed.handle(EditKey::Tab, &s);
            seen.push(ed.text().to_string());
        }
        assert_eq!(seen, ["cat", "cd", "clear", "connect"]);

        ed.clear();
        edit(&mut ed, &s, &typed("connect g"));
        ed.handle(EditKey::Tab, &s);
        assert!(ed.text().starts_with("connect gate"), "{}", ed.text());
        let first = ed.text().to_string();
        let gates = s
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Gate)
            .count();
        for _ in 0..gates {
            ed.handle(EditKey::Tab, &s);
        }
        assert_eq!(ed.text(), first, "cycle wraps");
        assert_eq!(ed.cursor(), ed.text().chars().count());

        // File names at the current node.
        let node = s.nodes().iter().position(|n| !n.files.is_empty()).unwrap();
        s.current = node;
        let name = s.nodes()[node].files[0].name.clone();
        ed.clear();
        edit(&mut ed, &s, &typed("cat "));
        edit(&mut ed, &s, &typed(&name[..1].to_lowercase()));
        ed.handle(EditKey::Tab, &s);
        assert_eq!(ed.text(), format!("cat {name}"));

        // Typing after a completion ends the cycle; run completes program names.
        ed.clear();
        edit(&mut ed, &s, &typed("run g"));
        ed.handle(EditKey::Tab, &s);
        assert_eq!(ed.text(), "run ghost");
        // No match leaves the buffer alone.
        ed.clear();
        edit(&mut ed, &s, &typed("zzz"));
        ed.handle(EditKey::Tab, &s);
        assert_eq!(ed.text(), "zzz");
    }

    #[test]
    fn unknown_command_flavour_follows_the_os() {
        for (os, needle) in [
            (HostOs::Unix, "blorp: command not found"),
            (HostOs::Vax, "%DCL-W-IVVERB"),
            (HostOs::Mainframe, "?SYNTAX ERROR"),
            (HostOs::Dos, "Bad command or file name"),
        ] {
            let mut s = gen_with(&level(ObjectiveKind::Retrieve, None, None, os), 1, None, 1);
            assert!(text(&execute(&mut s, "blorp")).contains(needle));
        }
        let mut s = mk(ObjectiveKind::Retrieve, 1, None, 1);
        assert!(execute(&mut s, "   ").is_empty());
    }
}
