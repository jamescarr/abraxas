//! Death: ego death. Fire for two seconds, then `EGO DEATH` and the obituary. The save is
//! deleted and the caller recorded as lost the moment the screen is built.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{DialerScreen, Nav, Screen, ScreenImpl};
use crate::pack::model::VisionKind;
use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_centered};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::transmission::Transmission;
use crate::visions::Vision;

/// Seconds of bare fire before the text (calm mode: a short beat on the frozen frame).
const FIRE_SECS: f32 = 2.0;
const FIRE_SECS_CALM: f32 = 0.5;
const BOX: Rect = Rect::new(9, 7, 62, 11);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Fire,
    Text,
}

/// The Death screen.
pub struct DeathScreen {
    t: f32,
    fire_secs: f32,
    stage: Stage,
    vision: Vision,
    text: Transmission,
}

impl DeathScreen {
    /// Snapshot what the obituary needs, delete the save and record the caller as lost. The
    /// run is dropped from the context here so nothing (not even a quit autosave) can bring
    /// the dead save back.
    pub fn new(ctx: &mut Ctx) -> Self {
        let calm = ctx.calm();
        let clock = ctx.clock().map(|c| c.hms()).unwrap_or_default();
        let title = ctx
            .place()
            .map_or_else(|| "UNKNOWN".to_string(), |p| p.title.clone());
        let (handle, depth, seed) = ctx.run_ref().map_or((String::from("CALLER"), 0, 0), |r| {
            (r.handle.clone(), r.depth, r.seed)
        });
        let vision = Vision::new(VisionKind::Fire, seed, depth, ctx.gradient(), calm);
        if let Some(run) = ctx.run.take()
            && let Err(e) = ctx.store.ego_death(&run, &clock)
        {
            tracing::warn!("recording the ego death failed: {e:#}");
        }
        let text = format!(
            "EGO DEATH\n\n{handle} was lost at LEVEL {depth} ({title}) at {clock}.\n\nNO CARRIER"
        );
        Self {
            t: 0.0,
            fire_secs: if calm { FIRE_SECS_CALM } else { FIRE_SECS },
            stage: Stage::Fire,
            vision,
            text: Transmission::new(text, ctx.baud()),
        }
    }

    /// Whether the obituary is showing (the fire is over).
    pub fn text_shown(&self) -> bool {
        self.stage == Stage::Text
    }

    fn leave(ctx: &mut Ctx) -> Nav {
        ctx.run = None;
        Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))))
    }
}

impl ScreenImpl for DeathScreen {
    fn handle_key(&mut self, _key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match self.stage {
            Stage::Fire => {
                self.stage = Stage::Text;
                Nav::Stay
            }
            Stage::Text if !self.text.is_done() => {
                self.text.skip();
                Nav::Stay
            }
            Stage::Text => Self::leave(ctx),
        }
    }

    fn tick(&mut self, dt: Duration, _ctx: &mut Ctx) -> Nav {
        self.t += dt.as_secs_f32();
        self.vision.tick(dt);
        if self.stage == Stage::Fire && self.t >= self.fire_secs {
            self.stage = Stage::Text;
        }
        if self.stage == Stage::Text {
            self.text.tick(dt);
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        fill(buf, area, ctx.text_style());
        self.vision.render(area, buf);
        if self.stage == Stage::Text {
            let black = p.color(Dos::Black);
            let frame_style = Style::new().fg(p.color(Dos::Red)).bg(black);
            let outer = Rect::new(area.x + BOX.x, area.y + BOX.y, BOX.width, BOX.height);
            let inner = boxed(buf, outer, None, BoxKind::Double, frame_style);
            for (i, line) in self.text.visible_lines(inner.width - 4).iter().enumerate() {
                let style = match line.as_str() {
                    "EGO DEATH" => p.style(Dos::LightRed, Dos::Black),
                    "NO CARRIER" => p.style(Dos::DarkGray, Dos::Black),
                    _ => ctx.text_style(),
                };
                put_centered(buf, inner, inner.y + 1 + i as u16, line, style);
            }
            if self.text.is_done() {
                put_centered(
                    buf,
                    inner,
                    inner.bottom() - 1,
                    "press any key",
                    p.style(Dos::DarkGray, Dos::Black),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::save::CallerStatus;
    use crate::ui::app::{App, install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::press_char;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::Event;

    const DT: Duration = Duration::from_millis(100);

    /// A dead run at depth 3 whose save exists on disk.
    fn dead(dir: &std::path::Path) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, 3);
        ctx.run.as_mut().unwrap().stats.ego = 0;
        ctx.autosave();
        assert!(ctx.store.exists("Tester"));
        ctx
    }

    fn rows(s: &DeathScreen, ctx: &Ctx) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 25);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| s.render(f, area, ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, area, y)).collect()
    }

    #[test]
    fn building_the_screen_deletes_the_save_and_records_the_caller_as_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = dead(dir.path());
        let _s = DeathScreen::new(&mut ctx);
        assert!(!ctx.store.exists("Tester"));
        let rec = &ctx.store.callers()[0];
        assert_eq!(
            (rec.handle.as_str(), rec.depth, rec.status),
            ("Tester", 3, CallerStatus::Lost)
        );
        assert!(ctx.store.is_dead("Tester"));
        assert!(ctx.run.is_none(), "nothing can autosave the dead run back");
    }

    #[test]
    fn fire_for_two_seconds_then_the_obituary_then_the_dialer() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = dead(dir.path());
        let title = ctx.place().unwrap().title.clone();
        let clock = ctx.clock().unwrap().hms();
        let mut s = DeathScreen::new(&mut ctx);
        for _ in 0..19 {
            s.tick(DT, &mut ctx);
        }
        let fire = rows(&s, &ctx).join("\n");
        assert!(!s.text_shown() && !fire.contains("EGO DEATH"));
        assert!(
            fire.contains('#') || fire.contains('@') || fire.contains('%'),
            "flames:\n{fire}"
        );
        s.tick(DT, &mut ctx);
        s.tick(DT, &mut ctx);
        assert!(s.text_shown());
        let r = rows(&s, &ctx);
        let text = r.join("\n");
        assert!(
            text.contains("EGO DEATH") && text.contains("NO CARRIER"),
            "{text}"
        );
        let story = format!("Tester was lost at LEVEL 3 ({title}) at {clock}.");
        let squashed: String = r
            .iter()
            .map(|l| l.trim_matches(|c| "║ ".contains(c)))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(squashed.contains(&story), "{squashed}");
        assert!(
            matches!(s.handle_key(press_char('x'), &mut ctx), Nav::To(n) if n.name() == "Dialer")
        );
        assert!(ctx.run.is_none());
    }

    #[test]
    fn at_a_real_baud_rate_a_key_completes_the_text_and_the_next_key_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = dead(dir.path());
        ctx.config.display.baud = 300;
        let mut s = DeathScreen::new(&mut ctx);
        assert!(matches!(s.handle_key(press_char('x'), &mut ctx), Nav::Stay));
        assert!(s.text_shown(), "a key ends the fire early");
        s.tick(DT, &mut ctx);
        assert!(
            !rows(&s, &ctx).join("\n").contains("NO CARRIER"),
            "still typing"
        );
        assert!(matches!(s.handle_key(press_char('x'), &mut ctx), Nav::Stay));
        s.tick(DT, &mut ctx);
        assert!(rows(&s, &ctx).join("\n").contains("NO CARRIER"));
        assert!(matches!(
            s.handle_key(press_char('x'), &mut ctx),
            Nav::To(_)
        ));
    }

    #[test]
    fn calm_mode_shows_a_frozen_flame_and_the_text_follows_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = dead(dir.path());
        ctx.config.display.calm = true;
        let mut s = DeathScreen::new(&mut ctx);
        let a = rows(&s, &ctx);
        s.tick(DT, &mut ctx);
        assert_eq!(a, rows(&s, &ctx));
        for _ in 0..5 {
            s.tick(DT, &mut ctx);
        }
        assert!(s.text_shown());
    }

    #[test]
    fn ego_zero_on_any_screen_hands_over_to_death() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 5);
        ctx.autosave();
        let screen = Screen::Descent(super::super::DescentScreen::new(&mut ctx));
        let mut app = App::with_screen(ctx, screen);
        app.ctx.run.as_mut().unwrap().stats.ego = 0;
        app.step(None, DT);
        assert_eq!(app.screen.name(), "Death");
        assert!(!app.ctx.store.exists("Tester"));
        for _ in 0..30 {
            app.step(None, DT);
        }
        app.step(Some(Event::Key(press_char('x'))), DT);
        app.step(Some(Event::Key(press_char('x'))), DT);
        assert_eq!(app.screen.name(), "Dialer");
        assert!(app.ctx.run.is_none());
    }
}
