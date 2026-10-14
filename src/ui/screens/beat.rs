//! Beat: the payload the objective revealed, typed out beside the focus character's art.
//!
//! Pushed by the Run screen when the objective completes; any key finishes the typing, the next
//! key opens the Choice (which replaces this screen). While the passage has not arrived yet
//! (resume, slow model) a noise vision and `DECRYPTING PAYLOAD...` stand in; the trace stays
//! paused because this screen, not the shell, is the one being ticked.

use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;

use super::{ChoiceScreen, Nav, Screen, ScreenImpl};
use crate::pack::model::{ObjectiveKind, VisionKind};
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{put_centered, put_str};
use crate::ui::widgets::transmission::Transmission;
use crate::visions::Vision;

/// Left edge, width and first row of the beat text.
const TEXT_X: u16 = 2;
const TEXT_W: u16 = 38;
const TEXT_Y: u16 = 3;
const TEXT_ROWS: u16 = 19;
/// Left edge and size of the art box (art is at most 36x16).
const ART_X: u16 = 42;
const ART_W: u16 = 36;
const ART_H: u16 = 16;

/// The Beat screen.
pub struct BeatScreen {
    header: String,
    /// `None` until the passage exists.
    text: Option<Transmission>,
    art: Vec<Line<'static>>,
    vision: Vision,
}

/// The header line for an objective kind.
fn header_for(kind: ObjectiveKind, target_file: &str, daemon: Option<&str>) -> String {
    match kind {
        ObjectiveKind::Retrieve => format!("PAYLOAD: {target_file}"),
        ObjectiveKind::Plant => "PAYLOAD.PIF ACCEPTED".to_string(),
        ObjectiveKind::Crash => {
            format!("{} DUMPED CORE", daemon.unwrap_or("PLEROMA")).to_uppercase()
        }
    }
}

impl BeatScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let (header, seed, depth) = match (ctx.level(), ctx.place(), ctx.run_ref()) {
            (Some(level), place, Some(run)) => (
                header_for(
                    level.seed.objective_kind,
                    place.map_or("", |p| p.host.target_file.as_str()),
                    level.seed.target_daemon.as_deref(),
                ),
                run.seed,
                run.depth,
            ),
            _ => (String::new(), 0, 0),
        };
        let art = ctx
            .focus_character()
            .and_then(|c| c.art.as_deref())
            .and_then(|name| ctx.pack.art(name))
            .map(|a| ctx.palette.pipe_lines(a))
            .unwrap_or_default();
        let vision = Vision::new(VisionKind::Noise, seed, depth, ctx.gradient(), ctx.calm());
        let mut screen = Self {
            header,
            text: None,
            art,
            vision,
        };
        screen.load_text(ctx);
        screen
    }

    /// Start typing the beat if the passage has arrived.
    fn load_text(&mut self, ctx: &Ctx) {
        if self.text.is_none()
            && let Some(p) = ctx.passage()
        {
            self.text = Some(Transmission::new(p.beat.clone(), ctx.baud()));
        }
    }
}

/// Draw pipe-coded `lines` with their top-left at (`x`, `y`): at most `max_w` columns (centre
/// cropped when wider) and `max_h` rows.
pub(super) fn draw_lines(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    lines: &[Line<'static>],
    max_w: u16,
    max_h: u16,
) {
    for (row, line) in lines.iter().take(usize::from(max_h)).enumerate() {
        let width = line.width();
        let skip = width.saturating_sub(usize::from(max_w)) / 2;
        let yy = y + u16::try_from(row).unwrap_or(0);
        let mut col = 0usize;
        for span in &line.spans {
            for c in span.content.chars() {
                if col >= skip && col - skip < usize::from(max_w) {
                    let xx = x + u16::try_from(col - skip).unwrap_or(0);
                    if let Some(cell) = buf.cell_mut((xx, yy)) {
                        cell.set_char(c).set_style(span.style);
                    }
                }
                col += 1;
            }
        }
    }
}

impl ScreenImpl for BeatScreen {
    fn handle_key(&mut self, _key: KeyEvent, ctx: &mut Ctx) -> Nav {
        let Some(text) = &mut self.text else {
            return Nav::Stay;
        };
        if !text.is_done() {
            text.skip();
            return Nav::Stay;
        }
        Nav::To(Box::new(Screen::Choice(ChoiceScreen::new(ctx))))
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        self.load_text(ctx);
        match &mut self.text {
            Some(t) => t.tick(dt),
            None => self.vision.tick(dt),
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        crate::ui::canvas::fill(buf, body, ctx.text_style());
        let Some(text) = &self.text else {
            self.vision.render(body, buf);
            let msg = " DECRYPTING PAYLOAD... ";
            put_centered(buf, body, body.y + body.height / 2, msg, ctx.fg(Dos::White));
            return;
        };
        put_str(
            buf,
            area.x + TEXT_X,
            area.y + 1,
            &self.header,
            area.width,
            ctx.palette.style(Dos::White, Dos::Black).fg(ctx.chrome()),
        );
        put_str(
            buf,
            area.x + TEXT_X,
            area.y + 2,
            &"═".repeat(usize::from(TEXT_W)),
            TEXT_W,
            Style::new().fg(ctx.chrome()),
        );
        text.render(
            buf,
            Rect::new(area.x + TEXT_X, area.y + TEXT_Y, TEXT_W, TEXT_ROWS),
            ctx.text_style(),
        );
        draw_lines(buf, area.x + ART_X, area.y + 2, &self.art, ART_W, ART_H);
        if text.is_done() {
            put_str(
                buf,
                area.x + TEXT_X,
                area.y + 23,
                "press any key",
                TEXT_W,
                ctx.fg(Dos::DarkGray),
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
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::press_char;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rows(screen: &BeatScreen, ctx: &Ctx) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, Rect::new(0, 0, 80, 25), ctx))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, buf.area, y)).collect()
    }

    /// The beat text column (the art lives to its right).
    fn text_col(rows: &[String]) -> Vec<String> {
        rows[3..22]
            .iter()
            .map(|r| {
                r.chars()
                    .skip(2)
                    .take(38)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect()
    }

    fn screen_at(depth: u32) -> (BeatScreen, Ctx, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        let s = BeatScreen::new(&mut ctx);
        (s, ctx, dir)
    }

    fn set_kind(ctx: &mut Ctx, kind: ObjectiveKind, daemon: Option<&str>) {
        let run = ctx.run.as_mut().unwrap();
        let level = run.levels.get_mut(&run.depth).unwrap();
        level.seed.objective_kind = kind;
        level.seed.target_daemon = daemon.map(str::to_string);
    }

    #[test]
    fn header_follows_the_objective_kind() {
        let (_, mut ctx, _d) = screen_at(1);
        let file = ctx.place().unwrap().host.target_file.clone();
        set_kind(&mut ctx, ObjectiveKind::Retrieve, None);
        let s = BeatScreen::new(&mut ctx);
        assert_eq!(rows(&s, &ctx)[1].trim(), format!("PAYLOAD: {file}"));
        set_kind(&mut ctx, ObjectiveKind::Plant, None);
        let s = BeatScreen::new(&mut ctx);
        assert_eq!(rows(&s, &ctx)[1].trim(), "PAYLOAD.PIF ACCEPTED");
        set_kind(&mut ctx, ObjectiveKind::Crash, Some("pleroma"));
        let s = BeatScreen::new(&mut ctx);
        assert_eq!(rows(&s, &ctx)[1].trim(), "PLEROMA DUMPED CORE");
    }

    #[test]
    fn the_beat_types_at_the_baud_rate_and_any_key_completes_it() {
        let (mut s, mut ctx, _d) = screen_at(1);
        let beat = ctx.passage().unwrap().beat.clone();
        let first_word = beat.split_whitespace().next().unwrap().to_string();
        // Baud 0: everything is there after the first tick.
        s.tick(Duration::ZERO, &mut ctx);
        assert!(rows(&s, &ctx).iter().any(|r| r.contains(&first_word)));
        // 9600 baud: nothing before the first tick, a little after 100 ms, all after a key.
        ctx.config.display.baud = 9600;
        let mut s9 = BeatScreen::new(&mut ctx);
        assert!(text_col(&rows(&s9, &ctx)).iter().all(String::is_empty));
        s9.tick(Duration::from_millis(100), &mut ctx);
        let typed: usize = text_col(&rows(&s9, &ctx)).iter().map(String::len).sum();
        assert!(beat.len() > 200, "test needs a long beat");
        assert!(
            (50..=100).contains(&typed),
            "100 ms at 9600 baud is 96 chars: {typed}"
        );
        assert!(matches!(
            s9.handle_key(press_char(' '), &mut ctx),
            Nav::Stay
        ));
        let full = text_col(&rows(&s9, &ctx)).join(" ");
        assert!(full.contains(first_word.as_str()));
        assert!(matches!(
            s9.handle_key(press_char(' '), &mut ctx),
            Nav::To(c) if c.name() == "Choice"
        ));
        assert!(matches!(
            s.handle_key(press_char('x'), &mut ctx),
            Nav::To(c) if c.name() == "Choice"
        ));
    }

    #[test]
    fn it_shows_decrypting_until_the_passage_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 1);
        let passage = {
            let run = ctx.run.as_mut().unwrap();
            run.levels.get_mut(&1).unwrap().passage.take().unwrap()
        };
        let mut s = BeatScreen::new(&mut ctx);
        assert!(
            rows(&s, &ctx)
                .iter()
                .any(|r| r.contains("DECRYPTING PAYLOAD..."))
        );
        assert!(matches!(s.handle_key(press_char(' '), &mut ctx), Nav::Stay));
        assert!(matches!(
            s.tick(Duration::from_millis(50), &mut ctx),
            Nav::Stay
        ));
        assert!(rows(&s, &ctx).iter().any(|r| r.contains("DECRYPTING")));
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&1)
            .unwrap()
            .passage = Some(passage.clone());
        s.tick(Duration::from_millis(50), &mut ctx);
        let r = rows(&s, &ctx);
        assert!(!r.iter().any(|r| r.contains("DECRYPTING")));
        let word = passage.beat.split_whitespace().next().unwrap();
        assert!(r.iter().any(|r| r.contains(word)));
        assert!(matches!(
            s.handle_key(press_char(' '), &mut ctx),
            Nav::To(c) if c.name() == "Choice"
        ));
    }

    #[test]
    fn the_focus_character_art_sits_on_the_right() {
        let (_, mut ctx, _d) = screen_at(1);
        // Depth 1 has a focus character with art (philemon/shadow); give it distinctive art.
        let id = ctx.level().unwrap().seed.focus.clone().unwrap();
        let art_name = ctx.pack.character(&id).unwrap().art.clone().unwrap();
        std::sync::Arc::make_mut(&mut ctx.pack)
            .art
            .insert(art_name, "ZZZZ|12QQQQ".to_string());
        let s = BeatScreen::new(&mut ctx);
        let r = rows(&s, &ctx);
        let line = r.iter().find(|r| r.contains("ZZZZQQQQ")).unwrap();
        assert!(line.find("ZZZZQQQQ").unwrap() >= usize::from(ART_X));
    }
}
