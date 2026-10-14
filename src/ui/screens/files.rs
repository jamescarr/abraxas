//! The file library: the level's text files, and the two programs for sale (bought with RATIO
//! through a ZMODEM transfer).

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl};
use crate::game::programs::{self, Program};
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::progress::{self, BarStyle};
use crate::ui::widgets::viewer::Viewer;

/// Shown instead of a purchase when RATIO is below the price.
pub const LOW_RATIO: &str = "Your ratio is too low. Complete a door to earn credit.";

const TEXT_WIDTH: u16 = 76;
const TEXT_ROWS: usize = 17;
const LIST_TOP: u16 = 5;
const MESSAGE_ROW: u16 = 22;
const HINT_ROW: u16 = 23;

/// How long a ZMODEM transfer of `program` takes at `baud`: `clamp(cost/24, 2, 4)` seconds,
/// half a second when the modem is instant (baud 0).
pub fn transfer_secs(program: Program, baud: u32) -> f32 {
    if baud == 0 {
        0.5
    } else {
        (program.cost_kb() as f32 / 24.0).clamp(2.0, 4.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// Index into `place.text_files`.
    Text(usize),
    Program(Program),
}

enum Mode {
    List,
    Text(Viewer),
    Confirm(Program),
    Transfer {
        program: Program,
        elapsed: f32,
        total: f32,
    },
}

/// The file library screen.
pub struct FilesScreen {
    items: Vec<Item>,
    selected: usize,
    mode: Mode,
    notice: String,
}

/// Text files first, then the programs on offer that the player may still buy.
fn collect_items(ctx: &Ctx) -> Vec<Item> {
    let mut items: Vec<Item> = ctx
        .place()
        .map(|p| (0..p.text_files.len()).map(Item::Text).collect())
        .unwrap_or_default();
    if let (Some(level), Some(run)) = (ctx.level(), ctx.run_ref()) {
        items.extend(
            level
                .programs_for_sale
                .iter()
                .copied()
                .filter(|p| !(p.is_persistent() && run.deck_owns(*p)))
                .map(Item::Program),
        );
    }
    items
}

impl FilesScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        Self {
            items: collect_items(ctx),
            selected: 0,
            mode: Mode::List,
            notice: String::new(),
        }
    }

    fn activate(&mut self, index: usize, ctx: &Ctx) {
        let Some(item) = self.items.get(index).copied() else {
            return;
        };
        self.selected = index;
        self.notice.clear();
        match item {
            Item::Text(i) => {
                if let Some(file) = ctx.place().and_then(|p| p.text_files.get(i)) {
                    self.mode = Mode::Text(Viewer::from_text(
                        &file.body,
                        TEXT_WIDTH,
                        TEXT_ROWS,
                        &ctx.palette,
                    ));
                }
            }
            Item::Program(program) => {
                let ratio = ctx.run_ref().map_or(0, |r| r.stats.ratio_kb);
                if ratio < program.cost_kb() {
                    self.notice = LOW_RATIO.to_string();
                } else {
                    self.mode = Mode::Confirm(program);
                }
            }
        }
    }

    /// The transfer finished: take the RATIO, add the program, save.
    fn complete(&mut self, program: Program, ctx: &mut Ctx) {
        self.mode = Mode::List;
        let bought = ctx.run_mut().is_some_and(|r| r.buy(program));
        if bought {
            ctx.autosave();
            self.notice = format!("{} received. It is in your deck.", program.file_name());
        } else {
            self.notice = LOW_RATIO.to_string();
        }
        self.items = collect_items(ctx);
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    fn key_list(&mut self, key: &KeyEvent, ctx: &Ctx) -> Nav {
        let count = self.items.len();
        match classify(key) {
            Key::Esc => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'q') => return Nav::Back,
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Key::Enter => self.activate(self.selected, ctx),
            Key::Char(c) => {
                if let Some(n) = c.to_digit(10).filter(|n| *n >= 1)
                    && let Ok(i) = usize::try_from(n - 1)
                    && i < count
                {
                    self.activate(i, ctx);
                }
            }
            _ => {}
        }
        Nav::Stay
    }
}

fn bytes_of(program: Program) -> u32 {
    program.cost_kb() * 1024
}

fn eta_text(remaining: f32) -> String {
    let secs = remaining.max(0.0).ceil() as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl ScreenImpl for FilesScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match &mut self.mode {
            Mode::List => return self.key_list(&key, ctx),
            Mode::Text(viewer) => {
                if !viewer.handle_key(&key)
                    && matches!(classify(&key), Key::Esc | Key::Char('q' | 'Q'))
                {
                    self.mode = Mode::List;
                }
            }
            Mode::Confirm(program) => {
                let program = *program;
                match classify(&key) {
                    Key::Char('y' | 'Y') | Key::Enter => {
                        self.mode = Mode::Transfer {
                            program,
                            elapsed: 0.0,
                            total: transfer_secs(program, ctx.baud()),
                        };
                        self.notice.clear();
                    }
                    Key::Char('n' | 'N') | Key::Esc => self.mode = Mode::List,
                    _ => {}
                }
            }
            Mode::Transfer { .. } => {
                if classify(&key) == Key::Esc {
                    self.mode = Mode::List;
                    self.notice = "Transfer aborted. You were not charged.".to_string();
                }
            }
        }
        Nav::Stay
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if let Mode::Transfer {
            program,
            elapsed,
            total,
        } = &mut self.mode
        {
            *elapsed += dt.as_secs_f32();
            if *elapsed >= *total {
                let program = *program;
                self.complete(program, ctx);
            }
        }
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
            &format!(" {bbs} ─ File Library"),
            w,
            title,
        );
        let ratio = ctx.run_ref().map_or(0, |r| r.stats.ratio_kb);
        put_str(
            buf,
            area.x + 1,
            area.y + 1,
            &format!("RATIO {ratio:04}K"),
            14,
            ctx.fg(Dos::Yellow),
        );
        let mut deck = String::from("DECK: PAYLOAD.PIF");
        if let Some(run) = ctx.run_ref() {
            for (program, n) in &run.deck {
                if programs::owns(&run.deck, *program) {
                    if program.is_persistent() {
                        deck.push_str(&format!("  {}", program.file_name()));
                    } else {
                        deck.push_str(&format!("  {} x{n}", program.file_name()));
                    }
                }
            }
        }
        put_str(
            buf,
            area.x + 16,
            area.y + 1,
            &deck,
            w - 17,
            ctx.fg(Dos::LightGray),
        );
        let hint = match &self.mode {
            Mode::List => " ↑↓ Select   Enter Open/Download   1-9 Quick   Esc Back",
            Mode::Text(_) => " ↑↓ PgUp PgDn Scroll   Esc Close",
            Mode::Confirm(_) => " Y Download   N Cancel",
            Mode::Transfer { .. } => " Esc Abort transfer",
        };
        put_str(
            buf,
            area.x,
            area.y + HINT_ROW,
            hint,
            w,
            ctx.fg(Dos::LightGray),
        );
        match &self.mode {
            Mode::List | Mode::Confirm(_) => self.render_list(buf, area, ctx),
            Mode::Text(viewer) => self.render_text(viewer, buf, area, ctx),
            Mode::Transfer {
                program,
                elapsed,
                total,
            } => render_transfer(buf, area, ctx, *program, *elapsed, *total),
        }
        let msg_y = area.y + MESSAGE_ROW;
        if let Mode::Confirm(program) = &self.mode {
            let q = format!(
                "Download {} for {}K? [Y/n]",
                program.file_name(),
                program.cost_kb()
            );
            put_str(buf, area.x + 1, msg_y, &q, w - 2, ctx.fg(Dos::White));
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
    }

    fn wants_status(&self) -> bool {
        true
    }
}

impl FilesScreen {
    fn render_list(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, ctx: &Ctx) {
        let p = &ctx.palette;
        let w = area.width;
        put_str(
            buf,
            area.x,
            area.y + 3,
            " #  File          Size Description",
            w,
            p.style(Dos::LightCyan, Dos::Black),
        );
        put_str(
            buf,
            area.x,
            area.y + 4,
            &"─".repeat(usize::from(w)),
            w,
            ctx.fg(Dos::DarkGray),
        );
        for (row, item) in self.items.iter().enumerate() {
            let y = area.y + LIST_TOP + u16::try_from(row).unwrap_or(0);
            let (name, size, desc, program) = match item {
                Item::Text(i) => match ctx.place().and_then(|pl| pl.text_files.get(*i)) {
                    Some(f) => (
                        f.name.clone(),
                        format!("{}", f.body.len()),
                        f.description.clone(),
                        false,
                    ),
                    None => continue,
                },
                Item::Program(pr) => (
                    pr.file_name().to_string(),
                    format!("{}K", pr.cost_kb()),
                    pr.effect().to_string(),
                    true,
                ),
            };
            let line = format!("{:>2}  {name:<12} {size:>5} {desc}", row + 1);
            let style = if row == self.selected {
                p.style(Dos::Black, Dos::LightGray)
            } else if program {
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
        if self.items.is_empty() {
            put_str(
                buf,
                area.x + 2,
                area.y + LIST_TOP,
                "No files.",
                w,
                ctx.fg(Dos::LightGray),
            );
        }
    }

    fn render_text(
        &self,
        viewer: &Viewer,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        ctx: &Ctx,
    ) {
        let Some(Item::Text(i)) = self.items.get(self.selected).copied() else {
            return;
        };
        let Some(file) = ctx.place().and_then(|p| p.text_files.get(i)) else {
            return;
        };
        put_str(
            buf,
            area.x + 1,
            area.y + 2,
            &format!("{}  {}", file.name, file.description),
            area.width - 2,
            ctx.fg(Dos::Yellow),
        );
        viewer.render(
            buf,
            Rect::new(
                area.x + 2,
                area.y + 3,
                TEXT_WIDTH,
                u16::try_from(TEXT_ROWS + 1).unwrap_or(18),
            ),
            &ctx.palette,
        );
    }
}

fn render_transfer(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    ctx: &Ctx,
    program: Program,
    elapsed: f32,
    total: f32,
) {
    let frac = (elapsed / total).clamp(0.0, 1.0);
    let bytes = bytes_of(program);
    let done = (bytes as f32 * frac) as u32;
    let cps = (bytes as f32 / total) as u32;
    let box_area = Rect::new(area.x + 4, area.y + 8, area.width - 8, 7);
    let inner = boxed(
        buf,
        box_area,
        Some("ZMODEM"),
        BoxKind::Single,
        ctx.fg(Dos::LightGray),
    );
    let line = format!(
        "ZMODEM  Receiving {}  Bytes {done}/{bytes}  CPS {cps}  ETA {}",
        program.file_name(),
        eta_text(total - elapsed)
    );
    put_str(
        buf,
        inner.x + 1,
        inner.y + 1,
        &line,
        inner.width.saturating_sub(2),
        ctx.fg(Dos::White),
    );
    progress::render(
        buf,
        inner.x + 1,
        inner.y + 3,
        inner.width.saturating_sub(8),
        frac,
        BarStyle {
            filled: ctx.fg(Dos::LightGreen),
            empty: ctx.fg(Dos::DarkGray),
        },
        true,
    );
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

    /// A depth-1 run with 200K of RATIO and GHOST.COM + ICEPICK.EXE for sale.
    fn setup(dir: &std::path::Path) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, 1);
        let run = ctx.run_mut().unwrap();
        run.stats.ratio_kb = 200;
        run.level_mut().unwrap().programs_for_sale = vec![Program::Ghost, Program::IcePick];
        ctx
    }

    fn draw(screen: &FilesScreen, ctx: &Ctx) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        let buf: &Buffer = term.backend().buffer();
        (0..25).map(|y| row_text(buf, buf.area, y)).collect()
    }

    fn text_count(ctx: &Ctx) -> usize {
        ctx.place().unwrap().text_files.len()
    }

    /// Select program `n` (0-based among the programs) with its number key.
    fn select_program(screen: &mut FilesScreen, ctx: &mut Ctx, n: usize) {
        let key = char::from_digit(u32::try_from(text_count(ctx) + n + 1).unwrap(), 10).unwrap();
        screen.handle_key(press_char(key), ctx);
    }

    #[test]
    fn listing_shows_ratio_deck_text_files_and_programs_with_cost_and_effect() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        let screen = FilesScreen::new(&mut ctx);
        let lines = draw(&screen, &ctx);
        assert!(lines[1].contains("RATIO 0200K") && lines[1].contains("PAYLOAD.PIF"));
        let files = ctx.place().unwrap().text_files.clone();
        assert!(!files.is_empty());
        for (i, f) in files.iter().enumerate() {
            assert!(lines[5 + i].contains(&f.name), "{:?}", lines[5 + i]);
        }
        let n = files.len();
        assert!(lines[5 + n].contains("GHOST.COM") && lines[5 + n].contains("96K"));
        assert!(lines[5 + n].contains("trace rate x0.6"));
        assert!(lines[6 + n].contains("ICEPICK.EXE") && lines[6 + n].contains("80K"));
    }

    #[test]
    fn a_text_file_opens_in_the_viewer_and_esc_returns_to_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        let mut screen = FilesScreen::new(&mut ctx);
        screen.handle_key(press_char('1'), &mut ctx);
        let file = ctx.place().unwrap().text_files[0].clone();
        let lines = draw(&screen, &ctx);
        assert!(lines[2].contains(&file.name));
        let first = file.body.split_whitespace().next().unwrap();
        assert!(lines[3].contains(first), "{:?}", lines[3]);
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Stay
        ));
        assert!(draw(&screen, &ctx)[3].contains("File"));
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
    }

    #[test]
    fn purchase_flow_charges_the_ratio_and_adds_the_program_after_the_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        let mut screen = FilesScreen::new(&mut ctx);
        select_program(&mut screen, &mut ctx, 0);
        assert!(draw(&screen, &ctx)[22].contains("Download GHOST.COM for 96K? [Y/n]"));
        assert_eq!(ctx.run_ref().unwrap().stats.ratio_kb, 200);

        screen.handle_key(press_char('y'), &mut ctx);
        screen.tick(Duration::from_millis(300), &mut ctx);
        let lines = draw(&screen, &ctx);
        let zm = lines
            .iter()
            .find(|l| l.contains("ZMODEM  Receiving GHOST.COM"))
            .unwrap();
        assert!(
            zm.contains("Bytes ")
                && zm.contains("/98304")
                && zm.contains("CPS ")
                && zm.contains("ETA 0:01"),
            "{zm:?}"
        );
        assert!(lines.iter().any(|l| l.contains("█") && l.contains('%')));
        assert_eq!(
            ctx.run_ref().unwrap().stats.ratio_kb,
            200,
            "not charged mid-transfer"
        );

        screen.tick(Duration::from_millis(300), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ratio_kb, 104);
        assert_eq!(programs::charges(&run.deck, Program::Ghost), 1);
        assert!(draw(&screen, &ctx)[22].contains("GHOST.COM received"));
        let saved = ctx.store.load("Tester").unwrap().unwrap();
        assert_eq!(saved.stats.ratio_kb, 104);
        assert_eq!(programs::charges(&saved.deck, Program::Ghost), 1);
    }

    #[test]
    fn transfer_lasts_cost_over_24_seconds_clamped_to_2_to_4_and_half_a_second_at_baud_0() {
        assert_eq!(transfer_secs(Program::Decrypt, 9600), 2.0);
        assert!((transfer_secs(Program::Buffer, 9600) - 64.0 / 24.0).abs() < 1e-6);
        assert!((transfer_secs(Program::IcePick, 9600) - 80.0 / 24.0).abs() < 1e-6);
        assert_eq!(transfer_secs(Program::Ghost, 9600), 4.0);
        assert_eq!(transfer_secs(Program::Ghost, 0), 0.5);

        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        ctx.config.display.baud = 9600;
        let mut screen = FilesScreen::new(&mut ctx);
        select_program(&mut screen, &mut ctx, 1);
        screen.handle_key(press_char('y'), &mut ctx);
        screen.tick(Duration::from_secs(3), &mut ctx);
        assert_eq!(ctx.run_ref().unwrap().stats.ratio_kb, 200);
        screen.tick(Duration::from_millis(400), &mut ctx);
        assert_eq!(ctx.run_ref().unwrap().stats.ratio_kb, 120);
    }

    #[test]
    fn persistent_programs_leave_the_list_and_consumables_stack() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        ctx.run_mut().unwrap().stats.ratio_kb = 500;
        let mut screen = FilesScreen::new(&mut ctx);
        let n = text_count(&ctx);
        for _ in 0..2 {
            select_program(&mut screen, &mut ctx, 0);
            screen.handle_key(press(KeyCode::Enter), &mut ctx);
            screen.tick(Duration::from_secs(1), &mut ctx);
        }
        assert_eq!(
            programs::charges(&ctx.run_ref().unwrap().deck, Program::Ghost),
            2
        );
        assert_eq!(ctx.run_ref().unwrap().stats.ratio_kb, 500 - 192);

        select_program(&mut screen, &mut ctx, 1);
        screen.handle_key(press_char('y'), &mut ctx);
        screen.tick(Duration::from_secs(1), &mut ctx);
        assert!(ctx.run_ref().unwrap().deck_owns(Program::IcePick));
        let lines = draw(&screen, &ctx);
        assert!(
            lines.iter().all(|l| !l.contains("80K")),
            "bought IcePick is hidden"
        );
        assert!(
            lines[5 + n].contains("GHOST.COM"),
            "consumables stay for sale"
        );
        assert!(lines[1].contains("GHOST.COM x2") && lines[1].contains("ICEPICK.EXE"));
        assert_eq!(screen.items.len(), n + 1);
    }

    #[test]
    fn too_low_a_ratio_blocks_the_download_without_changing_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        ctx.run_mut().unwrap().stats.ratio_kb = 95;
        let mut screen = FilesScreen::new(&mut ctx);
        select_program(&mut screen, &mut ctx, 0);
        let lines = draw(&screen, &ctx);
        assert!(lines[22].contains(LOW_RATIO));
        assert!(lines.iter().all(|l| !l.contains("[Y/n]")));
        screen.handle_key(press_char('y'), &mut ctx);
        screen.tick(Duration::from_secs(5), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ratio_kb, 95);
        assert!(run.deck.is_empty());
    }

    #[test]
    fn declining_or_aborting_a_transfer_charges_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = setup(dir.path());
        let mut screen = FilesScreen::new(&mut ctx);
        select_program(&mut screen, &mut ctx, 0);
        screen.handle_key(press_char('n'), &mut ctx);
        screen.tick(Duration::from_secs(5), &mut ctx);
        assert_eq!(ctx.run_ref().unwrap().stats.ratio_kb, 200);

        select_program(&mut screen, &mut ctx, 0);
        screen.handle_key(press_char('y'), &mut ctx);
        screen.tick(Duration::from_millis(200), &mut ctx);
        screen.handle_key(press(KeyCode::Esc), &mut ctx);
        screen.tick(Duration::from_secs(5), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ratio_kb, 200);
        assert!(run.deck.is_empty());
        assert!(draw(&screen, &ctx)[22].contains("not charged"));
    }
}
