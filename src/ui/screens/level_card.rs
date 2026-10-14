//! The level card: `LEVEL 07`, the title, the danger class, the field note, the omen hexagram
//! and the level's motifs. Shows `DECRYPTING LEVEL...` over line noise until the place exists.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{MenuScreen, Nav, Screen, ScreenImpl};
use crate::game::danger;
use crate::game::seed::ensure_level;
use crate::game::state::{GenSource, Phase};
use crate::oracle;
use crate::pack::model::{Archetype, ExitKind, VisionKind};
use crate::palette::Dos;
use crate::text::{truncate, wrap};
use crate::ui::app::{Ctx, FxCommand, Overlay};
use crate::ui::canvas::{WIDTH, fill, put_centered, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::visions::{HexagramCast, Vision, hexagram};

/// Width of the field-note column.
const NOTE_WIDTH: usize = 42;
/// Most rows the field note may use.
const NOTE_ROWS: usize = 8;
const NOTE_ROW: u16 = 8;
const NOTE_X: u16 = 3;
/// The hexagram column: x of its left edge and its width.
const HEX_X: u16 = 46;
const HEX_W: u16 = 32;
const MOTIF_ROW: u16 = 17;
const PROMPT_ROW: u16 = 19;
const FOOTER_ROW: u16 = 21;
/// Rows the screen owns (the status bar is row 24).
const BODY_ROWS: u16 = 24;
/// The lunar tint: hue shift toward cyan, in degrees and percent points.
const TINT_HUE: f32 = 180.0;
const TINT_SAT: f32 = 20.0;
const TINT_MS: u64 = 800;

/// The LevelCard screen.
pub struct LevelCardScreen {
    vision: Vision,
    hexagram: Option<HexagramCast>,
    tinted: bool,
}

impl LevelCardScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let Ctx { run, pack, .. } = ctx;
        let mut omen = None;
        if let Some(run) = run.as_mut() {
            let depth = run.depth;
            omen = Some(ensure_level(run, pack, depth).seed.omen);
        }
        let mut vision = Vision::new(
            VisionKind::Noise,
            ctx.opts.seed.unwrap_or(0),
            0,
            ctx.gradient(),
            ctx.calm(),
        );
        vision.set_density(0.35);
        Self {
            vision,
            hexagram: omen.map(|n| HexagramCast::new(n, true)),
            tinted: false,
        }
    }

    fn ready(ctx: &Ctx) -> bool {
        ctx.place().is_some()
    }

    fn footer(ctx: &Ctx) -> String {
        let Some(level) = ctx.level() else {
            return String::new();
        };
        let tag =
            |src: &Option<GenSource>| src.as_ref().map_or("pending".to_string(), GenSource::tag);
        let mut out = format!(
            "[place {}] [passage {}]",
            tag(&level.place_source),
            tag(&level.passage_source)
        );
        if level.interlude_source.is_some() {
            out.push_str(&format!(" [interlude {}]", tag(&level.interlude_source)));
        }
        out
    }

    fn draw_card(&self, buf: &mut Buffer, area: Rect, ctx: &Ctx) {
        let (Some(run), Some(level), Some(place)) = (ctx.run_ref(), ctx.level(), ctx.place())
        else {
            return;
        };
        let chrome = ctx.chrome();
        let seed = &level.seed;
        let shadow = run.depth > 0
            && run
                .exit_taken
                .as_ref()
                .is_some_and(|e| e.kind == ExitKind::Mirror);

        // The title box.
        let frame = Rect::new(area.x + 4, area.y + 1, area.width - 8, 6);
        let inner = boxed(
            buf,
            frame,
            None,
            BoxKind::Double,
            ratatui::style::Style::new()
                .fg(chrome)
                .bg(ctx.palette.color(Dos::Black)),
        );
        put_centered(
            buf,
            inner,
            inner.y,
            &format!("LEVEL {:02}", run.depth),
            ctx.fg(Dos::White),
        );
        let title = truncate(&place.title, usize::from(inner.width).saturating_sub(2));
        put_centered(
            buf,
            inner,
            inner.y + 1,
            &title,
            ratatui::style::Style::new()
                .fg(chrome)
                .bg(ctx.palette.color(Dos::Black)),
        );
        let class = usize::from(seed.base_danger.clamp(1, 5));
        let bar = format!(
            "{}{}  CLASS {}",
            "▓".repeat(class),
            "░".repeat(5 - class),
            class
        );
        put_centered(buf, inner, inner.y + 2, &bar, ctx.fg(Dos::LightGray));
        put_centered(
            buf,
            inner,
            inner.y + 3,
            &danger::descriptor(seed.base_danger, shadow),
            ctx.fg(Dos::LightGray),
        );

        // The field note.
        let mut lines = wrap(&place.level_card, NOTE_WIDTH);
        if lines.len() > NOTE_ROWS {
            lines.truncate(NOTE_ROWS);
            if let Some(last) = lines.last_mut() {
                *last = truncate(&format!("{last} ..."), NOTE_WIDTH);
            }
        }
        for (i, line) in lines.iter().enumerate() {
            let y = area.y + NOTE_ROW + u16::try_from(i).unwrap_or(0);
            put_str(
                buf,
                area.x + NOTE_X,
                y,
                line,
                NOTE_WIDTH as u16,
                ctx.text_style(),
            );
        }

        // The omen.
        if let Some(cast) = &self.hexagram {
            let title_w = oracle::title(seed.omen).chars().count();
            let block_w = u16::try_from(title_w.max(14)).unwrap_or(HEX_W).min(HEX_W);
            let x = area.x + HEX_X + (HEX_W - block_w) / 2;
            let rect = Rect::new(x, area.y + NOTE_ROW, block_w, hexagram::HEIGHT);
            cast.render(rect, buf, chrome);
        }

        if !seed.motifs.is_empty() {
            let motifs = format!("motifs: {}", seed.motifs.join("; "));
            put_centered(
                buf,
                area,
                area.y + MOTIF_ROW,
                &truncate(&motifs, usize::from(area.width)),
                ctx.fg(Dos::DarkGray),
            );
        }
        put_centered(
            buf,
            area,
            area.y + PROMPT_ROW,
            "press any key",
            ctx.fg(Dos::LightCyan),
        );
        if ctx.is_dev_footer() {
            let footer = truncate(&Self::footer(ctx), usize::from(area.width));
            put_centered(
                buf,
                area,
                area.y + FOOTER_ROW,
                &footer,
                ctx.fg(Dos::DarkGray),
            );
        }
    }
}

impl ScreenImpl for LevelCardScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if classify(&key) == Key::Esc {
            ctx.overlay_request = Some(Overlay::ConfirmQuit);
            return Nav::Stay;
        }
        if !Self::ready(ctx) {
            return Nav::Stay;
        }
        if let Some(run) = ctx.run_mut()
            && run.phase == Phase::Arrival
        {
            run.phase = Phase::Hub {
                door_done: false,
                choice: None,
            };
        }
        ctx.autosave();
        ctx.fx.push(FxCommand::Coalesce { ms: 400 });
        Nav::To(Box::new(Screen::Menu(MenuScreen::new(ctx))))
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if !Self::ready(ctx) {
            self.vision.tick(dt);
            return Nav::Stay;
        }
        if !self.tinted {
            self.tinted = true;
            let syzygy = ctx
                .focus_character()
                .is_some_and(|c| c.archetype == Archetype::Syzygy);
            if syzygy && !ctx.calm() {
                ctx.fx.push(FxCommand::HslShiftFg {
                    area: Rect::new(0, 0, WIDTH, BODY_ROWS),
                    hue: TINT_HUE,
                    sat: TINT_SAT,
                    light: 0.0,
                    ms: TINT_MS,
                });
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        fill(buf, area, ctx.text_style());
        if Self::ready(ctx) {
            self.draw_card(buf, area, ctx);
        } else {
            let body = Rect::new(area.x, area.y, area.width, BODY_ROWS.min(area.height));
            self.vision.render(body, buf);
            put_centered(
                buf,
                area,
                area.y + 11,
                "  DECRYPTING LEVEL...  ",
                ctx.palette.style(Dos::White, Dos::Black),
            );
        }
    }

    fn wants_status(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::model::ExitDef;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    fn draw(screen: &LevelCardScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..25).map(|y| row_text(buf, buf.area, y)).collect()
    }

    fn card_at(depth: u32) -> (Ctx, LevelCardScreen, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        let s = LevelCardScreen::new(&mut ctx);
        (ctx, s, dir)
    }

    fn tint_requests(ctx: &Ctx) -> usize {
        ctx.fx
            .iter()
            .filter(|f| matches!(f, FxCommand::HslShiftFg { .. }))
            .count()
    }

    #[test]
    fn renders_the_black_sun_card() {
        let (ctx, s, _dir) = card_at(3);
        let r = rows(&draw(&s, &ctx));
        let seed = ctx.level().unwrap().seed.clone();
        assert_eq!(r[1].trim(), format!("╔{}╗", "═".repeat(70)));
        assert_eq!(r[2].trim(), format!("║{:^70}║", "LEVEL 03"));
        assert_eq!(r[3].trim(), format!("║{:^70}║", "THE BLACK SUN"));
        assert_eq!(r[4].trim(), format!("║{:^70}║", "▓▓░░░  CLASS 2"));
        assert_eq!(
            r[5].trim(),
            format!("║{:^70}║", "Safe │ Unstable │ Low entity count")
        );
        assert!(r[6].trim().starts_with('╚'));

        // The field note on the left, wrapped.
        let note = ctx.place().unwrap().level_card.clone();
        let first_word = note.split_whitespace().next().unwrap();
        assert!(r[8].starts_with(&format!("   {first_word}")), "{:?}", r[8]);
        assert!(r[8..16].iter().all(|l| l.chars().take(45).count() == 45));

        // The hexagram on the right: six settled lines, then the title.
        let lines = oracle::lines(seed.omen);
        for (row, yang) in (8..14).zip(lines.iter().rev()) {
            let right: String = r[row].chars().skip(46).collect();
            let right = right.trim();
            let want = if *yang {
                "███████████"
            } else {
                "████   ████"
            };
            assert_eq!(right, want, "row {row}");
        }
        assert!(r[15].contains(&oracle::title(seed.omen)), "{:?}", r[15]);

        assert_eq!(r[17].trim(), format!("motifs: {}", seed.motifs.join("; ")));
        assert_eq!(r[19].trim(), "press any key");
    }

    #[test]
    fn shadow_presence_follows_a_mirror_exit() {
        let (mut ctx, s, _dir) = card_at(3);
        assert!(!rows(&draw(&s, &ctx))[5].contains("Shadow presence"));
        ctx.run.as_mut().unwrap().exit_taken = Some(ExitDef {
            kind: ExitKind::Mirror,
            text: "the reflection".into(),
        });
        let r = rows(&draw(&s, &ctx));
        assert!(
            r[5].contains("Safe │ Unstable │ Low entity count │ Shadow presence"),
            "{:?}",
            r[5]
        );
    }

    #[test]
    fn the_dev_footer_lists_the_source_of_each_part() {
        let (mut ctx, s, _dir) = card_at(3);
        {
            let level = ctx.run.as_mut().unwrap().levels.get_mut(&3).unwrap();
            level.place_source = Some(GenSource::Ai {
                model: "qwen3.8".into(),
                millis: 41_200,
            });
            level.passage_source = Some(GenSource::Offline {
                reason: "timeout".into(),
            });
            level.interlude_source = Some(GenSource::Authored);
        }
        let r = rows(&draw(&s, &ctx));
        assert_eq!(
            r[21].trim(),
            "[place ai qwen3.8 41.2s] [passage offline: timeout] [interlude authored]"
        );
        ctx.dev = false;
        assert!(rows(&draw(&s, &ctx))[21].trim().is_empty());
    }

    #[test]
    fn shows_decrypting_over_noise_until_the_place_arrives() {
        let (mut ctx, mut s, _dir) = card_at(3);
        ctx.run.as_mut().unwrap().levels.get_mut(&3).unwrap().place = None;
        s.tick(Duration::from_millis(100), &mut ctx);
        let r = rows(&draw(&s, &ctx));
        assert!(r[11].contains("DECRYPTING LEVEL..."), "{:?}", r[11]);
        assert!(!r.iter().any(|l| l.contains("press any key")));
        assert!(r[3].trim().len() > 3, "noise should fill the screen");

        // Keys do nothing yet; the phase is untouched.
        assert!(matches!(s.handle_key(press_char(' '), &mut ctx), Nav::Stay));
        assert_eq!(ctx.run.as_ref().unwrap().phase, Phase::Arrival);
        assert!(ctx.fx.is_empty());

        // Esc still offers to hang up.
        s.handle_key(press(KeyCode::Esc), &mut ctx);
        assert_eq!(ctx.overlay_request, Some(Overlay::ConfirmQuit));
    }

    #[test]
    fn any_key_opens_the_hub_and_the_menu() {
        let (mut ctx, mut s, dir) = card_at(3);
        let nav = s.handle_key(press_char('x'), &mut ctx);
        assert!(matches!(nav, Nav::To(m) if m.name() == "Menu"));
        let run = ctx.run.as_ref().unwrap();
        assert_eq!(
            run.phase,
            Phase::Hub {
                door_done: false,
                choice: None
            }
        );
        assert!(ctx.fx.contains(&FxCommand::Coalesce { ms: 400 }));
        let saved = ctx.store.load("tester").unwrap().expect("autosaved");
        assert_eq!(saved.phase, run.phase);
        drop(dir);
    }

    #[test]
    fn a_resumed_hub_phase_is_kept() {
        let (mut ctx, mut s, _dir) = card_at(3);
        let hub = Phase::Hub {
            door_done: true,
            choice: Some(1),
        };
        ctx.run.as_mut().unwrap().phase = hub;
        let nav = s.handle_key(press(KeyCode::Enter), &mut ctx);
        assert!(matches!(nav, Nav::To(m) if m.name() == "Menu"));
        assert_eq!(ctx.run.as_ref().unwrap().phase, hub);
    }

    #[test]
    fn the_lunar_tint_is_requested_once_for_a_syzygy_level_only() {
        let (mut ctx, mut s, _dir) = card_at(7);
        assert_eq!(
            ctx.level().unwrap().seed.focus.as_deref(),
            Some("syzygy"),
            "depth 7 is authored for SYZYGY"
        );
        s.tick(Duration::from_millis(33), &mut ctx);
        s.tick(Duration::from_millis(33), &mut ctx);
        assert_eq!(tint_requests(&ctx), 1);
        match ctx
            .fx
            .iter()
            .find(|f| matches!(f, FxCommand::HslShiftFg { .. }))
        {
            Some(FxCommand::HslShiftFg { area, hue, .. }) => {
                assert_eq!(*area, Rect::new(0, 0, 80, 24));
                assert_eq!(*hue, 180.0);
            }
            _ => unreachable!(),
        }

        let (mut other, mut s, _dir) = card_at(3);
        s.tick(Duration::from_millis(33), &mut other);
        assert_eq!(tint_requests(&other), 0);
    }

    #[test]
    fn calm_mode_never_tints_and_waits_for_the_place_before_tinting() {
        let (mut ctx, mut s, _dir) = card_at(7);
        ctx.config.display.calm = true;
        s.tick(Duration::from_millis(33), &mut ctx);
        assert_eq!(tint_requests(&ctx), 0);

        let (mut ctx, mut s, _dir) = card_at(7);
        let place = ctx
            .run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&7)
            .unwrap()
            .place
            .take();
        s.tick(Duration::from_millis(33), &mut ctx);
        assert_eq!(tint_requests(&ctx), 0, "still decrypting");
        ctx.run.as_mut().unwrap().levels.get_mut(&7).unwrap().place = place;
        s.tick(Duration::from_millis(33), &mut ctx);
        assert_eq!(tint_requests(&ctx), 1);
    }
}
