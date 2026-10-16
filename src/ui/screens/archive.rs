//! The archive: the chronicle of completed levels, with a detail view per entry.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl};
use crate::game::chronicle::ChronicleEntry;
use crate::oracle;
use crate::palette::Dos;
use crate::text::wrap;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::viewer::Viewer;

const LIST_TOP: u16 = 4;
const LIST_ROWS: usize = 18;
const DETAIL_WIDTH: u16 = 54;
const DETAIL_ROWS: usize = 17;
const HEX_X: u16 = 63;
const HEX_TOP: u16 = 4;
const YANG: &str = "███████████";
const YIN: &str = "████   ████";

/// One chronicle line: `L03 THE BLACK SUN  CLASS 2  63 AFTER COMPLETION`.
pub fn list_line(entry: &ChronicleEntry) -> String {
    format!(
        "L{:02} {}  CLASS {}  {}",
        entry.depth,
        entry.title.to_uppercase(),
        entry.class,
        oracle::title(entry.omen)
    )
}

/// The detail text: level card, summary, the choice made (stance hidden) and the exit taken.
fn detail_text(entry: &ChronicleEntry) -> String {
    let mut text = entry.level_card.clone();
    text.push_str("\n\n");
    text.push_str(&entry.summary);
    if let Some(choice) = &entry.choice {
        text.push_str(&format!("\n\nYou chose: \"{}\"", choice.text));
    }
    if let Some(exit) = &entry.exit {
        text.push_str(&format!("\n\nYou took: \"{}\"", exit.text));
    }
    text
}

/// The archive screen.
pub struct ArchiveScreen {
    selected: usize,
    /// The open detail view: entry index and its text.
    detail: Option<(usize, Viewer)>,
}

impl ArchiveScreen {
    pub fn new(_ctx: &mut Ctx) -> Self {
        Self {
            selected: 0,
            detail: None,
        }
    }

    fn open(&mut self, ctx: &Ctx) {
        let Some(entry) = ctx
            .run_ref()
            .and_then(|r| r.chronicle.entries.get(self.selected))
        else {
            return;
        };
        let viewer =
            Viewer::from_text(&detail_text(entry), DETAIL_WIDTH, DETAIL_ROWS, &ctx.palette);
        self.detail = Some((self.selected, viewer));
    }

    fn render_list(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let w = area.width;
        let entries = ctx.run_ref().map_or(&[][..], |r| &r.chronicle.entries[..]);
        put_str(
            buf,
            area.x,
            area.y + 2,
            " Completed levels",
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
        if entries.is_empty() {
            put_str(
                buf,
                area.x + 2,
                area.y + LIST_TOP,
                "No completed levels yet.",
                w,
                ctx.fg(Dos::LightGray),
            );
            return;
        }
        let top = self.selected.saturating_sub(LIST_ROWS - 1);
        for (row, (i, entry)) in entries
            .iter()
            .enumerate()
            .skip(top)
            .take(LIST_ROWS)
            .enumerate()
        {
            let y = area.y + LIST_TOP + u16::try_from(row).unwrap_or(0);
            let style = if i == self.selected {
                p.style(Dos::Black, Dos::LightGray)
            } else {
                p.style(Dos::White, Dos::Black)
            };
            put_str(
                buf,
                area.x,
                y,
                &format!(" {:<width$}", list_line(entry), width = usize::from(w) - 1),
                w,
                style,
            );
        }
    }

    fn render_detail(
        &self,
        buf: &mut Buffer,
        area: Rect,
        ctx: &Ctx,
        index: usize,
        viewer: &Viewer,
    ) {
        let Some(entry) = ctx.run_ref().and_then(|r| r.chronicle.entries.get(index)) else {
            return;
        };
        put_str(
            buf,
            area.x + 1,
            area.y + 2,
            &format!(
                "L{:02} {}  CLASS {}",
                entry.depth,
                entry.title.to_uppercase(),
                entry.class
            ),
            DETAIL_WIDTH + 2,
            ctx.fg(Dos::Yellow),
        );
        viewer.render(
            buf,
            Rect::new(
                area.x + 2,
                area.y + 3,
                DETAIL_WIDTH,
                u16::try_from(DETAIL_ROWS + 1).unwrap_or(18),
            ),
            &ctx.palette,
        );
        let style = ctx.palette.style(ctx_chrome(ctx), Dos::Black);
        for (k, yang) in oracle::lines(entry.omen).iter().rev().enumerate() {
            let y = area.y + HEX_TOP + 2 * u16::try_from(k).unwrap_or(0);
            put_str(
                buf,
                area.x + HEX_X,
                y,
                if *yang { YANG } else { YIN },
                11,
                style,
            );
        }
        for (k, line) in wrap(&oracle::title(entry.omen), 22).iter().enumerate() {
            let y = area.y + HEX_TOP + 13 + u16::try_from(k).unwrap_or(0);
            put_str(buf, area.x + 58, y, line, 22, ctx.fg(Dos::White));
        }
    }
}

/// The current act's chrome as a DOS colour (for the hexagram).
fn ctx_chrome(ctx: &Ctx) -> Dos {
    crate::palette::chrome_dos(ctx.act_palette())
}

impl ScreenImpl for ArchiveScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if let Some((_, viewer)) = &mut self.detail {
            if !viewer.handle_key(&key) && matches!(classify(&key), Key::Esc | Key::Char('q' | 'Q'))
            {
                self.detail = None;
            }
            return Nav::Stay;
        }
        let count = ctx.run_ref().map_or(0, |r| r.chronicle.entries.len());
        match classify(&key) {
            Key::Esc => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'q') => return Nav::Back,
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = count.saturating_sub(1),
            Key::Enter => self.open(ctx),
            _ => {}
        }
        Nav::Stay
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let w = area.width;
        let title = ctx.palette.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), title);
        put_str(
            buf,
            area.x,
            area.y,
            " Archive ─ Chronicle of levels",
            w,
            title,
        );
        let hint = match &self.detail {
            Some((index, viewer)) => {
                self.render_detail(buf, area, ctx, *index, viewer);
                " ↑↓ PgUp PgDn Scroll   Esc Back to list"
            }
            None => {
                self.render_list(buf, area, ctx);
                " ↑↓ Select   Enter Read   Esc Back"
            }
        };
        put_str(buf, area.x, area.y + 23, hint, w, ctx.fg(Dos::LightGray));
    }

    fn wants_status(&self) -> bool {
        true
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
    use ratatui::crossterm::event::KeyCode;

    fn draw(screen: &ArchiveScreen, ctx: &Ctx) -> (ratatui::buffer::Buffer, Vec<String>) {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        let rows = (0..25).map(|y| row_text(&buf, buf.area, y)).collect();
        (buf, rows)
    }

    /// A run at depth 3 that has made its choice and picked an exit.
    fn finished_level(dir: &std::path::Path) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, 3);
        let pack = ctx.pack.clone();
        let run = ctx.run_mut().unwrap();
        assert!(run.apply_choice(&pack, 1));
        assert!(run.apply_exit(1));
        ctx
    }

    #[test]
    fn an_empty_chronicle_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 3);
        let mut screen = ArchiveScreen::new(&mut ctx);
        assert!(draw(&screen, &ctx).1[4].contains("No completed levels yet."));
        screen.handle_key(press(KeyCode::Enter), &mut ctx);
        assert!(screen.detail.is_none());
    }

    #[test]
    fn the_list_line_has_depth_title_class_and_omen() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = finished_level(dir.path());
        let entry = ctx.run_ref().unwrap().chronicle.entries[0].clone();
        let screen = ArchiveScreen::new(&mut ctx);
        let expected = format!(
            "L03 {}  CLASS {}  {}",
            entry.title.to_uppercase(),
            entry.class,
            oracle::title(entry.omen)
        );
        assert_eq!(list_line(&entry), expected);
        let rows = draw(&screen, &ctx).1;
        assert_eq!(rows[4].trim(), expected);
    }

    #[test]
    fn list_line_matches_the_documented_example() {
        let entry = ChronicleEntry {
            depth: 3,
            act_id: "nigredo".into(),
            act_name: "NIGREDO".into(),
            title: "The Black Sun".into(),
            class: 2,
            omen: 63,
            level_card: String::new(),
            summary: String::new(),
            choice: None,
            exit: None,
        };
        assert_eq!(
            list_line(&entry),
            "L03 THE BLACK SUN  CLASS 2  63 AFTER COMPLETION"
        );
    }

    #[test]
    fn detail_shows_card_summary_choice_exit_and_the_hexagram_without_the_stance() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = finished_level(dir.path());
        let entry = ctx.run_ref().unwrap().chronicle.entries[0].clone();
        let mut screen = ArchiveScreen::new(&mut ctx);
        screen.handle_key(press(KeyCode::Enter), &mut ctx);
        let (buf, rows) = draw(&screen, &ctx);
        let joined = rows[3..21].join("\n");
        let flat: String = joined.split_whitespace().collect::<Vec<_>>().join(" ");
        let first_card_words: Vec<&str> = entry.level_card.split_whitespace().take(4).collect();
        assert!(flat.contains(&first_card_words.join(" ")), "{flat}");
        let choice = entry.choice.as_ref().unwrap();
        let chose = format!("You chose: \"{}\"", choice.text);
        assert!(
            flat.contains(
                &chose
                    .split_whitespace()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            "{flat}"
        );
        assert!(flat.contains("You took: \""), "{flat}");
        assert!(
            !flat.contains(&format!("({})", choice.stance.as_str())),
            "{flat}"
        );
        assert!(rows[2].contains(&format!("L03 {}", entry.title.to_uppercase())));
        // The six hexagram lines, top line first, two rows apart.
        let lines = oracle::lines(entry.omen);
        for (k, yang) in lines.iter().rev().enumerate() {
            let y = 4 + 2 * u16::try_from(k).unwrap();
            let cells: String = (63..74).map(|x| buf[(x, y)].symbol().to_string()).collect();
            assert_eq!(cells, if *yang { YANG } else { YIN }, "line {k}");
        }
        assert!(
            rows[17].contains(&oracle::title(entry.omen)[..6]),
            "{:?}",
            rows[17]
        );
        // Esc closes the detail first, then leaves.
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Stay
        ));
        assert!(screen.detail.is_none());
        assert!(matches!(
            screen.handle_key(press_char('q'), &mut ctx),
            Nav::Back
        ));
    }
}
