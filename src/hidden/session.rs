//! The hidden line's line protocol: pure, socket-free and deterministic.
//!
//! [`Session`] turns one command line into a [`Reply`]: the text to write, optional terminal text
//! (ANSI art redrawn row by row) to stream at modem speed, run events for the app, and whether to
//! hang up. The listener thread in
//! [`super`] owns all I/O, so every rule here is unit-tested without threads.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::ans;
use crate::game::programs::Program;
use crate::game::state::{BACKDOOR_MISSES, Backdoor};
use crate::ice::phreak::dtmf;
use crate::pack::model::FileDef;

use super::HiddenEvent;
use super::board::{BoardSnapshot, ZineEntry, ZineKind};

const UNKNOWN_FILE: &str = "*** no such file. DIR lists them.\n";

/// What the next line means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A command line.
    Prompt,
    /// A TAP attempt.
    Tap,
}

/// One answer to one input line.
#[derive(Debug, Default)]
pub struct Reply {
    /// Plain text, every line newline-terminated.
    pub text: String,
    /// Terminal text to send at modem speed after `text`.
    pub stream: Option<String>,
    pub events: Vec<HiddenEvent>,
    /// Hang up after this reply.
    pub close: bool,
}

/// One connected caller's state.
#[derive(Debug)]
pub struct Session {
    board: Arc<BoardSnapshot>,
    generation: u64,
    mode: Mode,
    backdoor: Backdoor,
    owned: BTreeSet<String>,
    learned: BTreeSet<Program>,
}

impl Session {
    /// A session on `board`; returns it and the message of the day.
    pub fn open(board: Arc<BoardSnapshot>, generation: u64) -> (Session, String) {
        let backdoor = board.backdoor;
        let mut session = Session {
            board,
            generation,
            mode: Mode::Prompt,
            backdoor,
            owned: BTreeSet::new(),
            learned: BTreeSet::new(),
        };
        session.load_state();
        let motd = session.motd();
        (session, motd)
    }

    /// Swap in a different board (a new generation): reset the mode and the snapshot-derived
    /// state. Returns the carrier-change notice followed by the new message of the day.
    pub fn reload(&mut self, board: Arc<BoardSnapshot>, generation: u64) -> String {
        self.board = board;
        self.generation = generation;
        self.mode = Mode::Prompt;
        self.load_state();
        let bbs = &self.board.bbs_name;
        let depth = self.board.depth;
        let head = format!("*** CARRIER CHANGED. the board moved on: {bbs} (level {depth:02}).\n");
        format!("{head}{}", self.motd())
    }

    /// The generation of the board this session is in sync with.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The prompt for the current mode.
    pub fn prompt(&self) -> &'static str {
        match self.mode {
            Mode::Prompt => "> ",
            Mode::Tap => "keys> ",
        }
    }

    /// Handle one input line.
    pub fn line(&mut self, input: &str) -> Reply {
        let trimmed = input.trim();
        if self.mode == Mode::Tap {
            return self.tap_attempt(trimmed);
        }
        if trimmed.is_empty() {
            return Reply::default();
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default().to_ascii_uppercase();
        let arg = parts.next().unwrap_or_default().trim();
        match command.as_str() {
            "HELP" => text(&self.help()),
            "INFO" => text(&self.info()),
            "DIR" | "LS" => text(&self.dir()),
            "TYPE" | "CAT" => self.view(arg),
            "GET" => self.get(arg),
            "TAP" => self.tap(),
            "QUIT" | "BYE" | "EXIT" => Reply {
                text: "NO CARRIER\n".to_string(),
                close: true,
                ..Reply::default()
            },
            _ => text("? unknown command. HELP lists them.\n"),
        }
    }

    // ----- output ---------------------------------------------------------------------

    fn load_state(&mut self) {
        self.owned = self
            .board
            .files
            .iter()
            .filter(|f| f.owned)
            .map(|f| f.name.clone())
            .collect();
        self.learned = self
            .board
            .files
            .iter()
            .filter_map(|f| match &f.kind {
                ZineKind::Article { attack, .. } if f.learned => Some(*attack),
                _ => None,
            })
            .collect();
        self.backdoor = self.board.backdoor;
    }

    fn banner(&self) -> Vec<String> {
        let board = &self.board;
        let relay = match &board.relay {
            Some(relay) => format!(
                "relay   : {} [{}] ({})",
                relay.id,
                relay.display,
                relay.grade.label()
            ),
            None => "relay   : none on this exchange".to_string(),
        };
        let backdoor = match self.backdoor {
            Backdoor::Open { misses } => format!("open ({misses} miss)"),
            Backdoor::Planted => "planted".to_string(),
            Backdoor::Locked => "locked".to_string(),
        };
        let os = board.os.label();
        vec![
            "ABRAXAS HIDDEN LINE".to_string(),
            format!(
                "board   : {} / {}  (level {:02})",
                board.bbs_name, board.title, board.depth
            ),
            format!("caller  : {}   sysop: {}", board.handle, board.sysop),
            format!("exchange: {} [{os}]", board.host_name),
            relay,
            format!("backdoor: {backdoor}"),
        ]
    }

    fn info(&self) -> String {
        let mut out = self.banner().join("\n");
        out.push('\n');
        out
    }

    fn motd(&self) -> String {
        let mut out = self.info();
        out.push_str("HELP lists commands.\n");
        out
    }

    fn help(&self) -> String {
        [
            "HELP        this list",
            "INFO        the board and its exchange",
            "DIR         the zine directory (LS)",
            "TYPE NAME   view a file at modem speed; reading an article teaches its attack (CAT)",
            "GET NAME    download a file to your run (/files on IRC)",
            "TAP         decode the relay's trunk signalling and plant a backdoor",
            "QUIT        hang up (BYE, EXIT)",
        ]
        .join("\n")
            + "\n"
    }

    fn dir(&self) -> String {
        let mut out = String::from(" NAME          SIZE  KIND  DESCRIPTION\n");
        for entry in &self.board.files {
            let (size, kind) = match &entry.kind {
                ZineKind::Ans { text } => (text.chars().count(), "ANS"),
                ZineKind::Article { body, .. } => (body.chars().count(), "ZINE"),
            };
            let marker = match &entry.kind {
                ZineKind::Article { attack, .. } if self.learned.contains(attack) => " (learned)",
                _ if self.is_owned(&entry.name) => " (yours)",
                _ => "",
            };
            out.push_str(&format!(
                " {:<12} {:>5}  {:<4}  {}{}\n",
                entry.name, size, kind, entry.description, marker
            ));
        }
        let count = self.board.files.len();
        out.push_str(&format!(
            "{count} file(s). TYPE <name> to view, GET <name> to download.\n"
        ));
        out
    }

    fn entry(&self, name: &str) -> Option<&ZineEntry> {
        if name.is_empty() {
            return None;
        }
        self.board
            .files
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    fn is_owned(&self, name: &str) -> bool {
        self.owned.iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    fn view(&mut self, name: &str) -> Reply {
        let Some(entry) = self.entry(name) else {
            return text(UNKNOWN_FILE);
        };
        let file = entry.name.clone();
        let (out, stream, learned_now) = match &entry.kind {
            ZineKind::Ans { text: body } => {
                let header = format!(
                    "*** {file} ({} bytes) at modem speed. any line skips.\n",
                    body.chars().count()
                );
                // The art is redrawn through the same 80-column interpreter as the in-game
                // viewer and sent as explicit rows, so it looks right in a terminal of any
                // width and needs no ANSI.SYS wrapping or iCE blink handling from the client.
                let ice = file.to_ascii_uppercase().ends_with(".ICE");
                let rows = ans::to_terminal(body, ice);
                (header, Some(format!("\u{1b}[0m\r\n{rows}")), None)
            }
            ZineKind::Article { body, attack } => {
                if self.learned.contains(attack) {
                    let mut out = with_newline(body);
                    out.push_str("*** you know this one.\n");
                    (out, None, None)
                } else {
                    let mut out = with_newline(body);
                    let label = attack
                        .attack()
                        .map_or_else(|| attack.file_name().to_string(), |a| a.label().to_string());
                    out.push_str(&format!("*** you learned {label}. {}\n", attack.effect()));
                    (out, None, Some(*attack))
                }
            }
        };
        let mut events = Vec::new();
        if let Some(attack) = learned_now {
            self.learned.insert(attack);
            events.push(HiddenEvent::Learn(attack));
        }
        Reply {
            text: out,
            stream,
            events,
            close: false,
        }
    }

    fn get(&mut self, name: &str) -> Reply {
        let Some(entry) = self.entry(name) else {
            return text(UNKNOWN_FILE);
        };
        if self.is_owned(&entry.name) {
            let file = &entry.name;
            return text(&format!("*** you already have {file}.\n"));
        }
        let name = entry.name.clone();
        let description = entry.description.clone();
        let body = match &entry.kind {
            ZineKind::Ans { text } => text.clone(),
            ZineKind::Article { body, .. } => body.clone(),
        };
        let size = body.chars().count();
        self.owned.insert(name.clone());
        Reply {
            text: format!(
                "*** {name} saved to your files ({size} bytes). /files on IRC lists it.\n"
            ),
            stream: None,
            events: vec![HiddenEvent::Download {
                depth: self.board.depth,
                file: FileDef {
                    name,
                    description,
                    body,
                },
            }],
            close: false,
        }
    }

    fn tap(&mut self) -> Reply {
        let Some(relay) = self.board.relay.as_ref() else {
            return text("*** no relay on this exchange. nothing to tap.\n");
        };
        let misses = match self.backdoor {
            Backdoor::Open { misses } => misses,
            Backdoor::Planted => {
                return text("*** the backdoor is already in place.\n");
            }
            Backdoor::Locked => {
                return text("*** line dead. the exchange heard you. no backdoor on this board.\n");
            }
        };
        self.mode = Mode::Tap;
        let mut out = String::new();
        out.push_str(&format!(
            "*** tapping {} [{}] ({})...\n",
            relay.id,
            relay.display,
            relay.grade.label()
        ));
        out.push_str(
            "the relay signals its sequence. decode each tone pair to a key and type the keys in order.\n",
        );
        out.push_str("      1209 1336 1477\n");
        out.push_str(" 697    1    2    3\n");
        out.push_str(" 770    4    5    6\n");
        out.push_str(" 852    7    8    9\n");
        out.push_str(" 941    *    0    #\n");
        out.push_str("tones:\n");
        for (i, &key) in relay.tones.iter().enumerate() {
            if let Some((lo, hi)) = dtmf(key) {
                out.push_str(&format!("  {}. {lo} Hz + {hi} Hz\n", i + 1));
            }
        }
        let left = BACKDOOR_MISSES.saturating_sub(misses);
        out.push_str(&format!("{left} attempt(s) left. an empty line cancels.\n"));
        text(&out)
    }

    fn tap_attempt(&mut self, input: &str) -> Reply {
        let attempt: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        if attempt.is_empty() {
            self.mode = Mode::Prompt;
            return text("*** tap cancelled.\n");
        }
        let Some(relay) = self.board.relay.as_ref() else {
            self.mode = Mode::Prompt;
            return text("*** no relay on this exchange. nothing to tap.\n");
        };
        let expected: String = relay.tones.iter().copied().collect();
        if attempt.eq_ignore_ascii_case(&expected) {
            let id = relay.id.clone();
            self.backdoor = Backdoor::Planted;
            self.mode = Mode::Prompt;
            return Reply {
                text: format!("*** BACKDOOR PLANTED. {id} will not challenge you.\n"),
                events: vec![HiddenEvent::Backdoor {
                    depth: self.board.depth,
                    state: Backdoor::Planted,
                }],
                ..Reply::default()
            };
        }
        let misses = match self.backdoor {
            Backdoor::Open { misses } => misses.saturating_add(1),
            _ => 1,
        };
        if misses >= BACKDOOR_MISSES {
            self.backdoor = Backdoor::Locked;
            self.mode = Mode::Prompt;
            Reply {
                text: "*** LINE DROPPED. the exchange heard you. no backdoor on this board.\n"
                    .to_string(),
                events: vec![HiddenEvent::Backdoor {
                    depth: self.board.depth,
                    state: Backdoor::Locked,
                }],
                ..Reply::default()
            }
        } else {
            self.backdoor = Backdoor::Open { misses };
            Reply {
                text: format!("*** wrong. {} attempt(s) left.\n", BACKDOOR_MISSES - misses),
                events: vec![HiddenEvent::Backdoor {
                    depth: self.board.depth,
                    state: Backdoor::Open { misses },
                }],
                ..Reply::default()
            }
        }
    }
}

/// The `reply.text` shorthand.
fn text(body: &str) -> Reply {
    Reply {
        text: body.to_string(),
        ..Reply::default()
    }
}

fn with_newline(body: &str) -> String {
    let mut out = String::from(body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::IceGrade;
    use crate::hidden::board::Relay;
    use crate::pack::model::HostOs;

    fn snapshot(backdoor: Backdoor) -> Arc<BoardSnapshot> {
        Arc::new(BoardSnapshot {
            depth: 2,
            handle: "Tester".to_string(),
            bbs_name: "BAUDLINE".to_string(),
            title: "the quiet board".to_string(),
            sysop: "TESTSYS".to_string(),
            host_name: "HOST".to_string(),
            os: HostOs::Unix,
            relay: Some(Relay {
                id: "relay1".to_string(),
                display: "THE RELAY".to_string(),
                grade: IceGrade::Soft,
                tones: vec!['1', '2', '*'],
            }),
            backdoor,
            files: vec![
                ZineEntry {
                    name: "DEMO1.ANS".to_string(),
                    description: "inline art".to_string(),
                    kind: ZineKind::Ans {
                        text: "\u{1b}[0;1;33mRED\u{1b}[0m\r\nplain".to_string(),
                    },
                    owned: false,
                    learned: false,
                },
                ZineEntry {
                    name: "PACK.ANS".to_string(),
                    description: "an artpack file".to_string(),
                    kind: ZineKind::Ans {
                        text: "\u{1b}[31mhi\r\n".to_string(),
                    },
                    owned: false,
                    learned: false,
                },
                ZineEntry {
                    name: "WINNUKE.TXT".to_string(),
                    description: "out of band".to_string(),
                    kind: ZineKind::Article {
                        body: "the wire is yours.".to_string(),
                        attack: Program::WinNuke,
                    },
                    owned: false,
                    learned: false,
                },
            ],
        })
    }

    fn open() -> (Session, String) {
        Session::open(snapshot(Backdoor::Open { misses: 0 }), 7)
    }

    #[test]
    fn motd_names_the_board_and_the_exchange() {
        let (_, motd) = open();
        assert!(motd.starts_with("ABRAXAS HIDDEN LINE\n"), "{motd}");
        assert!(
            motd.contains("board   : BAUDLINE / the quiet board  (level 02)"),
            "{motd}"
        );
        assert!(motd.contains("caller  : Tester   sysop: TESTSYS"), "{motd}");
        assert!(motd.contains("exchange: HOST [unix]"), "{motd}");
        assert!(
            motd.contains("relay   : relay1 [THE RELAY] (soft)"),
            "{motd}"
        );
        assert!(motd.contains("backdoor: open (0 miss)"), "{motd}");
        assert!(motd.ends_with("HELP lists commands.\n"), "{motd}");
    }

    #[test]
    fn info_omits_the_help_hint() {
        let (mut session, _) = open();
        let reply = session.line("INFO");
        assert!(reply.text.contains("ABRAXAS HIDDEN LINE"));
        assert!(!reply.text.contains("HELP lists commands."));
    }

    #[test]
    fn dir_lists_every_kind_with_markers() {
        let (mut session, _) = open();
        let reply = session.line("DIR");
        assert!(
            reply
                .text
                .starts_with(" NAME          SIZE  KIND  DESCRIPTION\n")
        );
        assert!(reply.text.contains(" DEMO1.ANS"), "{}", reply.text);
        assert!(reply.text.contains("ANS "), "{}", reply.text);
        assert!(reply.text.contains("ZINE"), "{}", reply.text);
        assert!(reply.text.contains("3 file(s)."), "{}", reply.text);
        // Download an ANS entry and learn the article, then look again.
        session.line("GET PACK.ANS");
        session.line("TYPE WINNUKE.TXT");
        let reply = session.line("LS");
        assert!(reply.text.contains("PACK.ANS"), "{}", reply.text);
        assert!(reply.text.contains("(yours)"), "{}", reply.text);
        assert!(reply.text.contains("(learned)"), "{}", reply.text);
    }

    #[test]
    fn type_of_an_ans_file_streams_explicit_rows() {
        let (mut session, _) = open();
        let reply = session.line("TYPE PACK.ANS");
        assert!(reply.text.contains("*** PACK.ANS"));
        assert!(reply.text.contains("at modem speed"));
        let stream = reply.stream.expect("ANSI is streamed");
        assert_eq!(
            stream, "\u{1b}[0m\r\n\u{1b}[38;5;1;48;5;0mhi\u{1b}[0m\r\n",
            "red on black, one explicit row"
        );
    }

    #[test]
    fn reading_an_article_learns_its_attack_once() {
        let (mut session, _) = open();
        let first = session.line("TYPE WINNUKE.TXT");
        assert!(
            first.text.contains("you learned WINNUKE."),
            "{}",
            first.text
        );
        assert_eq!(first.events, vec![HiddenEvent::Learn(Program::WinNuke)]);
        let second = session.line("TYPE WINNUKE.TXT");
        assert!(
            second.text.contains("you know this one."),
            "{}",
            second.text
        );
        assert!(second.events.is_empty());
    }

    #[test]
    fn get_downloads_the_decoded_body() {
        let (mut session, _) = open();
        let reply = session.line("GET PACK.ANS");
        assert!(reply.text.contains("saved to your files"), "{}", reply.text);
        let events = reply.events;
        assert_eq!(events.len(), 1);
        match &events[0] {
            HiddenEvent::Download { depth, file } => {
                assert_eq!(*depth, 2);
                assert_eq!(file.name, "PACK.ANS");
                assert_eq!(file.body, "\u{1b}[31mhi\r\n");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        // A second GET of the same file does nothing.
        let again = session.line("GET PACK.ANS");
        assert!(again.text.contains("already have"), "{}", again.text);
        assert!(again.events.is_empty());
    }

    #[test]
    fn get_reports_the_article_body() {
        let (mut session, _) = open();
        let reply = session.line("GET WINNUKE.TXT");
        match &reply.events[0] {
            HiddenEvent::Download { file, .. } => {
                assert_eq!(file.name, "WINNUKE.TXT");
                assert_eq!(file.body, "the wire is yours.");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tap_plants_the_backdoor_on_the_right_tones() {
        let (mut session, _) = open();
        let reply = session.line("TAP");
        assert!(
            reply
                .text
                .contains("*** tapping relay1 [THE RELAY] (soft)...")
        );
        assert!(reply.text.contains("tones:"));
        assert!(reply.text.contains("  1. 697 Hz + 1209 Hz"));
        assert!(reply.text.contains("  3. 941 Hz + 1209 Hz"));
        assert!(reply.text.contains("3 attempt(s) left."));
        assert_eq!(session.prompt(), "keys> ");
        let reply = session.line("1 2 *");
        assert!(reply.text.contains("*** BACKDOOR PLANTED."));
        assert_eq!(
            reply.events,
            vec![HiddenEvent::Backdoor {
                depth: 2,
                state: Backdoor::Planted,
            }]
        );
        assert_eq!(session.prompt(), "> ");
    }

    #[test]
    fn three_wrong_attempts_lock_the_backdoor() {
        let (mut session, _) = open();
        session.line("TAP");
        let first = session.line("9");
        assert!(first.text.contains("*** wrong. 2 attempt(s) left."));
        assert_eq!(
            first.events,
            vec![HiddenEvent::Backdoor {
                depth: 2,
                state: Backdoor::Open { misses: 1 },
            }]
        );
        let second = session.line("8");
        assert!(second.text.contains("*** wrong. 1 attempt(s) left."));
        assert_eq!(
            second.events,
            vec![HiddenEvent::Backdoor {
                depth: 2,
                state: Backdoor::Open { misses: 2 },
            }]
        );
        let third = session.line("7");
        assert!(third.text.contains("*** LINE DROPPED."));
        assert_eq!(
            third.events,
            vec![HiddenEvent::Backdoor {
                depth: 2,
                state: Backdoor::Locked,
            }]
        );
        assert_eq!(session.prompt(), "> ");
    }

    #[test]
    fn tap_cancels_on_an_empty_line() {
        let (mut session, _) = open();
        session.line("TAP");
        let reply = session.line("   ");
        assert!(reply.text.contains("*** tap cancelled."));
        assert_eq!(session.prompt(), "> ");
    }

    #[test]
    fn tap_refuses_without_a_relay() {
        let mut board = (*snapshot(Backdoor::default())).clone();
        board.relay = None;
        let (mut session, _) = Session::open(Arc::new(board), 1);
        let reply = session.line("TAP");
        assert!(reply.text.contains("no relay on this exchange"));
        assert_eq!(session.prompt(), "> ");
    }

    #[test]
    fn tap_refuses_a_planted_or_locked_backdoor() {
        let (mut planted, _) = Session::open(snapshot(Backdoor::Planted), 1);
        assert!(planted.line("TAP").text.contains("already in place"));
        let (mut locked, _) = Session::open(snapshot(Backdoor::Locked), 1);
        assert!(locked.line("TAP").text.contains("line dead"));
    }

    #[test]
    fn quit_closes_and_unknown_commands_say_so() {
        let (mut session, _) = open();
        let quit = session.line("quit");
        assert!(quit.close);
        assert_eq!(quit.text, "NO CARRIER\n");
        let (mut session, _) = open();
        let reply = session.line("FROB");
        assert_eq!(reply.text, "? unknown command. HELP lists them.\n");
        assert!(session.line("").text.is_empty());
        assert!(session.line("TYPE NOPE.ANS").text.contains("no such file"));
    }

    #[test]
    fn reload_resets_the_mode_and_announces_the_change() {
        let (mut session, _) = open();
        session.line("TAP");
        assert_eq!(session.prompt(), "keys> ");
        let mut next = (*snapshot(Backdoor::Planted)).clone();
        next.depth = 3;
        next.bbs_name = "OTHERBOARD".to_string();
        let message = session.reload(Arc::new(next), 8);
        assert!(
            message.starts_with("*** CARRIER CHANGED. the board moved on: OTHERBOARD (level 03)."),
            "{message}"
        );
        assert!(message.contains("backdoor: planted"), "{message}");
        assert_eq!(session.generation(), 8);
        assert_eq!(session.prompt(), "> ");
    }

    #[test]
    fn owned_files_from_the_snapshot_are_already_owned() {
        let mut board = (*snapshot(Backdoor::default())).clone();
        board.files[0].owned = true;
        let (mut session, _) = Session::open(Arc::new(board), 1);
        let reply = session.line("GET DEMO1.ANS");
        assert!(reply.text.contains("already have"));
        assert!(reply.events.is_empty());
        // And it is listed as owned.
        assert!(session.line("DIR").text.contains("(yours)"));
    }
}
