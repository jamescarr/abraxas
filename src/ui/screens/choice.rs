//! Choice: the story prompt and three options. Stances stay hidden; the choice is applied to the
//! run (integration, flags, chronicle, hub phase) and the player returns to the still-running
//! shell, or goes to the Ending at an anchor that ends the run.

use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{EndingScreen, Nav, Screen, ScreenImpl};
use crate::game::state::Phase;
use crate::palette::Dos;
use crate::text::wrap;
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};

const TEXT_X: u16 = 4;
const WIDTH: usize = 70;

/// The Choice screen.
pub struct ChoiceScreen {
    prompt: String,
    options: Vec<String>,
    selected: usize,
}

impl ChoiceScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let (prompt, options) = ctx.passage().map_or_else(Default::default, |p| {
            (
                p.choice.prompt.clone(),
                p.choice.options.iter().map(|o| o.text.clone()).collect(),
            )
        });
        Self {
            prompt,
            options,
            selected: 0,
        }
    }

    /// Apply option `idx` and decide where to go next.
    fn choose(&mut self, idx: usize, ctx: &mut Ctx) -> Nav {
        let ends_run = {
            let pack = ctx.pack.clone();
            let Some(run) = ctx.run.as_mut() else {
                return Nav::Back;
            };
            if !run.apply_choice(&pack, idx) {
                return Nav::Back;
            }
            let ends = pack.ends_run_at(run.depth);
            if ends {
                run.phase = Phase::Ending;
            }
            ends
        };
        ctx.autosave();
        if ends_run {
            Nav::To(Box::new(Screen::Ending(EndingScreen::new(ctx))))
        } else {
            Nav::Back
        }
    }
}

impl ScreenImpl for ChoiceScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        if self.options.is_empty() {
            return Nav::Stay;
        }
        match classify(&key) {
            Key::Char(c @ '1'..='9') => {
                let idx = usize::from(c as u8 - b'1');
                if idx < self.options.len() {
                    return self.choose(idx, ctx);
                }
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(self.options.len() - 1),
            Key::Enter => return self.choose(self.selected, ctx),
            _ => {}
        }
        Nav::Stay
    }

    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) -> Nav {
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        fill(buf, body, ctx.text_style());
        let mut y = area.y + 3;
        for line in wrap(&self.prompt, WIDTH) {
            put_str(
                buf,
                area.x + TEXT_X,
                y,
                &line,
                area.width,
                ctx.fg(Dos::White),
            );
            y += 1;
        }
        y += 1;
        for (i, text) in self.options.iter().enumerate() {
            let on = i == self.selected;
            let style = if on {
                ctx.palette.style(Dos::Black, Dos::LightGray)
            } else {
                ctx.text_style()
            };
            for (n, line) in wrap(text, WIDTH - 4).iter().enumerate() {
                let lead = if n == 0 {
                    format!("[{}] ", i + 1)
                } else {
                    "    ".to_string()
                };
                put_str(
                    buf,
                    area.x + TEXT_X,
                    y,
                    &format!("{lead}{line}"),
                    area.width,
                    style,
                );
                y += 1;
            }
            y += 1;
        }
        put_str(
            buf,
            area.x + TEXT_X,
            area.y + 22,
            "1-3 or arrows + Enter",
            area.width,
            ctx.fg(Dos::DarkGray),
        );
    }

    fn wants_status(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::model::Stance;
    use crate::ui::app::{install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn setup(depth: u32) -> (ChoiceScreen, Ctx, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        let s = ChoiceScreen::new(&mut ctx);
        (s, ctx, dir)
    }

    fn rows(s: &ChoiceScreen, ctx: &Ctx) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| s.render(f, Rect::new(0, 0, 80, 25), ctx))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..25).map(|y| row_text(&buf, buf.area, y)).collect()
    }

    #[test]
    fn shows_the_prompt_and_three_numbered_options_without_stances() {
        let (s, ctx, _d) = setup(3);
        let text = rows(&s, &ctx).join("\n");
        let choice = &ctx.passage().unwrap().choice;
        assert!(text.contains(choice.prompt.split_whitespace().next().unwrap()));
        for (i, o) in choice.options.iter().enumerate() {
            assert!(
                text.contains(&format!("[{}] {}", i + 1, &o.text[..8])),
                "{text}"
            );
        }
        for stance in ["integrate", "repress", "neutral"] {
            assert!(!text.to_lowercase().contains(stance), "{stance} leaked");
        }
    }

    #[test]
    fn each_option_applies_its_stance_flags_chronicle_phase_and_autosaves() {
        for idx in 0..3 {
            let (mut s, mut ctx, _d) = setup(3);
            let opt = ctx.passage().unwrap().choice.options[idx].clone();
            let before = ctx.run.as_ref().unwrap().stats.integration;
            let nav = s.handle_key(press_char(char::from(b'1' + idx as u8)), &mut ctx);
            assert!(matches!(nav, Nav::Back), "depth 3 returns to the shell");
            let run = ctx.run.as_ref().unwrap();
            let want = match opt.stance {
                Stance::Integrate => 1,
                Stance::Repress => -1,
                Stance::Neutral => 0,
            };
            assert_eq!(run.stats.integration, before + want);
            assert!(opt.flags.iter().all(|f| run.flags.contains(f)));
            let e = run.chronicle.entry(3).unwrap();
            assert_eq!(e.choice.as_ref().unwrap().text, opt.text);
            assert_eq!(
                run.phase,
                Phase::Hub {
                    door_done: true,
                    choice: Some(idx)
                }
            );
            let saved = ctx.store.load("Tester").unwrap().unwrap();
            assert_eq!(saved.phase, run.phase, "autosaved");
        }
    }

    #[test]
    fn arrows_and_enter_select_and_digits_beyond_the_options_do_nothing() {
        let (mut s, mut ctx, _d) = setup(3);
        assert!(matches!(s.handle_key(press_char('7'), &mut ctx), Nav::Stay));
        s.handle_key(press(KeyCode::Down), &mut ctx);
        s.handle_key(press(KeyCode::Down), &mut ctx);
        s.handle_key(press(KeyCode::Down), &mut ctx);
        assert_eq!(s.selected, 2);
        s.handle_key(press(KeyCode::Up), &mut ctx);
        assert!(matches!(
            s.handle_key(press(KeyCode::Enter), &mut ctx),
            Nav::Back
        ));
        assert!(matches!(
            ctx.run.as_ref().unwrap().phase,
            Phase::Hub {
                choice: Some(1),
                ..
            }
        ));
    }

    #[test]
    fn the_final_anchor_goes_to_the_ending() {
        let (mut s, mut ctx, _d) = setup(15);
        let nav = s.handle_key(press_char('1'), &mut ctx);
        assert!(matches!(nav, Nav::To(e) if e.name() == "Ending"));
        assert_eq!(ctx.run.as_ref().unwrap().phase, Phase::Ending);
        assert_eq!(
            ctx.store.load("Tester").unwrap().unwrap().phase,
            Phase::Ending
        );
    }

    #[test]
    fn without_endless_the_last_depth_ends_the_run_even_with_no_authored_ending() {
        for (depth, ends) in [(15, true), (14, false)] {
            let dir = tempfile::tempdir().unwrap();
            let mut ctx = test_ctx(dir.path());
            let pack = std::sync::Arc::make_mut(&mut ctx.pack);
            pack.meta.endless = None;
            for a in &mut pack.anchors {
                a.anchor.ending.clear();
            }
            install_test_run(&mut ctx, depth);
            let mut s = ChoiceScreen::new(&mut ctx);
            let nav = s.handle_key(press_char('2'), &mut ctx);
            assert_eq!(
                matches!(nav, Nav::To(e) if e.name() == "Ending"),
                ends,
                "{depth}"
            );
            assert_eq!(ctx.run.as_ref().unwrap().phase == Phase::Ending, ends);
        }
    }
}
