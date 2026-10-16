//! Dialing: the modem dialogue, a burst of line noise, `RING RING` and `CONNECT`. A new game
//! also crosses the line (`CARRIER CROSSED`) and types the first level's interlude.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::dialer::DialTarget;
use super::{DialerScreen, LoginScreen, Nav, Screen, ScreenImpl};
use crate::pack::model::VisionKind;
use crate::palette::Dos;
use crate::text::strip_pipe_codes;
use crate::ui::app::{Ctx, FxCommand};
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::transmission::Transmission;
use crate::visions::Vision;

/// Seconds of line noise.
const NOISE_SECS: f32 = 1.5;
/// Density of the noise vision.
const NOISE_DENSITY: f32 = 0.35;
/// Typing speed of the AT commands (baud); 0 stays instant.
const COMMAND_BAUD: u32 = 300;
/// First canvas row of the modem log.
const LOG_ROW: u16 = 2;
/// Noise covers the rows below the log's first lines.
const NOISE_ROW: u16 = 6;
const CONNECT: &str = "CONNECT 28800/ARQ/V34/LAPM/V42BIS";
const CROSSED: &str = "CARRIER CROSSED";
/// How long the glitch burst on `CARRIER CROSSED` runs.
const GLITCH_MS: u64 = 1500;

enum StepKind {
    /// One line of modem dialogue, typed at `baud`.
    Line {
        text: String,
        colour: Dos,
        baud: u32,
    },
    /// `CARRIER CROSSED`, with a glitch burst.
    Crossed,
    /// Line noise.
    Noise,
    /// The first level's interlude, in a box.
    Interlude(String),
}

struct Step {
    kind: StepKind,
    /// Seconds to hold after the step's transmission finished.
    pause: f32,
}

/// The Dialing screen. Its output is a function of the steps played so far and `dt` sums.
pub struct DialingScreen {
    pub target: DialTarget,
    steps: Vec<Step>,
    idx: usize,
    /// Completed log lines.
    log: Vec<(String, Dos)>,
    tx: Option<Transmission>,
    /// Seconds spent in the current step after its transmission finished (or in the noise).
    wait: f32,
    vision: Vision,
}

impl DialingScreen {
    pub fn new(ctx: &mut Ctx, target: DialTarget) -> Self {
        let number = match target {
            DialTarget::Abraxas => "5550000",
            _ => "5550187",
        };
        let baud = ctx.baud();
        let cmd_baud = if baud == 0 { 0 } else { COMMAND_BAUD };
        let line = |text: &str, colour: Dos, baud: u32, pause: f32| Step {
            kind: StepKind::Line {
                text: text.to_string(),
                colour,
                baud,
            },
            pause,
        };
        let mut steps = vec![
            line("ATZ", Dos::LightGray, cmd_baud, 0.3),
            line("OK", Dos::LightGreen, baud, 0.3),
            line(&format!("ATM1L2DT{number}"), Dos::LightGray, cmd_baud, 0.4),
            Step {
                kind: StepKind::Noise,
                pause: 0.0,
            },
            line("RING", Dos::LightGray, baud, 0.8),
            line("RING", Dos::LightGray, baud, 0.8),
            line(CONNECT, Dos::White, baud, 0.8),
        ];
        if target == DialTarget::NightShift && ctx.run.is_none() {
            let interlude = ctx
                .pack
                .anchor_at(0)
                .and_then(|a| a.anchor.interlude.as_deref())
                .map(|t| strip_pipe_codes(t.trim()))
                .filter(|t| !t.is_empty());
            if let Some(text) = interlude {
                steps.push(Step {
                    kind: StepKind::Crossed,
                    pause: 1.2,
                });
                steps.push(Step {
                    kind: StepKind::Interlude(text),
                    pause: 1.0,
                });
            }
        }
        let mut vision = Vision::new(
            VisionKind::Noise,
            ctx.opts.seed.unwrap_or(0),
            0,
            ctx.gradient(),
            ctx.calm(),
        );
        vision.set_density(NOISE_DENSITY);
        let mut screen = Self {
            target,
            steps,
            idx: 0,
            log: Vec::new(),
            tx: None,
            wait: 0.0,
            vision,
        };
        screen.begin(ctx);
        screen
    }

    /// The lines on screen so far, including the one being typed.
    pub fn log_lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.log.iter().map(|(t, _)| t.clone()).collect();
        if let Some(Step {
            kind: StepKind::Line { .. } | StepKind::Crossed,
            ..
        }) = self.steps.get(self.idx)
            && let Some(tx) = &self.tx
        {
            out.extend(tx.visible_lines(80).into_iter().filter(|l| !l.is_empty()));
        }
        out
    }

    /// True while the line-noise vision is showing.
    pub fn in_noise(&self) -> bool {
        matches!(
            self.steps.get(self.idx),
            Some(Step {
                kind: StepKind::Noise,
                ..
            })
        )
    }

    fn begin(&mut self, ctx: &mut Ctx) {
        self.wait = 0.0;
        self.tx = match self.steps.get(self.idx).map(|s| &s.kind) {
            Some(StepKind::Line { text, baud, .. }) => Some(Transmission::new(text.clone(), *baud)),
            Some(StepKind::Crossed) => {
                ctx.fx.push(FxCommand::Glitch { ms: GLITCH_MS });
                Some(Transmission::new(CROSSED, ctx.baud()))
            }
            Some(StepKind::Interlude(text)) => Some(Transmission::new(text.clone(), ctx.baud())),
            _ => None,
        };
    }

    /// Move on to the next step; the Login screen after the last one.
    fn finish_step(&mut self, ctx: &mut Ctx) -> Nav {
        match self.steps.get(self.idx).map(|s| &s.kind) {
            Some(StepKind::Line { colour, .. }) => {
                if let Some(tx) = self.tx.take() {
                    self.log.push((tx.full_text().to_string(), *colour));
                }
            }
            Some(StepKind::Crossed) => {
                self.log.push((CROSSED.to_string(), Dos::LightRed));
            }
            _ => {}
        }
        self.tx = None;
        self.idx += 1;
        if self.idx >= self.steps.len() {
            return Nav::To(Box::new(Screen::Login(LoginScreen::new(ctx))));
        }
        self.begin(ctx);
        Nav::Stay
    }
}

impl ScreenImpl for DialingScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if classify(&key) == Key::Esc {
            return Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))));
        }
        match &mut self.tx {
            Some(tx) if !tx.is_done() => {
                tx.skip();
                Nav::Stay
            }
            _ => self.finish_step(ctx),
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        let secs = dt.as_secs_f32();
        let Some(step) = self.steps.get(self.idx) else {
            return Nav::Stay;
        };
        let (pause, noise) = (step.pause, matches!(step.kind, StepKind::Noise));
        if noise {
            self.vision.tick(dt);
            self.wait += secs;
            if self.wait >= NOISE_SECS {
                return self.finish_step(ctx);
            }
            return Nav::Stay;
        }
        let done = match &mut self.tx {
            Some(tx) => {
                tx.tick(dt);
                tx.is_done()
            }
            None => true,
        };
        if done {
            self.wait += secs;
            if self.wait >= pause {
                return self.finish_step(ctx);
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let w = area.width;
        fill(buf, area, p.style(Dos::LightGray, Dos::Black));
        let bar = p.style(Dos::White, Dos::Blue);
        fill(buf, Rect::new(area.x, area.y, w, 1), bar);
        let title = match self.target {
            DialTarget::Abraxas => " TERMINUS 3.5 ─ Dialing ???-????",
            _ => " TERMINUS 3.5 ─ Dialing NIGHT SHIFT BBS",
        };
        put_str(buf, area.x, area.y, title, w, bar);

        let mut row = area.y + LOG_ROW;
        for (text, colour) in &self.log {
            put_str(buf, area.x + 1, row, text, w - 2, ctx.fg(*colour));
            row += 1;
        }
        match self.steps.get(self.idx).map(|s| &s.kind) {
            Some(StepKind::Noise) => {
                let rect = Rect::new(
                    area.x,
                    area.y + NOISE_ROW,
                    w,
                    area.height.saturating_sub(NOISE_ROW),
                );
                self.vision.render(rect, buf);
            }
            Some(StepKind::Line { colour, .. }) => {
                if let Some(tx) = &self.tx {
                    let style = ctx.fg(*colour);
                    for line in tx.visible_lines(w - 2) {
                        put_str(buf, area.x + 1, row, &line, w - 2, style);
                        row += 1;
                    }
                }
            }
            Some(StepKind::Crossed) => {
                if let Some(tx) = &self.tx {
                    let style = ctx.fg(Dos::LightRed);
                    for line in tx.visible_lines(w - 2) {
                        put_str(buf, area.x + 1, row, &line, w - 2, style);
                        row += 1;
                    }
                }
            }
            Some(StepKind::Interlude(_)) => {
                let box_area = Rect::new(area.x + 2, area.y + 11, w - 4, 11);
                let inner = boxed(buf, box_area, None, BoxKind::Single, ctx.fg(Dos::DarkGray));
                if let Some(tx) = &self.tx {
                    let text = Rect::new(
                        inner.x + 1,
                        inner.y,
                        inner.width.saturating_sub(2),
                        inner.height,
                    );
                    tx.render(buf, text, ctx.fg(Dos::LightGray));
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_ctx;
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    const DT: Duration = Duration::from_millis(100);

    fn draw(screen: &DialingScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn all_text(buf: &Buffer) -> String {
        (0..25)
            .map(|y| row_text(buf, buf.area, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Ticks until Login takes over; returns the longest log seen and the ticks it took.
    fn play(screen: &mut DialingScreen, ctx: &mut Ctx) -> (Vec<String>, usize) {
        let mut longest = Vec::new();
        for n in 0..400 {
            match screen.tick(DT, ctx) {
                Nav::To(s) => {
                    assert_eq!(s.name(), "Login");
                    return (longest, n + 1);
                }
                Nav::Stay => {}
                _ => panic!("unexpected nav"),
            }
            let lines = screen.log_lines();
            if lines.len() >= longest.len() {
                longest = lines;
            }
        }
        panic!("dialing never finished");
    }

    #[test]
    fn redial_types_the_modem_dialogue_in_order_then_hands_over_to_login() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut ctx, DialTarget::Abraxas);
        let (log, ticks) = play(&mut s, &mut ctx);
        assert_eq!(
            log,
            [
                "ATZ",
                "OK",
                "ATM1L2DT5550000",
                "RING",
                "RING",
                "CONNECT 28800/ARQ/V34/LAPM/V42BIS"
            ]
        );
        assert!(!ctx.fx.iter().any(|f| matches!(f, FxCommand::Glitch { .. })));
        // 0.3+0.3+0.4 s of dialogue, 1.5 s of noise, 0.8*3 s of ring and connect.
        assert!((45..=52).contains(&ticks), "{ticks} ticks");
    }

    #[test]
    fn night_shift_with_an_existing_run_is_a_plain_redial_of_its_own_number() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.pack = std::sync::Arc::new(crate::pack::embedded().unwrap());
        ctx.run = Some(crate::game::state::RunState::new(
            "x", "y", "z", 1, "rollover", None,
        ));
        let mut s = DialingScreen::new(&mut ctx, DialTarget::NightShift);
        let (log, _) = play(&mut s, &mut ctx);
        assert_eq!(log[2], "ATM1L2DT5550187");
        assert!(!log.iter().any(|l| l.contains("CROSSED")));
    }

    #[test]
    fn noise_shows_for_one_and_a_half_seconds_between_dialogue_and_ring() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut ctx, DialTarget::Abraxas);
        let mut noisy = 0.0;
        while !s.in_noise() {
            s.tick(DT, &mut ctx);
        }
        let log_at_noise = s.log_lines();
        assert_eq!(log_at_noise, ["ATZ", "OK", "ATM1L2DT5550000"]);
        while s.in_noise() {
            noisy += DT.as_secs_f32();
            s.tick(DT, &mut ctx);
        }
        assert!((1.4..=1.6).contains(&noisy), "{noisy}");
        let text = all_text(&draw(&s, &ctx));
        assert!(text.contains("ATM1L2DT5550000"));
    }

    #[test]
    fn a_new_game_crosses_the_carrier_and_types_the_first_interlude() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut ctx, DialTarget::NightShift);
        let (log, _) = play(&mut s, &mut ctx);
        assert_eq!(
            &log[5..],
            ["CONNECT 28800/ARQ/V34/LAPM/V42BIS", "CARRIER CROSSED"]
        );
        let glitches = ctx
            .fx
            .iter()
            .filter(|f| matches!(f, FxCommand::Glitch { ms: 1500 }))
            .count();
        assert_eq!(glitches, 1);

        let mut s = DialingScreen::new(&mut ctx, DialTarget::NightShift);
        while !matches!(s.steps[s.idx].kind, StepKind::Interlude(_)) {
            s.tick(DT, &mut ctx);
        }
        s.tick(Duration::from_millis(1), &mut ctx);
        let text = all_text(&draw(&s, &ctx));
        assert!(text.contains("The line crosses."), "{text}");
        assert!(text.contains("CARRIER CROSSED"));
    }

    #[test]
    fn a_key_finishes_the_transmission_first_and_then_advances() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.config.display.baud = 9600;
        let mut s = DialingScreen::new(&mut ctx, DialTarget::Abraxas);
        s.tick(Duration::from_millis(40), &mut ctx);
        assert_eq!(s.log_lines(), ["A"]);
        assert!(matches!(s.handle_key(press_char(' '), &mut ctx), Nav::Stay));
        assert_eq!(s.log_lines(), ["ATZ"]);
        assert!(matches!(s.handle_key(press_char(' '), &mut ctx), Nav::Stay));
        // The second press moved on to `OK`, which has not been typed yet.
        assert_eq!(s.log_lines(), ["ATZ"]);
        s.tick(Duration::from_millis(10), &mut ctx);
        assert_eq!(s.log_lines(), ["ATZ", "OK"]);
    }

    #[test]
    fn a_key_during_the_noise_cuts_it_short() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut ctx, DialTarget::Abraxas);
        while !s.in_noise() {
            s.tick(DT, &mut ctx);
        }
        s.handle_key(press(KeyCode::Enter), &mut ctx);
        assert!(!s.in_noise());
    }

    #[test]
    fn esc_hangs_up_to_the_dialer() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut ctx, DialTarget::Abraxas);
        let nav = s.handle_key(press(KeyCode::Esc), &mut ctx);
        assert!(matches!(nav, Nav::To(d) if d.name() == "Dialer"));
    }

    #[test]
    fn calm_mode_freezes_the_noise_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut calm = test_ctx(dir.path());
        calm.config.display.calm = true;
        let mut s = DialingScreen::new(&mut calm, DialTarget::Abraxas);
        while !s.in_noise() {
            s.tick(DT, &mut calm);
        }
        let first = all_text(&draw(&s, &calm));
        s.tick(Duration::from_millis(400), &mut calm);
        assert!(s.in_noise());
        assert_eq!(first, all_text(&draw(&s, &calm)));
        assert!(
            first.lines().nth(10).unwrap().trim().len() > 5,
            "noise frame is empty"
        );

        let mut live = test_ctx(dir.path());
        let mut s = DialingScreen::new(&mut live, DialTarget::Abraxas);
        while !s.in_noise() {
            s.tick(DT, &mut live);
        }
        let first = all_text(&draw(&s, &live));
        s.tick(Duration::from_millis(400), &mut live);
        assert_ne!(first, all_text(&draw(&s, &live)));
    }
}
