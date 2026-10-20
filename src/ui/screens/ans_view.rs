//! Full-canvas ANSI art viewer: a decoded artpack file drawn one character at a time at modem
//! speed, then scrolled by the usual viewer keys.
//!
//! The text comes from [`crate::ans::decode`]; [`crate::ans::Canvas`] interprets the escapes and
//! lays the art out 80 columns wide. Rows 0..24 are the artwork, row 24 is the footer. At baud 0
//! the whole file is shown on the first tick; otherwise any key completes the drawing.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::ans::Canvas;
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_cell, put_str};
use crate::ui::keys::{Key, classify};

use super::{Nav, ScreenImpl};

/// Artwork rows (canvas rows `0..24`; row 24 is the footer).
const ROWS: usize = 24;
/// Nanoseconds in a second, for the baud carry.
const NANOS: u128 = 1_000_000_000;

/// An ANSI art file drawn into the canvas at modem speed.
pub struct AnsViewScreen {
    /// `NAME  description` in the footer.
    title: String,
    /// The decoded file, fed to the canvas one character at a time.
    text: Vec<char>,
    /// Characters fed so far.
    fed: usize,
    canvas: Canvas,
    /// Characters per second; `None` = instant (baud 0).
    cps: Option<u64>,
    /// Sub-character progress carried between ticks, in nanosecond-characters.
    carry: u128,
    /// First visible canvas row.
    top: usize,
    /// The whole file has been fed.
    done: bool,
}

impl AnsViewScreen {
    /// A viewer over decoded ANSI `text`; `ice_colors` enables iCE background colours.
    pub fn new(ctx: &mut Ctx, title: String, text: &str, ice_colors: bool) -> Self {
        Self {
            title,
            text: text.chars().collect(),
            fed: 0,
            canvas: Canvas::new(ice_colors),
            cps: (ctx.baud() > 0).then(|| u64::from((ctx.baud() / 10).max(1))),
            carry: 0,
            top: 0,
            done: false,
        }
    }

    /// Feed up to `n` more characters into the canvas.
    fn feed_n(&mut self, n: usize) {
        let end = self.fed.saturating_add(n).min(self.text.len());
        while self.fed < end {
            self.canvas.feed(self.text[self.fed]);
            self.fed += 1;
        }
    }

    /// Feed the rest of the file at once.
    fn skip(&mut self) {
        self.feed_n(usize::MAX);
        self.done = true;
        self.top = self.rows().saturating_sub(ROWS);
    }

    /// Rows the canvas has used (at least 1).
    fn rows(&self) -> usize {
        self.canvas.rows()
    }

    /// The largest valid first visible row.
    fn max_top(&self) -> usize {
        self.rows().saturating_sub(ROWS)
    }
}

impl ScreenImpl for AnsViewScreen {
    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Nav {
        if !self.done {
            self.skip();
            return Nav::Stay;
        }
        match classify(&key) {
            Key::Up => self.top = self.top.saturating_sub(1),
            Key::Down => self.top = (self.top + 1).min(self.max_top()),
            Key::PgUp => self.top = self.top.saturating_sub(ROWS),
            Key::PgDn | Key::Char(' ') => self.top = (self.top + ROWS).min(self.max_top()),
            Key::Home => self.top = 0,
            Key::End => self.top = self.max_top(),
            Key::Esc | Key::Char('q') | Key::Char('Q') => return Nav::Back,
            _ => {}
        }
        Nav::Stay
    }

    fn tick(&mut self, dt: Duration, _ctx: &mut Ctx) -> Nav {
        if !self.done {
            match self.cps {
                None => self.feed_n(usize::MAX),
                Some(cps) => {
                    self.carry += dt.as_nanos() * u128::from(cps);
                    let whole = self.carry / NANOS;
                    self.carry %= NANOS;
                    self.feed_n(usize::try_from(whole).unwrap_or(usize::MAX));
                }
            }
            let follow = (self.canvas.cursor_row() + 1).saturating_sub(ROWS);
            self.top = self.top.max(follow);
            self.done = self.fed == self.text.len();
            if self.done {
                self.top = self.top.min(self.max_top());
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        for y in 0..ROWS {
            let row = self.canvas.row(self.top + y);
            for (x, cell) in row.iter().take(usize::from(area.width)).enumerate() {
                put_cell(
                    buf,
                    area.x + u16::try_from(x).unwrap_or(0),
                    area.y + u16::try_from(y).unwrap_or(0),
                    cell.ch,
                    ctx.palette.style(cell.fg, cell.bg),
                );
            }
        }
        let style = ctx.palette.style(Dos::Black, Dos::LightGray);
        let footer_area = Rect::new(
            area.x,
            area.y + u16::try_from(ROWS).unwrap_or(0),
            area.width,
            1,
        );
        fill(buf, footer_area, style);
        let footer = if self.done {
            format!(
                "{}  rows {}-{} of {}  Up/Down PgUp/PgDn scroll, Esc leaves",
                self.title,
                self.top + 1,
                (self.top + ROWS).min(self.rows()),
                self.rows()
            )
        } else {
            format!(
                "{}  RX {}/{} chars  any key completes",
                self.title,
                self.fed,
                self.text.len()
            )
        };
        put_str(
            buf,
            footer_area.x,
            footer_area.y,
            &footer,
            footer_area.width,
            style,
        );
    }

    fn wants_status(&self) -> bool {
        false
    }

    fn in_text_input(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_ctx;
    use crate::ui::canvas::{HEIGHT, row_text};
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    /// A test context at `baud`; the `TempDir` must outlive it.
    fn rig(baud: u32) -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.config.display.baud = baud;
        (dir, ctx)
    }

    /// The buffer the screen draws into.
    fn render_buffer(screen: &AnsViewScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    /// The 25 rendered canvas rows as running text.
    fn screen_text(screen: &AnsViewScreen, ctx: &Ctx) -> String {
        let buf = render_buffer(screen, ctx);
        (0..HEIGHT)
            .map(|y| row_text(&buf, buf.area, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn draws_decoded_art_immediately_at_baud_zero() {
        let (_dir, mut ctx) = rig(0);
        let mut screen =
            AnsViewScreen::new(&mut ctx, "X.ANS  art".to_string(), "\u{1b}[1;31mhi", false);
        assert!(matches!(screen.tick(Duration::ZERO, &mut ctx), Nav::Stay));
        let buf = render_buffer(&screen, &ctx);
        assert_eq!(buf[(0, 0)].symbol(), "h");
        assert_eq!(buf[(0, 0)].fg, ctx.palette.color(Dos::LightRed));
        assert_eq!(buf[(1, 0)].symbol(), "i");
        let footer = row_text(&buf, buf.area, 24);
        assert!(footer.contains("rows 1-1 of 1"), "{footer}");
    }

    #[test]
    fn streams_at_baud_and_any_key_completes() {
        let (_dir, mut ctx) = rig(9600);
        let text = "x".repeat(2000);
        let mut screen = AnsViewScreen::new(&mut ctx, "big".to_string(), &text, false);
        let nav = screen.tick(Duration::from_secs(1), &mut ctx);
        assert!(matches!(nav, Nav::Stay));
        assert_eq!(screen.fed, 960);
        let buf = render_buffer(&screen, &ctx);
        let footer = row_text(&buf, buf.area, 24);
        assert!(footer.contains("RX 960/2000 chars"), "{footer}");

        // A key during the animation completes it instead of leaving the screen.
        let nav = screen.handle_key(press(KeyCode::Esc), &mut ctx);
        assert!(matches!(nav, Nav::Stay));
        assert!(screen.done);
        assert_eq!(screen.fed, 2000);

        let mut screen = AnsViewScreen::new(&mut ctx, "big".to_string(), &text, false);
        let nav = screen.tick(Duration::from_secs(1), &mut ctx);
        assert!(matches!(nav, Nav::Stay));
        let nav = screen.handle_key(press_char('a'), &mut ctx);
        assert!(matches!(nav, Nav::Stay));
        assert!(screen.done);
        let nav = screen.handle_key(press(KeyCode::Esc), &mut ctx);
        assert!(matches!(nav, Nav::Back));
    }

    #[test]
    fn tall_art_follows_the_cursor_and_scrolls_when_done() {
        let (_dir, mut ctx) = rig(0);
        let text: String = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        let mut screen = AnsViewScreen::new(&mut ctx, "tall".to_string(), &text, false);
        assert!(matches!(screen.tick(Duration::ZERO, &mut ctx), Nav::Stay));
        assert!(screen.done);
        assert_eq!(screen.top, 36);
        let nav = screen.handle_key(press(KeyCode::Up), &mut ctx);
        assert!(matches!(nav, Nav::Stay));
        assert_eq!(screen.top, 35);
        let text = screen_text(&screen, &ctx);
        assert!(text.contains("line 35"), "{text}");
    }
}
