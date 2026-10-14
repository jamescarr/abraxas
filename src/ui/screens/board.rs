//! The message base: the level's posts (including the engine's `re: front door` clue) plus the
//! player's own replies, a reader with `[N]ext [P]rev [R]eply [Q]uit`, and a one-line reply input.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl};
use crate::game::state::PlayerPost;
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::input::{InputEvent, TextInput};
use crate::ui::widgets::viewer::Viewer;

/// Longest reply the input accepts.
pub const MAX_REPLY: usize = 200;

/// First row of the message list.
const LIST_TOP: u16 = 4;
/// Rows available to the list.
const LIST_ROWS: usize = 18;
/// Reader body width and rows (the viewer's `--More--` footer takes one more row).
const BODY_WIDTH: u16 = 76;
const BODY_ROWS: usize = 15;
const BODY_TOP: u16 = 6;
const MESSAGE_ROW: u16 = 22;
const HINT_ROW: u16 = 23;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    from: String,
    subject: String,
    body: String,
    mine: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    List,
    Read,
}

/// The message base screen.
pub struct BoardScreen {
    messages: Vec<Message>,
    selected: usize,
    view: View,
    viewer: Viewer,
    reply: Option<TextInput>,
    notice: String,
}

/// The board's messages at the current depth: the place's posts, then the player's own.
fn collect_messages(ctx: &Ctx) -> Vec<Message> {
    let mut out = Vec::new();
    if let Some(place) = ctx.place() {
        out.extend(place.posts.iter().map(|p| Message {
            from: p.from.clone(),
            subject: p.subject.clone(),
            body: p.body.clone(),
            mine: false,
        }));
    }
    if let Some(run) = ctx.run_ref() {
        out.extend(
            run.player_posts
                .iter()
                .filter(|p| p.depth == run.depth)
                .map(|p| Message {
                    from: run.handle.clone(),
                    subject: p.subject.clone(),
                    body: p.body.clone(),
                    mine: true,
                }),
        );
    }
    out
}

impl BoardScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        Self {
            messages: collect_messages(ctx),
            selected: 0,
            view: View::List,
            viewer: Viewer::new(Vec::new(), BODY_ROWS),
            reply: None,
            notice: String::new(),
        }
    }

    fn open(&mut self, index: usize, ctx: &Ctx) {
        let Some(msg) = self.messages.get(index) else {
            return;
        };
        self.selected = index;
        self.viewer = Viewer::from_text(&msg.body, BODY_WIDTH, BODY_ROWS, &ctx.palette);
        self.view = View::Read;
        self.notice.clear();
    }

    /// Store the reply to the open message and return to the list with it selected.
    fn post_reply(&mut self, body: &str, ctx: &mut Ctx) {
        let Some(subject) = self.messages.get(self.selected).map(|m| m.subject.clone()) else {
            return;
        };
        let Some(run) = ctx.run_mut() else {
            return;
        };
        run.player_posts.push(PlayerPost {
            depth: run.depth,
            subject: format!("re: {subject}"),
            body: body.to_string(),
        });
        ctx.autosave();
        self.messages = collect_messages(ctx);
        self.selected = self.messages.len().saturating_sub(1);
        self.view = View::List;
        self.notice = format!("Reply posted as message #{}.", self.selected + 1);
    }

    fn key_reply(&mut self, key: &KeyEvent, ctx: &mut Ctx) {
        let Some(input) = self.reply.as_mut() else {
            return;
        };
        match input.handle_key(key) {
            InputEvent::Submit(text) => {
                self.reply = None;
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.notice = "Reply cancelled.".to_string();
                } else {
                    self.post_reply(&text, ctx);
                }
            }
            InputEvent::Cancel => {
                self.reply = None;
                self.notice = "Reply cancelled.".to_string();
            }
            InputEvent::None | InputEvent::Changed => {}
        }
    }

    fn key_list(&mut self, key: &KeyEvent, ctx: &Ctx) -> Nav {
        let count = self.messages.len();
        match classify(key) {
            Key::Esc => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'q') => return Nav::Back,
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = count.saturating_sub(1),
            Key::Enter => self.open(self.selected, ctx),
            Key::Char(c) => {
                if let Some(n) = c.to_digit(10).filter(|n| *n >= 1)
                    && let Ok(i) = usize::try_from(n - 1)
                    && i < count
                {
                    self.open(i, ctx);
                }
            }
            _ => {}
        }
        Nav::Stay
    }

    fn key_read(&mut self, key: &KeyEvent, ctx: &Ctx) {
        let count = self.messages.len();
        match classify(key) {
            Key::Esc => self.view = View::List,
            Key::Char(c) => match c.to_ascii_lowercase() {
                'n' => self.open((self.selected + 1) % count.max(1), ctx),
                'p' => self.open((self.selected + count.max(1) - 1) % count.max(1), ctx),
                'q' => self.view = View::List,
                'r' => {
                    self.reply = Some(TextInput::new("Reply> ", MAX_REPLY));
                    self.notice.clear();
                }
                ' ' => {
                    self.viewer.handle_key(key);
                }
                _ => {}
            },
            _ => {
                self.viewer.handle_key(key);
            }
        }
    }

    fn render_list(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let w = area.width;
        put_str(
            buf,
            area.x,
            area.y + 2,
            " #   From             Subject",
            w,
            p.style(Dos::LightCyan, Dos::Black),
        );
        put_str(
            buf,
            area.x,
            area.y + 3,
            &"─".repeat(usize::from(w)),
            w,
            ctx.fg(Dos::DarkGray),
        );
        let top = self.selected.saturating_sub(LIST_ROWS - 1);
        for (row, (i, m)) in self
            .messages
            .iter()
            .enumerate()
            .skip(top)
            .take(LIST_ROWS)
            .enumerate()
        {
            let y = area.y + LIST_TOP + u16::try_from(row).unwrap_or(0);
            let line = format!("{:>2}   {:<17}{}", i + 1, text_fit(&m.from, 16), m.subject);
            let style = if i == self.selected {
                p.style(Dos::Black, Dos::LightGray)
            } else if m.mine {
                p.style(Dos::LightGreen, Dos::Black)
            } else {
                p.style(Dos::White, Dos::Black)
            };
            put_str(
                buf,
                area.x,
                y,
                &format!("{line:<width$}", width = usize::from(w)),
                w,
                style,
            );
        }
        if self.messages.is_empty() {
            put_str(
                buf,
                area.x + 2,
                area.y + LIST_TOP,
                "No messages.",
                w,
                ctx.fg(Dos::LightGray),
            );
        }
    }

    fn render_read(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, ctx: &Ctx) {
        let Some(msg) = self.messages.get(self.selected) else {
            return;
        };
        let w = area.width;
        let label = ctx.fg(Dos::LightCyan);
        let value = ctx.fg(Dos::White);
        let head = format!(" Message {} of {}", self.selected + 1, self.messages.len());
        put_str(buf, area.x, area.y + 2, &head, w, ctx.fg(Dos::Yellow));
        put_str(buf, area.x + 1, area.y + 3, "From:", 6, label);
        put_str(buf, area.x + 8, area.y + 3, &msg.from, w - 9, value);
        put_str(buf, area.x + 1, area.y + 4, "Subj:", 6, label);
        put_str(buf, area.x + 8, area.y + 4, &msg.subject, w - 9, value);
        put_str(
            buf,
            area.x,
            area.y + 5,
            &"─".repeat(usize::from(w)),
            w,
            ctx.fg(Dos::DarkGray),
        );
        let body = Rect::new(
            area.x + 2,
            area.y + BODY_TOP,
            BODY_WIDTH,
            u16::try_from(BODY_ROWS + 1).unwrap_or(16),
        );
        self.viewer.render(buf, body, &ctx.palette);
    }
}

/// `text` cut to at most `max` characters.
fn text_fit(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

impl ScreenImpl for BoardScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if self.reply.is_some() {
            self.key_reply(&key, ctx);
            return Nav::Stay;
        }
        match self.view {
            View::List => return self.key_list(&key, ctx),
            View::Read => self.key_read(&key, ctx),
        }
        Nav::Stay
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let w = area.width;
        let bbs = ctx.place().map_or("", |pl| pl.bbs_name.as_str());
        let title = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), title);
        put_str(
            buf,
            area.x,
            area.y,
            &format!(" {bbs} ─ Message Bases"),
            w,
            title,
        );
        match self.view {
            View::List => self.render_list(buf, area, ctx),
            View::Read => self.render_read(buf, area, ctx),
        }
        let msg_y = area.y + MESSAGE_ROW;
        if let Some(input) = &self.reply {
            input.render(
                buf,
                Rect::new(area.x + 1, msg_y, w - 2, 1),
                ctx.fg(Dos::White),
            );
        } else if !self.notice.is_empty() {
            put_str(
                buf,
                area.x + 1,
                msg_y,
                &self.notice,
                w - 2,
                ctx.fg(Dos::Yellow),
            );
        }
        let hint = match (self.view, self.reply.is_some()) {
            (_, true) => " Enter Post   Esc Cancel",
            (View::List, _) => " ↑↓ Select   Enter Read   1-9 Jump   Esc Back",
            (View::Read, _) => " [N]ext [P]rev [R]eply [Q]uit   ↑↓ Scroll",
        };
        put_str(
            buf,
            area.x,
            area.y + HINT_ROW,
            hint,
            w,
            ctx.fg(Dos::LightGray),
        );
    }

    fn wants_status(&self) -> bool {
        true
    }

    fn in_text_input(&self) -> bool {
        self.reply.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    fn ctx_at(dir: &std::path::Path, depth: u32) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, depth);
        ctx
    }

    fn draw(screen: &BoardScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..25).map(|y| row_text(buf, buf.area, y)).collect()
    }

    fn type_text(screen: &mut BoardScreen, ctx: &mut Ctx, text: &str) {
        for c in text.chars() {
            screen.handle_key(press_char(c), ctx);
        }
    }

    #[test]
    fn list_shows_every_post_including_the_clue_and_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        let screen = BoardScreen::new(&mut ctx);
        let posts = ctx.place().unwrap().posts.clone();
        assert!(posts.iter().any(|p| p.subject == "re: front door"));
        let lines = rows(&draw(&screen, &ctx));
        assert!(
            lines[2].contains("#") && lines[2].contains("From") && lines[2].contains("Subject")
        );
        for (i, post) in posts.iter().enumerate() {
            let line = &lines[4 + i];
            assert!(line.contains(&post.subject), "{line:?}");
            assert!(
                line.contains(&post.from.chars().take(16).collect::<String>()),
                "{line:?}"
            );
        }
        assert!(lines[4 + posts.len()].trim().is_empty());
    }

    #[test]
    fn number_keys_open_a_message_and_the_reader_shows_its_body() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        let mut screen = BoardScreen::new(&mut ctx);
        let post = ctx.place().unwrap().posts[1].clone();
        screen.handle_key(press_char('2'), &mut ctx);
        let lines = rows(&draw(&screen, &ctx));
        assert!(lines[3].contains(&post.from) && lines[4].contains(&post.subject));
        let first_word = post.body.split_whitespace().next().unwrap();
        assert!(lines[6].contains(first_word), "{:?}", lines[6]);
        assert!(lines[23].contains("[N]ext [P]rev [R]eply [Q]uit"));
        assert!(matches!(
            screen.handle_key(press_char('q'), &mut ctx),
            Nav::Stay
        ));
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
    }

    #[test]
    fn next_and_prev_wrap_around_the_message_base() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        let mut screen = BoardScreen::new(&mut ctx);
        let n = screen.messages.len();
        screen.handle_key(press_char('1'), &mut ctx);
        screen.handle_key(press_char('p'), &mut ctx);
        assert_eq!(screen.selected, n - 1);
        screen.handle_key(press_char('N'), &mut ctx);
        assert_eq!(screen.selected, 0);
        screen.handle_key(press_char('n'), &mut ctx);
        assert_eq!(screen.selected, 1);
        let lines = rows(&draw(&screen, &ctx));
        assert!(lines[2].contains(&format!("Message 2 of {n}")));
    }

    #[test]
    fn reply_is_stored_saved_and_listed_as_the_players_post() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        let mut screen = BoardScreen::new(&mut ctx);
        let posts = ctx.place().unwrap().posts.clone();
        screen.handle_key(press_char('1'), &mut ctx);
        assert!(!screen.in_text_input());
        screen.handle_key(press_char('r'), &mut ctx);
        assert!(screen.in_text_input());
        type_text(&mut screen, &mut ctx, "i hear you");
        assert!(rows(&draw(&screen, &ctx))[22].contains("Reply> i hear you"));
        screen.handle_key(press(KeyCode::Enter), &mut ctx);
        assert!(!screen.in_text_input());

        let run = ctx.run_ref().unwrap();
        assert_eq!(
            run.player_posts,
            vec![PlayerPost {
                depth: 1,
                subject: format!("re: {}", posts[0].subject),
                body: "i hear you".into(),
            }]
        );
        let saved = ctx.store.load("Tester").unwrap().unwrap();
        assert_eq!(saved.player_posts, run.player_posts);

        let lines = rows(&draw(&screen, &ctx));
        let mine = &lines[4 + posts.len()];
        assert!(
            mine.contains("Tester") && mine.contains(&format!("re: {}", posts[0].subject)),
            "{mine:?}"
        );
        assert!(lines[22].contains(&format!("Reply posted as message #{}", posts.len() + 1)));
        // A fresh visit still lists it.
        let again = BoardScreen::new(&mut ctx);
        assert_eq!(again.messages.len(), posts.len() + 1);
        assert!(again.messages.last().unwrap().mine);
    }

    #[test]
    fn replies_are_capped_at_200_characters_and_cancel_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        let mut screen = BoardScreen::new(&mut ctx);
        screen.handle_key(press_char('1'), &mut ctx);
        screen.handle_key(press_char('r'), &mut ctx);
        type_text(&mut screen, &mut ctx, "no");
        screen.handle_key(press(KeyCode::Esc), &mut ctx);
        assert!(ctx.run_ref().unwrap().player_posts.is_empty());
        assert!(!screen.in_text_input());

        screen.handle_key(press_char('r'), &mut ctx);
        type_text(&mut screen, &mut ctx, &"x".repeat(250));
        screen.handle_key(press(KeyCode::Enter), &mut ctx);
        let posts = &ctx.run_ref().unwrap().player_posts;
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].body.chars().count(), MAX_REPLY);
    }

    #[test]
    fn an_empty_reply_posts_nothing_and_other_depths_are_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 2);
        ctx.run_mut().unwrap().player_posts.push(PlayerPost {
            depth: 1,
            subject: "re: old".into(),
            body: "from another level".into(),
        });
        let mut screen = BoardScreen::new(&mut ctx);
        let place_posts = ctx.place().unwrap().posts.len();
        assert_eq!(screen.messages.len(), place_posts);
        screen.handle_key(press_char('1'), &mut ctx);
        screen.handle_key(press_char('r'), &mut ctx);
        screen.handle_key(press(KeyCode::Enter), &mut ctx);
        assert_eq!(ctx.run_ref().unwrap().player_posts.len(), 1);
        assert_eq!(screen.messages.len(), place_posts);
    }
}
