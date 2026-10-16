//! Exits: the three ways down, one per kind. Picking one starts the Descent.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{DescentScreen, Nav, Screen, ScreenImpl};
use crate::pack::model::{ExitDef, ExitKind};
use crate::palette::Dos;
use crate::text::wrap;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_centered, put_str};
use crate::ui::keys::{Key, classify};

/// Width of the exit text column.
const TEXT_WIDTH: usize = 62;
/// First row of the exit list and rows per exit.
const LIST_TOP: u16 = 6;
const ROW_STEP: u16 = 5;

/// The Exits screen: `1`/`2`/`3` take an exit at once; arrows move and Enter takes the selection.
pub struct ExitsScreen {
    selected: usize,
    exits: Vec<ExitDef>,
    heading: String,
}

fn kind_mark(kind: ExitKind) -> char {
    match kind {
        ExitKind::Calm => '≈',
        ExitKind::Deep => '▼',
        ExitKind::Mirror => '◘',
    }
}

fn kind_label(kind: ExitKind) -> &'static str {
    match kind {
        ExitKind::Calm => "CALM",
        ExitKind::Deep => "DEEP",
        ExitKind::Mirror => "MIRROR",
    }
}

fn kind_colour(kind: ExitKind) -> Dos {
    match kind {
        ExitKind::Calm => Dos::LightCyan,
        ExitKind::Deep => Dos::LightRed,
        ExitKind::Mirror => Dos::LightMagenta,
    }
}

impl ExitsScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let mut s = Self {
            selected: 0,
            exits: Vec::new(),
            heading: String::new(),
        };
        s.load(ctx);
        s
    }

    /// Pick up the passage's exits and the level heading (also retried from `tick`).
    fn load(&mut self, ctx: &Ctx) {
        let Some(run) = ctx.run_ref() else {
            return;
        };
        self.heading = match ctx.place() {
            Some(p) => format!("LEVEL {:02}  {}", run.depth, p.title),
            None => format!("LEVEL {:02}", run.depth),
        };
        if let Some(passage) = ctx.passage() {
            self.exits.clone_from(&passage.exits);
            self.selected = self.selected.min(self.exits.len().saturating_sub(1));
        }
    }

    /// The exits shown (kind and text), for tests.
    pub fn exits(&self) -> &[ExitDef] {
        &self.exits
    }

    /// The highlighted exit index.
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn take(&self, index: usize, ctx: &mut Ctx) -> Nav {
        let Some(run) = ctx.run_mut() else {
            return Nav::Stay;
        };
        if !run.apply_exit(index) {
            return Nav::Stay;
        }
        ctx.autosave();
        Nav::To(Box::new(Screen::Descent(DescentScreen::new(ctx))))
    }
}

impl ScreenImpl for ExitsScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if self.exits.is_empty() {
            return match classify(&key) {
                Key::Esc => Nav::Back,
                _ => Nav::Stay,
            };
        }
        let last = self.exits.len() - 1;
        match classify(&key) {
            Key::Esc => Nav::Back,
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                Nav::Stay
            }
            Key::Down => {
                self.selected = (self.selected + 1).min(last);
                Nav::Stay
            }
            Key::Enter => self.take(self.selected, ctx),
            Key::Char(c @ '1'..='9') => {
                let index = usize::from(c as u8 - b'1');
                if index <= last {
                    self.selected = index;
                    self.take(index, ctx)
                } else {
                    Nav::Stay
                }
            }
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, _dt: Duration, ctx: &mut Ctx) -> Nav {
        if self.exits.is_empty() {
            self.load(ctx);
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let black = p.color(Dos::Black);
        let chrome = Style::new().fg(ctx.chrome()).bg(black);
        let body = ctx.text_style();
        fill(buf, area, body);

        put_centered(buf, area, area.y + 1, &self.heading, chrome);
        put_str(
            buf,
            area.x + 1,
            area.y + 2,
            &"═".repeat(usize::from(area.width) - 2),
            area.width - 2,
            chrome,
        );
        if self.exits.is_empty() {
            put_centered(
                buf,
                area,
                area.y + 11,
                "DECRYPTING...",
                p.style(Dos::LightGreen, Dos::Black),
            );
            return;
        }
        put_centered(
            buf,
            area,
            area.y + 3,
            "There is no hanging up. Every Goodbye leads one level deeper.",
            body,
        );
        put_centered(
            buf,
            area,
            area.y + 4,
            "Choose the way down.",
            p.style(Dos::White, Dos::Black),
        );
        for (i, exit) in self.exits.iter().enumerate() {
            let y = area.y + LIST_TOP + ROW_STEP * u16::try_from(i).unwrap_or(0);
            let on = i == self.selected;
            let kind = p.style(kind_colour(exit.kind), Dos::Black);
            let label = format!(
                "{} [{}] {} {}",
                if on { '►' } else { ' ' },
                i + 1,
                kind_mark(exit.kind),
                kind_label(exit.kind)
            );
            let label_style = if on {
                p.style(Dos::Black, Dos::LightGray)
            } else {
                kind
            };
            let label_w = u16::try_from(label.chars().count()).unwrap_or(0) + 1;
            put_str(
                buf,
                area.x + 4,
                y,
                &format!("{label} "),
                label_w,
                label_style,
            );
            let text_style = if on {
                p.style(Dos::White, Dos::Black)
            } else {
                body
            };
            for (k, line) in wrap(&exit.text, TEXT_WIDTH).iter().take(3).enumerate() {
                put_str(
                    buf,
                    area.x + 9,
                    y + 1 + u16::try_from(k).unwrap_or(0),
                    line,
                    TEXT_WIDTH as u16,
                    text_style,
                );
            }
        }
        put_centered(
            buf,
            area,
            area.y + 22,
            "1-3 or ↑/↓ + Enter to descend │ Esc back to the menu",
            p.style(Dos::DarkGray, Dos::Black),
        );
    }

    fn wants_status(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::Phase;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    /// A run at `depth` that has made its choice, as it is when the player picks `G`.
    fn ready(dir: &std::path::Path, depth: u32) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, depth);
        let pack = ctx.pack.clone();
        let run = ctx.run.as_mut().unwrap();
        assert!(run.apply_choice(&pack, 0));
        ctx
    }

    fn rows(screen: &ExitsScreen, ctx: &Ctx) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 25);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, area, ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, area, y)).collect()
    }

    #[test]
    fn a_number_key_takes_that_exit_and_starts_the_descent() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ready(dir.path(), 3);
        let mut s = ExitsScreen::new(&mut ctx);
        let wanted = s.exits()[1].clone();
        let nav = s.handle_key(press_char('2'), &mut ctx);
        assert!(matches!(nav, Nav::To(next) if next.name() == "Descent"));
        let run = ctx.run.as_ref().unwrap();
        assert_eq!(run.exit_taken.as_ref(), Some(&wanted));
        assert_eq!(run.phase, Phase::Descending { exit: 1 });
        assert_eq!(run.chronicle.entry(3).unwrap().exit.as_ref(), Some(&wanted));
        assert!(ctx.store.exists("Tester"), "the choice is autosaved");
    }

    #[test]
    fn arrows_move_and_enter_takes_the_highlighted_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ready(dir.path(), 3);
        let mut s = ExitsScreen::new(&mut ctx);
        assert_eq!(s.selected(), 0);
        s.handle_key(press(KeyCode::Down), &mut ctx);
        s.handle_key(press(KeyCode::Down), &mut ctx);
        s.handle_key(press(KeyCode::Down), &mut ctx);
        assert_eq!(s.selected(), 2, "the selection stops at the last exit");
        s.handle_key(press(KeyCode::Up), &mut ctx);
        assert!(matches!(
            s.handle_key(press(KeyCode::Enter), &mut ctx),
            Nav::To(_)
        ));
        assert_eq!(
            ctx.run.as_ref().unwrap().phase,
            Phase::Descending { exit: 1 }
        );
    }

    #[test]
    fn an_unknown_number_and_other_keys_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ready(dir.path(), 3);
        let mut s = ExitsScreen::new(&mut ctx);
        assert!(matches!(s.handle_key(press_char('4'), &mut ctx), Nav::Stay));
        assert!(matches!(s.handle_key(press_char('x'), &mut ctx), Nav::Stay));
        assert!(ctx.run.as_ref().unwrap().exit_taken.is_none());
        assert!(matches!(
            s.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
    }

    #[test]
    fn the_three_exits_show_their_kind_marks_and_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ready(dir.path(), 3);
        let s = ExitsScreen::new(&mut ctx);
        let text = rows(&s, &ctx).join("\n");
        for (i, e) in s.exits().iter().enumerate() {
            let mark = kind_mark(e.kind);
            assert!(text.contains(&format!("[{}] {mark} {}", i + 1, kind_label(e.kind))));
            let first_word = e.text.split_whitespace().next().unwrap();
            assert!(text.contains(first_word), "{first_word} missing:\n{text}");
        }
        assert!(text.contains('≈') && text.contains('▼') && text.contains('◘'));
        assert!(text.contains("LEVEL 03"));
    }

    #[test]
    fn without_a_passage_it_decrypts_and_keys_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ready(dir.path(), 3);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .passage = None;
        let mut s = ExitsScreen::new(&mut ctx);
        assert!(rows(&s, &ctx).join("\n").contains("DECRYPTING..."));
        assert!(matches!(s.handle_key(press_char('1'), &mut ctx), Nav::Stay));
        assert!(ctx.run.as_ref().unwrap().exit_taken.is_none());
    }
}
