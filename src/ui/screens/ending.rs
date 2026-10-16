//! Ending: the epilogue streams over the ending's vision. When the pack has an endless tail the
//! player may continue into it; otherwise (or on `N`) the run is complete and the caller
//! ascends.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{DialerScreen, ExitsScreen, Nav, Screen, ScreenImpl};
use crate::director::offline;
use crate::game::state::Phase;
use crate::pack::model::{EndingDef, Stance, VisionKind};
use crate::palette::Dos;
use crate::ui::app::{Ctx, EpilogueBuf, FxCommand};
use crate::ui::canvas::{fill, put_centered};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::transmission::Transmission;
use crate::visions::Vision;

/// Id, title and text of the ending a pack gets when no authored ending matches.
pub const GENERIC_ID: &str = "end_of_line";
pub const GENERIC_TITLE: &str = "END OF LINE";
pub const GENERIC_TEXT: &str = "The board goes quiet. NO CARRIER.";

const BOX: Rect = Rect::new(6, 1, 68, 21);
const PROMPT_ROW: u16 = 23;
const DISSOLVE_MS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The epilogue is arriving and being typed.
    Epilogue,
    /// `Continue into THE UNDERBOARD? [Y/n]`.
    Continue,
    /// Nothing to ask; any key ends the run.
    Farewell,
}

/// The Ending screen.
pub struct EndingScreen {
    ending: EndingDef,
    generic: bool,
    stage: Stage,
    t: f32,
    timeout: f32,
    vision: Vision,
    text: Transmission,
    /// Bytes of `ctx.epilogue.text` already pushed into `text`.
    pushed: usize,
    text_final: bool,
    /// The option index chosen at this depth, restored on continue.
    choice: Option<usize>,
    endless_name: Option<String>,
}

/// The generic ending.
pub fn generic_ending() -> EndingDef {
    EndingDef {
        id: GENERIC_ID.into(),
        title: GENERIC_TITLE.into(),
        when_stance: Stance::Neutral,
        min_integration: None,
        max_integration: None,
        vision: Some(VisionKind::Noise),
        text: GENERIC_TEXT.into(),
    }
}

impl EndingScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let calm = ctx.calm();
        let (ending, generic, choice, seed, depth) = match ctx.run_ref() {
            Some(run) => {
                let chosen = run
                    .chronicle
                    .entry(run.depth)
                    .and_then(|e| e.choice.as_ref());
                let ending = chosen
                    .and_then(|c| {
                        ctx.pack
                            .ending_for(run.depth, c.stance, run.stats.integration)
                    })
                    .cloned();
                let choice = chosen.and_then(|c| {
                    ctx.passage()?
                        .choice
                        .options
                        .iter()
                        .position(|o| o.text == c.text)
                });
                let generic = ending.is_none();
                (
                    ending.unwrap_or_else(generic_ending),
                    generic,
                    choice,
                    run.seed,
                    run.depth,
                )
            }
            None => (generic_ending(), true, None, 0, 0),
        };
        if let Some(run) = ctx.run_mut() {
            run.ending = Some(ending.id.clone());
            run.phase = Phase::Ending;
        }
        ctx.autosave();
        ctx.epilogue = EpilogueBuf::default();
        if let Some(run) = ctx.run.as_ref() {
            ctx.director.request_epilogue(run, &ending);
        }
        let mut vision = Vision::new(ending.vision(), seed, depth, ctx.gradient(), calm);
        if ending.vision() == VisionKind::Mirror {
            vision.set_snapshot(ctx.last_frame.clone());
        }
        if generic {
            vision.set_density(0.15);
        }
        if !calm {
            ctx.fx.push(FxCommand::Dissolve { ms: DISSOLVE_MS });
        }
        let endless_name = (ctx.pack.endless_enabled() && !generic)
            .then(|| ctx.pack.meta.endless.as_ref().map(|e| e.name.clone()))
            .flatten();
        Self {
            ending,
            generic,
            stage: Stage::Epilogue,
            t: 0.0,
            timeout: ctx.config.ai.timeout_secs.max(1) as f32,
            vision,
            text: Transmission::streaming(ctx.baud()),
            pushed: 0,
            text_final: false,
            choice,
            endless_name,
        }
    }

    /// The ending being played.
    pub fn ending(&self) -> &EndingDef {
        &self.ending
    }

    pub fn is_generic(&self) -> bool {
        self.generic
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Feed the typing area from `ctx.epilogue`; fall back to the authored text when the
    /// director has not answered in `ai.timeout_secs`.
    fn sync_epilogue(&mut self, ctx: &Ctx) {
        if self.text_final {
            return;
        }
        let buf = &ctx.epilogue;
        if buf.done && !buf.text.trim().is_empty() {
            self.text.set_final(&buf.text);
            self.text_final = true;
        } else if !buf.done
            && let Some(fresh) = buf.text.get(self.pushed..)
            && !fresh.is_empty()
        {
            self.text.push_str(fresh);
            self.pushed = buf.text.len();
        }
        if !self.text_final && (buf.done || self.t >= self.timeout) {
            let local = match ctx.run_ref() {
                Some(run) => offline::epilogue(&self.ending, run),
                None => self.ending.text.clone(),
            };
            self.text.set_final(&local);
            self.text_final = true;
        }
    }

    /// Continue into the endless tail through the exits of this depth's passage.
    fn continue_on(&self, ctx: &mut Ctx) -> Nav {
        let Some(run) = ctx.run_mut() else {
            return Nav::Stay;
        };
        run.flags.insert(format!("ending_{}", self.ending.id));
        run.phase = Phase::Hub {
            door_done: true,
            choice: self.choice,
        };
        ctx.autosave();
        Nav::To(Box::new(Screen::Exits(ExitsScreen::new(ctx))))
    }

    /// Mark the run complete, record the caller as ascended (deleting the save) and hang up.
    fn complete(ctx: &mut Ctx) -> Nav {
        let clock = ctx.clock().map(|c| c.hms()).unwrap_or_default();
        if let Some(mut run) = ctx.run.take() {
            run.completed = true;
            if let Err(e) = ctx.store.ascend(&run, &clock) {
                tracing::warn!("recording the ascension failed: {e:#}");
            }
        }
        Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))))
    }
}

impl ScreenImpl for EndingScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match self.stage {
            Stage::Epilogue => {
                self.text.skip();
                Nav::Stay
            }
            Stage::Continue => match classify(&key) {
                Key::Enter | Key::Char('y' | 'Y') => self.continue_on(ctx),
                Key::Esc | Key::Char('n' | 'N') => Self::complete(ctx),
                _ => Nav::Stay,
            },
            Stage::Farewell => Self::complete(ctx),
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        self.t += dt.as_secs_f32();
        self.vision.tick(dt);
        self.sync_epilogue(ctx);
        self.text.tick(dt);
        if self.stage == Stage::Epilogue && self.text.is_done() {
            self.stage = if self.endless_name.is_some() {
                Stage::Continue
            } else {
                Stage::Farewell
            };
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let p = &ctx.palette;
        let black = p.color(Dos::Black);
        fill(buf, area, ctx.text_style());
        self.vision.render(area, buf);

        let chrome = Style::new().fg(ctx.chrome()).bg(black);
        let outer = Rect::new(area.x + BOX.x, area.y + BOX.y, BOX.width, BOX.height);
        let inner = boxed(
            buf,
            outer,
            Some(&self.ending.title),
            BoxKind::Double,
            chrome,
        );
        let text_area = Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            inner.height.saturating_sub(2),
        );
        self.text.render(buf, text_area, ctx.text_style());

        let prompt = match (self.stage, &self.endless_name) {
            (Stage::Continue, Some(name)) => Some(format!(" Continue into {name}? [Y/n] ")),
            (Stage::Farewell, _) => Some(" press any key ".to_string()),
            _ => None,
        };
        if let Some(prompt) = prompt {
            put_centered(
                buf,
                area,
                area.y + PROMPT_ROW,
                &prompt,
                p.style(Dos::White, Dos::Black),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::save::CallerStatus;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use std::sync::Arc;

    const DT: Duration = Duration::from_millis(100);

    /// A run at the Center (depth 15) that chose the option of `stance` with `integration`.
    fn at_the_center(dir: &std::path::Path, stance: Stance, integration: i32) -> Ctx {
        let mut ctx = test_ctx(dir);
        install_test_run(&mut ctx, 15);
        let pack = ctx.pack.clone();
        let run = ctx.run.as_mut().unwrap();
        let idx = run.levels[&15]
            .passage
            .as_ref()
            .unwrap()
            .choice
            .options
            .iter()
            .position(|o| o.stance == stance)
            .unwrap();
        assert!(run.apply_choice(&pack, idx));
        run.stats.integration = integration;
        run.phase = Phase::Ending;
        ctx
    }

    fn rows(s: &EndingScreen, ctx: &Ctx) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 25);
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| s.render(f, area, ctx)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, area, y)).collect()
    }

    /// Deliver the epilogue the way the app does and let the typing finish.
    fn finish_epilogue(s: &mut EndingScreen, ctx: &mut Ctx, text: &str) {
        ctx.epilogue.text = text.into();
        ctx.epilogue.done = true;
        s.tick(DT, ctx);
        s.tick(DT, ctx);
    }

    #[test]
    fn the_stance_and_integration_pick_the_ending() {
        let cases = [
            (Stance::Integrate, 5, "individuation", VisionKind::Mandala),
            (Stance::Integrate, 1, "cold_hand", VisionKind::Mirror),
            (Stance::Repress, -2, "the_cut", VisionKind::Noise),
            (Stance::Neutral, 0, "abraxas", VisionKind::Ouroboros),
        ];
        for (stance, integration, id, vision) in cases {
            let dir = tempfile::tempdir().unwrap();
            let mut ctx = at_the_center(dir.path(), stance, integration);
            let s = EndingScreen::new(&mut ctx);
            assert_eq!(s.ending().id, id, "{stance:?} {integration}");
            assert_eq!(s.vision.kind(), vision);
            assert!(!s.is_generic());
            let run = ctx.run.as_ref().unwrap();
            assert_eq!(run.ending.as_deref(), Some(id));
            assert_eq!(run.phase, Phase::Ending);
            assert_eq!(
                ctx.store.load("Tester").unwrap().unwrap().ending.as_deref(),
                Some(id)
            );
        }
    }

    #[test]
    fn a_pack_without_a_matching_ending_gets_end_of_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Integrate, 5);
        let mut pack = (*ctx.pack).clone();
        pack.meta.endless.as_mut().unwrap().enabled = false;
        for a in &mut pack.anchors {
            a.anchor.ending.clear();
        }
        ctx.pack = Arc::new(pack);
        let mut s = EndingScreen::new(&mut ctx);
        assert!(s.is_generic());
        assert_eq!(s.ending().title, "END OF LINE");
        assert_eq!(
            ctx.run.as_ref().unwrap().ending.as_deref(),
            Some("end_of_line")
        );
        // No director answer: the local text appears after the timeout, with no prompt after it.
        ctx.config.ai.timeout_secs = 2;
        s.timeout = 2.0;
        for _ in 0..25 {
            s.tick(DT, &mut ctx);
        }
        let text = rows(&s, &ctx).join("\n");
        assert!(text.contains("The board goes quiet. NO CARRIER."), "{text}");
        assert!(text.contains("press any key") && !text.contains("Continue"));
        assert_eq!(s.stage(), Stage::Farewell);
    }

    #[test]
    fn the_epilogue_streams_in_as_deltas_arrive_then_the_final_text_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Integrate, 5);
        ctx.config.display.baud = 0;
        let mut s = EndingScreen::new(&mut ctx);
        ctx.epilogue.text = "The phone ".into();
        s.tick(DT, &mut ctx);
        assert!(rows(&s, &ctx)[3].contains("The phone"));
        assert_eq!(s.stage(), Stage::Epilogue);
        ctx.epilogue.text.push_str("rings.");
        s.tick(DT, &mut ctx);
        assert!(rows(&s, &ctx)[3].contains("The phone rings."));
        assert_eq!(
            s.stage(),
            Stage::Epilogue,
            "not finished until the final text"
        );
        ctx.epilogue.text = "The phone rings. It is Wren.".into();
        ctx.epilogue.done = true;
        s.tick(DT, &mut ctx);
        s.tick(DT, &mut ctx);
        assert!(rows(&s, &ctx)[3].contains("It is Wren."));
        assert_eq!(s.stage(), Stage::Continue);
    }

    #[test]
    fn without_an_answer_the_authored_epilogue_appears_after_the_ai_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Repress, -3);
        ctx.config.ai.timeout_secs = 3;
        let mut s = EndingScreen::new(&mut ctx);
        for _ in 0..25 {
            s.tick(DT, &mut ctx);
        }
        assert_eq!(
            s.stage(),
            Stage::Epilogue,
            "still waiting before the timeout"
        );
        for _ in 0..10 {
            s.tick(DT, &mut ctx);
        }
        let expect = offline::epilogue(s.ending(), ctx.run.as_ref().unwrap());
        assert_eq!(s.text.full_text(), expect);
        assert_eq!(s.stage(), Stage::Continue);
    }

    #[test]
    fn the_continue_prompt_names_the_underboard() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Neutral, 0);
        let mut s = EndingScreen::new(&mut ctx);
        finish_epilogue(&mut s, &mut ctx, "You become the sysop.");
        assert!(rows(&s, &ctx)[23].contains("Continue into THE UNDERBOARD? [Y/n]"));
        assert!(matches!(s.handle_key(press_char('q'), &mut ctx), Nav::Stay));
    }

    #[test]
    fn yes_sets_the_ending_flag_reopens_the_hub_and_offers_the_exits() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Integrate, 5);
        let mut s = EndingScreen::new(&mut ctx);
        finish_epilogue(&mut s, &mut ctx, "Wren calls.");
        let Nav::To(next) = s.handle_key(press(KeyCode::Enter), &mut ctx) else {
            panic!("expected a transition");
        };
        let Screen::Exits(exits) = *next else {
            panic!("expected the Exits screen");
        };
        assert_eq!(exits.exits().len(), 3, "the exits of the Center's passage");
        let run = ctx.run.as_ref().unwrap();
        assert!(run.has_flag("ending_individuation"));
        let chosen = run
            .chronicle
            .entry(15)
            .unwrap()
            .choice
            .as_ref()
            .unwrap()
            .text
            .clone();
        let idx = run.levels[&15]
            .passage
            .as_ref()
            .unwrap()
            .choice
            .options
            .iter()
            .position(|o| o.text == chosen);
        assert_eq!(
            run.phase,
            Phase::Hub {
                door_done: true,
                choice: idx
            }
        );
        assert!(!run.completed);
        assert!(ctx.store.exists("Tester"), "the run goes on");
    }

    #[test]
    fn no_ascends_deletes_the_save_and_hangs_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Repress, -2);
        let mut s = EndingScreen::new(&mut ctx);
        finish_epilogue(&mut s, &mut ctx, "The clocks read 19100.");
        assert!(
            matches!(s.handle_key(press_char('n'), &mut ctx), Nav::To(n) if n.name() == "Dialer")
        );
        assert!(ctx.run.is_none());
        assert!(!ctx.store.exists("Tester"));
        let rec = &ctx.store.callers()[0];
        assert_eq!(
            (rec.handle.as_str(), rec.depth, rec.status),
            ("Tester", 15, CallerStatus::Ascended)
        );
    }

    #[test]
    fn a_key_while_the_epilogue_types_only_completes_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = at_the_center(dir.path(), Stance::Integrate, 5);
        ctx.config.display.baud = 300;
        let mut s = EndingScreen::new(&mut ctx);
        ctx.epilogue.text = "x".repeat(200);
        ctx.epilogue.done = true;
        s.tick(DT, &mut ctx);
        assert_eq!(s.stage(), Stage::Epilogue);
        assert!(matches!(s.handle_key(press_char('y'), &mut ctx), Nav::Stay));
        s.tick(DT, &mut ctx);
        assert_eq!(
            s.stage(),
            Stage::Continue,
            "the key did not answer the prompt"
        );
        assert!(!ctx.run.as_ref().unwrap().has_flag("ending_individuation"));
    }
}
