//! The Self screen: a mandala that lights one quadrant per completed act, and the player's
//! stats, integration meter and deck.
//!
//! An act is *complete* when its last depth is below the current depth, or equals it and the
//! level there has a chronicle entry with a choice (the player has finished that level's story).
//! Quadrants run clockwise from the top left in act order (act 1 top left, act 2 top right,
//! act 3 bottom right, act 4 bottom left); acts beyond the fourth have no quadrant.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Nav, ScreenImpl};
use crate::game::programs::{self, Program};
use crate::game::state::{EGO_MAX, INTEGRATION_MAX, INTEGRATION_MIN, RunState};
use crate::pack::model::StoryPack;
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::progress::{self, BarStyle};

/// EGO restored by one MANDALA.SYS.
pub const MANDALA_RESTORE: i32 = 30;

const MANDALA_W: i32 = 23;
const MANDALA_H: i32 = 11;
const MANDALA_X: u16 = 3;
const MANDALA_Y: u16 = 2;
const PANEL_X: u16 = 32;
/// Cells between the meter's arrowheads.
const METER_CELLS: i32 = 9;

/// Which acts of the pack are complete for this run, in act order.
pub fn acts_completed(run: &RunState, pack: &StoryPack) -> Vec<bool> {
    pack.meta
        .act
        .iter()
        .map(|act| {
            let last = act.depths[1];
            last < run.depth
                || (last == run.depth
                    && run
                        .chronicle
                        .entry(last)
                        .is_some_and(|e| e.choice.is_some()))
        })
        .collect()
}

/// The glyph and colour at the mandala's centre for an integration value.
pub fn centre_glyph(integration: i32) -> (char, Dos) {
    if integration <= -4 {
        ('◘', Dos::Red)
    } else if integration >= 4 {
        ('☼', Dos::Yellow)
    } else {
        ('○', Dos::LightGray)
    }
}

/// `REPRESSED ◄────■────► INTEGRATED`, the marker at integration -10 (left) .. 10 (right).
pub fn meter(integration: i32) -> String {
    let span = INTEGRATION_MAX - INTEGRATION_MIN;
    let pos = ((integration.clamp(INTEGRATION_MIN, INTEGRATION_MAX) - INTEGRATION_MIN)
        * (METER_CELLS - 1)
        + span / 2)
        / span;
    let cells: String = (0..METER_CELLS)
        .map(|i| if i == pos { '■' } else { '─' })
        .collect();
    format!("REPRESSED ◄{cells}► INTEGRATED")
}

/// One mandala cell: its glyph and the quadrant (0..4, clockwise from top left) it belongs to
/// (`None` on the axes, which are always dim).
fn mandala_cell(x: i32, y: i32) -> Option<(char, Option<usize>)> {
    let (cx, cy) = (MANDALA_W / 2, MANDALA_H / 2);
    let (dx, dy) = (x - cx, y - cy);
    let r = f64::from(dx).hypot(f64::from(dy * 2));
    let quadrant = match (dx.signum(), dy.signum()) {
        (-1, -1) => Some(0),
        (1, -1) => Some(1),
        (1, 1) => Some(2),
        (-1, 1) => Some(3),
        _ => None,
    };
    let glyph = if (r - 10.0).abs() < 0.9 {
        '▒'
    } else if (r - 6.2).abs() < 0.8 {
        '░'
    } else if (r - 2.7).abs() < 0.8 {
        '▒'
    } else if dx.abs() == dy.abs() * 2 && r < 9.0 && r > 3.4 {
        '∙'
    } else if dy == 0 && dx.abs() <= 10 {
        '─'
    } else if dx == 0 && dy.abs() <= 5 {
        '│'
    } else {
        return None;
    };
    Some((glyph, quadrant))
}

/// The Self screen.
pub struct PsycheScreen {
    notice: String,
}

impl PsycheScreen {
    pub fn new(_ctx: &mut Ctx) -> Self {
        Self {
            notice: String::new(),
        }
    }

    /// Key `U`: spend one MANDALA.SYS charge to restore EGO.
    fn use_mandala(&mut self, ctx: &mut Ctx) {
        let Some(run) = ctx.run_mut() else {
            return;
        };
        let name = Program::Mandala.file_name();
        if !programs::owns(&run.deck, Program::Mandala) {
            self.notice = format!("You have no {name}.");
            return;
        }
        if run.stats.ego >= EGO_MAX {
            self.notice = "Your EGO is already at maximum.".to_string();
            return;
        }
        let before = run.stats.ego;
        if programs::spend(&mut run.deck, Program::Mandala) {
            run.stats.add_ego(MANDALA_RESTORE);
            self.notice = format!("{name} runs. EGO +{}.", run.stats.ego - before);
            ctx.autosave();
        }
    }

    fn render_mandala(buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let (Some(run), p) = (ctx.run_ref(), &ctx.palette) else {
            return;
        };
        let done = acts_completed(run, &ctx.pack);
        let lit: Vec<Dos> = ctx
            .pack
            .meta
            .act
            .iter()
            .zip(&done)
            .map(|(a, d)| {
                if *d {
                    crate::palette::chrome_dos(a.palette)
                } else {
                    Dos::DarkGray
                }
            })
            .collect();
        for y in 0..MANDALA_H {
            for x in 0..MANDALA_W {
                let Some((glyph, quadrant)) = mandala_cell(x, y) else {
                    continue;
                };
                let colour = quadrant
                    .and_then(|q| lit.get(q))
                    .copied()
                    .unwrap_or(Dos::DarkGray);
                let mut s = [0u8; 4];
                put_str(
                    buf,
                    area.x + MANDALA_X + u16::try_from(x).unwrap_or(0),
                    area.y + MANDALA_Y + u16::try_from(y).unwrap_or(0),
                    glyph.encode_utf8(&mut s),
                    1,
                    p.style(colour, Dos::Black),
                );
            }
        }
        let (glyph, colour) = centre_glyph(run.stats.integration);
        let mut s = [0u8; 4];
        put_str(
            buf,
            area.x + MANDALA_X + u16::try_from(MANDALA_W / 2).unwrap_or(0),
            area.y + MANDALA_Y + u16::try_from(MANDALA_H / 2).unwrap_or(0),
            glyph.encode_utf8(&mut s),
            1,
            p.style(colour, Dos::Black),
        );
        for (i, (act, d)) in ctx.pack.meta.act.iter().zip(&done).take(4).enumerate() {
            let colour = if *d {
                crate::palette::chrome_dos(act.palette)
            } else {
                Dos::DarkGray
            };
            let y = area.y + MANDALA_Y + 13 + u16::try_from(i).unwrap_or(0);
            put_str(
                buf,
                area.x + MANDALA_X,
                y,
                &format!("■ {}", act.name),
                20,
                p.style(colour, Dos::Black),
            );
        }
    }
}

impl ScreenImpl for PsycheScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match classify(&key) {
            Key::Esc => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'q') => return Nav::Back,
            Key::Char(c) if c.eq_ignore_ascii_case(&'u') => self.use_mandala(ctx),
            _ => {}
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
        let title = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), title);
        put_str(
            buf,
            area.x,
            area.y,
            " SELF ─ the board under the board",
            w,
            title,
        );
        let Some(run) = ctx.run_ref() else {
            return;
        };
        Self::render_mandala(buf, area, ctx);

        let label = ctx.fg(Dos::LightCyan);
        let value = ctx.fg(Dos::White);
        let x = area.x + PANEL_X;
        let room = w - PANEL_X - 1;
        let row = |buf: &mut Buffer, y: u16, name: &str, text: &str| {
            put_str(buf, x, area.y + y, name, 12, label);
            put_str(buf, x + 12, area.y + y, text, room - 12, value);
        };
        row(buf, 2, "HANDLE", &run.handle);
        let clock = ctx
            .clock()
            .map(|c| format!("{} {}", c.hms(), c.date))
            .unwrap_or_default();
        row(buf, 3, "LEVEL", &format!("{:02}   {clock}", run.depth));
        row(buf, 4, "EGO", &format!("{:>3}/{EGO_MAX}", run.stats.ego));
        progress::render(
            buf,
            x + 22,
            area.y + 4,
            20,
            run.stats.ego as f32 / EGO_MAX as f32,
            BarStyle {
                filled: ctx.fg(Dos::LightGreen),
                empty: ctx.fg(Dos::DarkGray),
            },
            false,
        );
        row(buf, 5, "RATIO", &format!("{:04}K", run.stats.ratio_kb));
        row(buf, 6, "SEED", &run.seed.to_string());
        row(
            buf,
            8,
            "INTEGRATION",
            &format!("{:+}", run.stats.integration),
        );
        put_str(
            buf,
            x,
            area.y + 9,
            &meter(run.stats.integration),
            room,
            value,
        );
        put_str(buf, x, area.y + 11, "DECK", room, label);
        put_str(
            buf,
            x + 2,
            area.y + 12,
            "PAYLOAD.PIF   (implicit)",
            room - 2,
            value,
        );
        for (i, (program, n)) in run
            .deck
            .iter()
            .filter(|(pr, _)| programs::owns(&run.deck, **pr))
            .enumerate()
        {
            let text = if program.is_persistent() {
                format!("{:<13} installed", program.file_name())
            } else {
                format!(
                    "{:<13} {n} charge{}",
                    program.file_name(),
                    if *n == 1 { "" } else { "s" }
                )
            };
            let y = area.y + 13 + u16::try_from(i).unwrap_or(0);
            if y < area.y + 22 {
                put_str(buf, x + 2, y, &text, room - 2, value);
            }
        }
        if !self.notice.is_empty() {
            put_str(
                buf,
                area.x + 1,
                area.y + 22,
                &self.notice,
                w - 2,
                ctx.fg(Dos::Yellow),
            );
        }
        put_str(
            buf,
            area.x,
            area.y + 23,
            " U Use MANDALA.SYS (+30 EGO)   Esc Back",
            w,
            ctx.fg(Dos::LightGray),
        );
    }

    fn wants_status(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::chrome_dos;
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

    fn draw(ctx: &Ctx, screen: &PsycheScreen) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..25).map(|y| row_text(buf, buf.area, y)).collect()
    }

    #[test]
    fn acts_complete_when_their_last_depth_is_behind_you_or_the_final_choice_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 3);
        let pack = ctx.pack.clone();
        assert_eq!(acts_completed(ctx.run_ref().unwrap(), &pack), [false; 4]);
        // Finishing level 3 (its choice is in the chronicle) completes NIGREDO...
        assert!(ctx.run_mut().unwrap().apply_choice(&pack, 0));
        assert_eq!(
            acts_completed(ctx.run_ref().unwrap(), &pack),
            [true, false, false, false]
        );
        // ...and so does simply being at depth 4 or beyond.
        for (depth, expect) in [
            (4, [true, false, false, false]),
            (8, [true, true, false, false]),
            (12, [true, true, true, false]),
            (16, [true, true, true, true]),
        ] {
            ctx.run_mut().unwrap().depth = depth;
            assert_eq!(
                acts_completed(ctx.run_ref().unwrap(), &pack),
                expect,
                "depth {depth}"
            );
        }
    }

    #[test]
    fn centre_glyph_follows_the_integration_thresholds() {
        assert_eq!(centre_glyph(-10), ('◘', Dos::Red));
        assert_eq!(centre_glyph(-4), ('◘', Dos::Red));
        assert_eq!(centre_glyph(-3), ('○', Dos::LightGray));
        assert_eq!(centre_glyph(0), ('○', Dos::LightGray));
        assert_eq!(centre_glyph(3), ('○', Dos::LightGray));
        assert_eq!(centre_glyph(4), ('☼', Dos::Yellow));
        assert_eq!(centre_glyph(10), ('☼', Dos::Yellow));
    }

    #[test]
    fn the_meter_marker_runs_from_repressed_to_integrated() {
        assert_eq!(meter(0), "REPRESSED ◄────■────► INTEGRATED");
        assert_eq!(meter(-10), "REPRESSED ◄■────────► INTEGRATED");
        assert_eq!(meter(10), "REPRESSED ◄────────■► INTEGRATED");
        assert_eq!(meter(-99), meter(-10));
        let marker = |i: i32| meter(i).chars().position(|c| c == '■').unwrap();
        assert!((-10..10).all(|i| marker(i) <= marker(i + 1)));
    }

    #[test]
    fn completed_acts_light_their_quadrant_in_the_act_chrome_and_the_centre_shows_integration() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 8);
        ctx.run_mut().unwrap().stats.integration = 5;
        let screen = PsycheScreen::new(&mut ctx);
        let buf = draw(&ctx, &screen);
        let at = |dx: i32, dy: i32| {
            let x = u16::try_from(i32::from(MANDALA_X) + MANDALA_W / 2 + dx).unwrap();
            let y = u16::try_from(i32::from(MANDALA_Y) + MANDALA_H / 2 + dy).unwrap();
            buf[(x, y)].clone()
        };
        let acts = &ctx.pack.meta.act;
        let lit = |i: usize| ctx.palette.color(chrome_dos(acts[i].palette));
        let dark = ctx.palette.color(Dos::DarkGray);
        // top left, top right, bottom right, bottom left
        let quadrants = [(-4, -2), (4, -2), (4, 2), (-4, 2)];
        let expected = [lit(0), lit(1), dark, dark];
        for ((dx, dy), colour) in quadrants.iter().zip(expected) {
            let cell = at(*dx, *dy);
            assert_ne!(cell.symbol(), " ", "mandala cell at {dx},{dy}");
            assert_eq!(cell.fg, colour, "quadrant at {dx},{dy}");
        }
        let centre = at(0, 0);
        assert_eq!(centre.symbol(), "☼");
        assert_eq!(centre.fg, ctx.palette.color(Dos::Yellow));
        let r = rows(&buf);
        assert!(
            r.iter()
                .any(|l| l.contains("INTEGRATION") && l.contains("+5"))
        );
        assert!(r.iter().any(|l| l.contains("REPRESSED ◄")));
    }

    #[test]
    fn stats_deck_seed_clock_and_depth_are_shown() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 2);
        {
            let run = ctx.run_mut().unwrap();
            run.stats.ego = 64;
            run.stats.ratio_kb = 123;
            run.deck.insert(Program::Ghost, 2);
            run.deck.insert(Program::IcePick, 1);
        }
        let screen = PsycheScreen::new(&mut ctx);
        let text = rows(&draw(&ctx, &screen)).join("\n");
        for needle in [
            "HANDLE",
            "Tester",
            "02   23:08:00 12/31/99",
            " 64/100",
            "0123K",
            "42",
            "PAYLOAD.PIF",
            "GHOST.COM     2 charges",
            "ICEPICK.EXE   installed",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
    }

    #[test]
    fn u_without_a_mandala_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        ctx.run_mut().unwrap().stats.ego = 40;
        let mut screen = PsycheScreen::new(&mut ctx);
        screen.handle_key(press_char('u'), &mut ctx);
        assert_eq!(ctx.run_ref().unwrap().stats.ego, 40);
        assert!(rows(&draw(&ctx, &screen))[22].contains("You have no MANDALA.SYS."));
    }

    #[test]
    fn u_restores_30_ego_capped_at_100_and_spends_one_charge() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        {
            let run = ctx.run_mut().unwrap();
            run.stats.ego = 50;
            run.deck.insert(Program::Mandala, 2);
        }
        let mut screen = PsycheScreen::new(&mut ctx);
        screen.handle_key(press_char('U'), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ego, 80);
        assert_eq!(programs::charges(&run.deck, Program::Mandala), 1);
        assert!(rows(&draw(&ctx, &screen))[22].contains("EGO +30"));
        let saved = ctx.store.load("Tester").unwrap().unwrap();
        assert_eq!(saved.stats.ego, 80);
        assert_eq!(programs::charges(&saved.deck, Program::Mandala), 1);

        ctx.run_mut().unwrap().stats.ego = 90;
        screen.handle_key(press_char('u'), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ego, 100);
        assert!(!programs::owns(&run.deck, Program::Mandala));
        assert!(rows(&draw(&ctx, &screen))[22].contains("EGO +10"));
    }

    #[test]
    fn u_at_full_ego_keeps_the_charge() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx_at(dir.path(), 1);
        ctx.run_mut().unwrap().deck.insert(Program::Mandala, 1);
        let mut screen = PsycheScreen::new(&mut ctx);
        screen.handle_key(press_char('u'), &mut ctx);
        let run = ctx.run_ref().unwrap();
        assert_eq!(run.stats.ego, 100);
        assert_eq!(programs::charges(&run.deck, Program::Mandala), 1);
        assert!(rows(&draw(&ctx, &screen))[22].contains("already at maximum"));
        assert!(matches!(
            screen.handle_key(press(KeyCode::Esc), &mut ctx),
            Nav::Back
        ));
    }
}
