//! Chat: page the level's sysop and talk. Replies stream in through the director (which answers
//! with ELIZA itself when no model is available); a local timeout falls back to ELIZA too, so a
//! chat can never hang.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::beat::draw_lines;
use super::{Nav, ScreenImpl};
use crate::director::offline::eliza_reply;
use crate::director::{ChatLine, ChatPartner, DirectorEvent};
use crate::palette::Dos;
use crate::text::{to_cp437_safe, wrap};
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::input::{InputEvent, TextInput};

/// Seconds of `Paging SysOp...`.
const PAGING_SECS: f32 = 1.5;
/// Player messages after which the sysop leaves.
pub const MAX_MESSAGES: usize = 12;
/// Longest player message.
const MAX_INPUT: usize = 70;
/// Extra seconds on top of `ai.timeout_secs` before the local ELIZA fallback.
const TIMEOUT_GRACE: f32 = 5.0;

const SYSOP_W: usize = 48;
const SYSOP_ROWS: usize = 11;
const PLAYER_W: usize = 76;
const PLAYER_ROWS: usize = 10;
const AVATAR_X: u16 = 51;
const AVATAR_W: u16 = 29;

/// A generic terminal block for NPC sysops.
const NPC_ART: [&str; 7] = [
    "   ╔═══════════════╗",
    "   ║ ┌───────────┐ ║",
    "   ║ │ > _       │ ║",
    "   ║ │           │ ║",
    "   ║ └───────────┘ ║",
    "   ╚═══╤═══════╤═══╝",
    "    ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄",
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Paging(f32),
    Open,
    /// A reply to `session` is outstanding; `waited` seconds so far.
    Waiting {
        session: u64,
        waited: f32,
    },
    /// The sysop left; any key returns.
    Left,
}

/// The Chat screen.
pub struct ChatScreen {
    partner: ChatPartner,
    phase: Phase,
    history: Vec<ChatLine>,
    sent: usize,
    input: TextInput,
    /// Wrapped rows of the sysop pane / player pane (rebuilt when the history changes).
    sysop_rows: Vec<String>,
    player_rows: Vec<String>,
    art: Vec<Line<'static>>,
    /// The reply being streamed (shown as the last sysop text until `ChatDone`).
    streaming: String,
    handle: String,
}

impl ChatScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        ctx.chat_inbox.clear();
        let partner = ctx.place().map_or_else(
            || ChatPartner {
                handle: "SYSOP".to_string(),
                cast_id: None,
            },
            |p| ChatPartner {
                handle: p.sysop.handle.clone(),
                cast_id: p.sysop.cast_id.clone(),
            },
        );
        let art = match partner
            .cast_id
            .as_deref()
            .and_then(|id| ctx.pack.character(id))
            .and_then(|c| c.art.as_deref())
            .and_then(|name| ctx.pack.art(name))
        {
            Some(a) => ctx.palette.pipe_lines(a),
            None => NPC_ART
                .iter()
                .map(|l| Line::styled((*l).to_string(), ctx.fg(Dos::DarkGray)))
                .collect(),
        };
        let handle = ctx.run_ref().map(|r| r.handle.clone()).unwrap_or_default();
        Self {
            partner,
            phase: Phase::Paging(PAGING_SECS),
            history: Vec::new(),
            sent: 0,
            input: TextInput::new("> ", MAX_INPUT),
            sysop_rows: Vec::new(),
            player_rows: Vec::new(),
            art,
            streaming: String::new(),
            handle,
        }
    }

    fn rebuild(&mut self) {
        let mut sysop = Vec::new();
        let mut player = Vec::new();
        for l in &self.history {
            if l.from_player {
                player.extend(
                    wrap(&l.text, PLAYER_W - 2)
                        .into_iter()
                        .map(|r| format!("> {r}")),
                );
            } else {
                if !sysop.is_empty() {
                    sysop.push(String::new());
                }
                sysop.extend(wrap(&l.text, SYSOP_W));
            }
        }
        if !self.streaming.is_empty() {
            if !sysop.is_empty() {
                sysop.push(String::new());
            }
            sysop.extend(wrap(&self.streaming, SYSOP_W));
        }
        self.sysop_rows = sysop;
        self.player_rows = player;
    }

    fn timeout_secs(ctx: &Ctx) -> f32 {
        ctx.config.ai.timeout_secs as f32 + TIMEOUT_GRACE
    }

    fn finish_reply(&mut self, text: String) {
        self.streaming.clear();
        self.history.push(ChatLine {
            from_player: false,
            text,
        });
        if self.sent >= MAX_MESSAGES {
            self.history.push(ChatLine {
                from_player: false,
                text: format!("{} has left chat.", self.partner.handle),
            });
            self.phase = Phase::Left;
        } else {
            self.phase = Phase::Open;
        }
        self.rebuild();
    }

    fn send(&mut self, text: String, ctx: &mut Ctx) {
        self.history.push(ChatLine {
            from_player: true,
            text,
        });
        self.sent += 1;
        let session = match ctx.run.as_ref() {
            Some(run) => ctx.director.request_chat(run, &self.partner, &self.history),
            None => 0,
        };
        self.phase = Phase::Waiting {
            session,
            waited: 0.0,
        };
        self.rebuild();
    }

    fn local_reply(&self, ctx: &Ctx) -> String {
        let character = self
            .partner
            .cast_id
            .as_deref()
            .and_then(|id| ctx.pack.character(id));
        let last = self
            .history
            .iter()
            .rev()
            .find(|l| l.from_player)
            .map_or("", |l| l.text.as_str());
        eliza_reply(&ctx.pack, character, last, self.sent.saturating_sub(1))
    }

    fn consume_inbox(&mut self, ctx: &mut Ctx) {
        let events = std::mem::take(&mut ctx.chat_inbox);
        for ev in events {
            let Phase::Waiting { session, .. } = self.phase else {
                continue;
            };
            match ev {
                DirectorEvent::ChatDelta { session: s, text } if s == session => {
                    self.streaming.push_str(&to_cp437_safe(&text));
                    self.rebuild();
                }
                DirectorEvent::ChatDone {
                    session: s, text, ..
                } if s == session => {
                    let text = to_cp437_safe(&text);
                    let text = if text.trim().is_empty() {
                        self.local_reply(ctx)
                    } else {
                        text
                    };
                    self.finish_reply(text);
                }
                _ => {}
            }
        }
    }

    fn title_bar(&self) -> String {
        let left = format!("─[ {} ]────", self.partner.handle);
        let mid = " SysOp Chat ";
        let right = " Esc ends ─";
        let fill = 80usize
            .saturating_sub(left.chars().count() + mid.chars().count() + right.chars().count());
        format!("{left}{mid}{}{right}", "─".repeat(fill))
    }
}

impl ScreenImpl for ChatScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if matches!(classify(&key), Key::Esc) {
            return Nav::Back;
        }
        match self.phase {
            Phase::Left => return Nav::Back,
            Phase::Open => {}
            Phase::Paging(_) | Phase::Waiting { .. } => return Nav::Stay,
        }
        if let InputEvent::Submit(text) = self.input.handle_key(&key) {
            let text = text.trim().to_string();
            self.input.clear();
            if !text.is_empty() {
                self.send(text, ctx);
            }
        }
        Nav::Stay
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        let secs = dt.as_secs_f32();
        match &mut self.phase {
            Phase::Paging(left) => {
                *left -= secs;
                if *left <= 0.0 {
                    self.phase = Phase::Open;
                }
            }
            Phase::Waiting { waited, .. } => *waited += secs,
            _ => {}
        }
        self.consume_inbox(ctx);
        if let Phase::Waiting { waited, .. } = self.phase
            && waited >= Self::timeout_secs(ctx)
        {
            let reply = self.local_reply(ctx);
            self.finish_reply(reply);
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        fill(buf, body, ctx.text_style());
        let chrome = ctx.palette.style(Dos::White, Dos::Black).fg(ctx.chrome());
        put_str(buf, area.x, area.y, &self.title_bar(), area.width, chrome);
        if matches!(self.phase, Phase::Paging(_)) {
            put_str(
                buf,
                area.x + 2,
                area.y + 2,
                "Paging SysOp...",
                area.width,
                ctx.fg(Dos::White),
            );
        } else {
            let skip = self.sysop_rows.len().saturating_sub(SYSOP_ROWS);
            for (i, row) in self.sysop_rows.iter().skip(skip).enumerate() {
                let y = area.y + 1 + u16::try_from(i).unwrap_or(0);
                put_str(buf, area.x + 1, y, row, area.width, ctx.text_style());
            }
        }
        draw_lines(
            buf,
            area.x + AVATAR_X,
            area.y + 1,
            &self.art,
            AVATAR_W,
            SYSOP_ROWS as u16,
        );
        let you = format!("─[ {} ]", self.handle);
        let bar = format!(
            "{you}{}",
            "─".repeat(80usize.saturating_sub(you.chars().count()))
        );
        put_str(buf, area.x, area.y + 12, &bar, area.width, chrome);
        let skip = self.player_rows.len().saturating_sub(PLAYER_ROWS);
        for (i, row) in self.player_rows.iter().skip(skip).enumerate() {
            let y = area.y + 13 + u16::try_from(i).unwrap_or(0);
            put_str(buf, area.x + 1, y, row, area.width, ctx.fg(Dos::White));
        }
        let input_area = Rect::new(area.x + 1, area.y + 23, area.width - 2, 1);
        match self.phase {
            Phase::Open => self.input.render(buf, input_area, ctx.fg(Dos::White)),
            Phase::Waiting { .. } => {
                put_str(
                    buf,
                    input_area.x,
                    input_area.y,
                    "> ...",
                    10,
                    ctx.fg(Dos::DarkGray),
                );
            }
            Phase::Left => {
                put_str(
                    buf,
                    input_area.x,
                    input_area.y,
                    "press any key",
                    20,
                    ctx.fg(Dos::DarkGray),
                );
            }
            Phase::Paging(_) => {}
        }
    }

    fn wants_status(&self) -> bool {
        true
    }

    fn in_text_input(&self) -> bool {
        matches!(self.phase, Phase::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::GenSource;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn setup() -> (ChatScreen, Ctx, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 3);
        let mut s = ChatScreen::new(&mut ctx);
        s.tick(Duration::from_millis(1600), &mut ctx);
        (s, ctx, dir)
    }

    fn rows(s: &ChatScreen, ctx: &Ctx) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| s.render(f, Rect::new(0, 0, 80, 25), ctx))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, buf.area, y)).collect()
    }

    fn say(s: &mut ChatScreen, ctx: &mut Ctx, text: &str) {
        for c in text.chars() {
            s.handle_key(press_char(c), ctx);
        }
        s.handle_key(press(KeyCode::Enter), ctx);
    }

    fn session(s: &ChatScreen) -> u64 {
        match s.phase {
            Phase::Waiting { session, .. } => session,
            p => panic!("not waiting: {p:?}"),
        }
    }

    fn done(session: u64, text: &str) -> DirectorEvent {
        DirectorEvent::ChatDone {
            session,
            text: text.into(),
            source: GenSource::Offline { reason: "t".into() },
        }
    }

    #[test]
    fn it_pages_the_sysop_for_a_second_and_a_half_then_opens_the_split_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 3);
        let handle = ctx.place().unwrap().sysop.handle.clone();
        let mut s = ChatScreen::new(&mut ctx);
        assert!(rows(&s, &ctx).iter().any(|r| r.contains("Paging SysOp...")));
        assert!(!s.in_text_input());
        s.tick(Duration::from_millis(1400), &mut ctx);
        assert!(matches!(s.phase, Phase::Paging(_)));
        // Keys type nothing while paging.
        s.handle_key(press_char('x'), &mut ctx);
        s.tick(Duration::from_millis(200), &mut ctx);
        assert!(s.in_text_input());
        assert!(s.input.is_empty());
        let r = rows(&s, &ctx);
        assert!(!r.iter().any(|r| r.contains("Paging")));
        assert!(r[0].starts_with(&format!("─[ {handle} ]")), "{}", r[0]);
        assert!(r[0].contains("SysOp Chat") && r[0].trim_end().ends_with("Esc ends ─"));
        assert!(r[12].starts_with("─[ Tester ]"));
        assert!(r[23].starts_with(" > "));
    }

    #[test]
    fn a_message_streams_a_reply_and_stale_sessions_are_ignored() {
        let (mut s, mut ctx, _d) = setup();
        say(&mut s, &mut ctx, "who are you?");
        assert!(
            !s.in_text_input(),
            "input is disabled while a reply is pending"
        );
        let id = session(&s);
        assert!(rows(&s, &ctx)[13].contains("> who are you?"));
        // Typing while pending does nothing.
        s.handle_key(press_char('z'), &mut ctx);
        assert!(s.input.is_empty());
        ctx.chat_inbox.push(DirectorEvent::ChatDelta {
            session: id + 1,
            text: "STALE".into(),
        });
        ctx.chat_inbox.push(DirectorEvent::ChatDelta {
            session: id,
            text: "I am ".into(),
        });
        ctx.chat_inbox.push(DirectorEvent::ChatDelta {
            session: id,
            text: "here".into(),
        });
        s.tick(Duration::from_millis(33), &mut ctx);
        let r = rows(&s, &ctx);
        assert!(r[1].contains("I am here"), "{}", r[1]);
        assert!(!r.join("\n").contains("STALE"));
        assert!(matches!(s.phase, Phase::Waiting { .. }));
        ctx.chat_inbox.push(done(id, "I am here. Always."));
        ctx.chat_inbox.push(done(id + 1, "STALE DONE"));
        s.tick(Duration::from_millis(33), &mut ctx);
        let r = rows(&s, &ctx);
        assert!(r[1].contains("I am here. Always."));
        assert!(!r.join("\n").contains("STALE"));
        assert!(s.in_text_input(), "the turn is over");
        // The history carries both sides.
        assert_eq!(s.history.len(), 2);
        assert!(s.history[0].from_player && !s.history[1].from_player);
    }

    #[test]
    fn the_sysop_leaves_after_twelve_messages() {
        let (mut s, mut ctx, _d) = setup();
        let handle = s.partner.handle.clone();
        for i in 0..MAX_MESSAGES {
            say(&mut s, &mut ctx, &format!("message {i}"));
            assert_eq!(s.sent, i + 1);
            let id = session(&s);
            ctx.chat_inbox.push(done(id, &format!("reply {i}")));
            s.tick(Duration::from_millis(33), &mut ctx);
        }
        assert_eq!(s.phase, Phase::Left);
        assert!(!s.in_text_input());
        let r = rows(&s, &ctx);
        assert!(
            r.iter()
                .any(|r| r.contains(&format!("{handle} has left chat.")))
        );
        assert!(r[13..23].iter().any(|r| r.contains("message 11")));
        assert!(matches!(s.handle_key(press_char('a'), &mut ctx), Nav::Back));
    }

    #[test]
    fn without_a_reply_the_screen_falls_back_to_eliza() {
        let (mut s, mut ctx, _d) = setup();
        ctx.config.ai.timeout_secs = 10;
        say(&mut s, &mut ctx, "tell me about wren");
        s.tick(Duration::from_secs(14), &mut ctx);
        assert!(
            matches!(s.phase, Phase::Waiting { .. }),
            "still within the grace"
        );
        s.tick(Duration::from_secs(2), &mut ctx);
        assert_eq!(s.phase, Phase::Open);
        let character = s
            .partner
            .cast_id
            .as_deref()
            .and_then(|id| ctx.pack.character(id));
        let want = eliza_reply(&ctx.pack, character, "tell me about wren", 0);
        assert_eq!(s.history.last().unwrap().text, want);
        assert!(rows(&s, &ctx)[1].contains(want.split_whitespace().next().unwrap()));
    }

    #[test]
    fn esc_ends_chat_and_later_events_do_not_leak_into_the_next_one() {
        let (mut s, mut ctx, _d) = setup();
        say(&mut s, &mut ctx, "hello");
        let id = session(&s);
        assert!(matches!(
            s.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
        ctx.chat_inbox.push(done(id, "LATE"));
        let mut next = ChatScreen::new(&mut ctx);
        next.tick(Duration::from_millis(1600), &mut ctx);
        assert!(ctx.chat_inbox.is_empty());
        assert!(!rows(&next, &ctx).join("\n").contains("LATE"));
    }

    #[test]
    fn long_conversations_scroll_and_wrap_to_the_panes() {
        let (mut s, mut ctx, _d) = setup();
        let long = "word ".repeat(40);
        say(&mut s, &mut ctx, &long[..69]);
        let id = session(&s);
        ctx.chat_inbox.push(done(id, &long));
        s.tick(Duration::from_millis(33), &mut ctx);
        let r = rows(&s, &ctx);
        assert!(
            r[1..12].iter().all(|row| row
                .chars()
                .take(50)
                .collect::<String>()
                .trim_end()
                .chars()
                .count()
                <= 49),
            "the sysop text stays out of the avatar column"
        );
        assert!(
            r[1..12]
                .iter()
                .filter(|row| row.contains("word word"))
                .count()
                >= 4
        );
        for i in 0..10 {
            say(&mut s, &mut ctx, &format!("line number {i}"));
            let id = session(&s);
            ctx.chat_inbox.push(done(id, &format!("r{i}")));
            s.tick(Duration::from_millis(33), &mut ctx);
        }
        let r = rows(&s, &ctx);
        assert!(r[13..23].iter().any(|row| row.contains("line number 9")));
        assert!(
            !r[13..23].iter().any(|row| row.contains("word word")),
            "oldest line scrolled off"
        );
        assert!(r[1..12].iter().any(|row| row.contains("r9")));
    }
}
