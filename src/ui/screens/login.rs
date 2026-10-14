//! Login: the ABRAXAS logo, the last callers, and the `Handle:` prompt. Registers a new caller
//! or resumes a saved run.

use std::sync::Arc;
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{DescentScreen, DialerScreen, EndingScreen, LevelCardScreen, Nav, Screen, ScreenImpl};
use crate::game::save::{CallerRecord, CallerStatus};
use crate::game::seed::ensure_level;
use crate::game::state::{Phase, RunState};
use crate::pack;
use crate::palette::Dos;
use crate::text::{pipe_size, valid_handle};
use crate::ui::app::Ctx;
use crate::ui::canvas::{fill, put_str};
use crate::ui::keys::{Key, classify};
use crate::ui::widgets::banner::{BoxKind, boxed};
use crate::ui::widgets::input::{InputEvent, TextInput};

const LOGO: &str = "abraxas-logo.txt";
const MAX_HANDLE: usize = 16;
const MAX_REAL_NAME: usize = 40;
const MAX_HEARD: usize = 60;
/// Seconds `Welcome back.` stays up before the run resumes (any key skips).
const WELCOME_SECS: f32 = 1.2;
const CALLERS_SHOWN: usize = 5;
const LOGO_ROW: u16 = 0;
const CALLERS_ROW: u16 = 9;
const PROMPT_ROW: u16 = 17;

const HANDLE_PROMPT: &str = "Handle: ";

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Handle,
    /// `Register as X? [Y/n]`.
    Confirm(String),
    RealName(String),
    Heard {
        handle: String,
        real_name: String,
    },
    /// A saved run was found and loaded; `Welcome back.` is showing.
    Welcome(f32),
}

/// The Login screen.
pub struct LoginScreen {
    step: Step,
    input: TextInput,
    /// Answered prompts, drawn above the current one.
    transcript: Vec<String>,
    message: Option<(String, Dos)>,
    logo: Vec<Line<'static>>,
    logo_x: u16,
    callers: Vec<CallerRecord>,
}

impl LoginScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let (logo, logo_x) = match ctx.pack.art(LOGO) {
            Some(text) => {
                let (cols, _) = pipe_size(text);
                let x = 80usize.saturating_sub(cols) / 2;
                (ctx.palette.pipe_lines(text), u16::try_from(x).unwrap_or(0))
            }
            None => (Vec::new(), 0),
        };
        Self {
            step: Step::Handle,
            input: TextInput::new(HANDLE_PROMPT, MAX_HANDLE),
            transcript: Vec::new(),
            message: None,
            logo,
            logo_x,
            callers: ctx.store.last_callers(CALLERS_SHOWN),
        }
    }

    /// The message line, if any.
    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(m, _)| m.as_str())
    }

    fn say(&mut self, text: impl Into<String>, colour: Dos) {
        self.message = Some((text.into(), colour));
    }

    fn reset_to_handle(&mut self) {
        self.step = Step::Handle;
        self.transcript.clear();
        self.input = TextInput::new(HANDLE_PROMPT, MAX_HANDLE);
    }

    fn submit_handle(&mut self, handle: &str, ctx: &mut Ctx) {
        let chars = handle.chars().count();
        if !(2..=MAX_HANDLE).contains(&chars) {
            self.say("A handle is 2 to 16 characters.", Dos::LightRed);
            return;
        }
        if !valid_handle(handle) {
            self.say("Use letters, digits, - and _ only.", Dos::LightRed);
            return;
        }
        if ctx
            .pack
            .cast
            .iter()
            .any(|c| c.handle.eq_ignore_ascii_case(handle))
        {
            self.say("That handle is taken.", Dos::LightRed);
            return;
        }
        if ctx.store.is_dead(handle) {
            self.say("That handle belongs to the dead.", Dos::LightRed);
            return;
        }
        match ctx.store.load(handle) {
            Ok(Some(run)) => self.resume(run, ctx),
            Ok(None) => {
                self.message = None;
                self.transcript.push(format!("{HANDLE_PROMPT}{handle}"));
                self.step = Step::Confirm(handle.to_string());
            }
            Err(e) => {
                tracing::warn!("loading save for {handle}: {e:#}");
                self.say(format!("Your save could not be read: {e:#}"), Dos::LightRed);
            }
        }
    }

    /// Adopt a loaded run: switch to the pack it was started with when that differs, then show
    /// `Welcome back.`.
    fn resume(&mut self, run: RunState, ctx: &mut Ctx) {
        if run.pack_id != ctx.pack.meta.id {
            match pack::open_for_run(run.pack_dir.as_deref()) {
                Ok(p) if p.meta.id == run.pack_id => ctx.pack = Arc::new(p),
                Ok(p) => {
                    self.say(
                        format!(
                            "This run belongs to story pack '{}', not '{}'.",
                            run.pack_id, p.meta.id
                        ),
                        Dos::LightRed,
                    );
                    return;
                }
                Err(e) => {
                    let first = e.0.first().map_or_else(String::new, ToString::to_string);
                    self.say(
                        format!("Story pack '{}' cannot be opened: {first}", run.pack_id),
                        Dos::LightRed,
                    );
                    return;
                }
            }
        }
        self.transcript
            .push(format!("{HANDLE_PROMPT}{}", run.handle));
        ctx.run = Some(run);
        ctx.record_caller(CallerStatus::Online);
        self.say("Welcome back.", Dos::LightGreen);
        self.step = Step::Welcome(0.0);
    }

    fn finish_welcome(ctx: &mut Ctx) -> Nav {
        match ctx.run.as_ref().map(|r| r.phase) {
            Some(Phase::Descending { .. }) => {
                Nav::To(Box::new(Screen::Descent(DescentScreen::new(ctx))))
            }
            Some(Phase::Ending) => Nav::To(Box::new(Screen::Ending(EndingScreen::new(ctx)))),
            _ => Nav::To(Box::new(Screen::LevelCard(LevelCardScreen::new(ctx)))),
        }
    }

    fn register(&mut self, handle: &str, real_name: &str, heard_from: &str, ctx: &mut Ctx) -> Nav {
        let seed = ctx.opts.seed.unwrap_or_else(rand::random);
        let mut run = RunState::new(
            handle,
            real_name,
            heard_from,
            seed,
            ctx.pack.meta.id.clone(),
            ctx.opts.pack.clone(),
        );
        ensure_level(&mut run, &ctx.pack, 0);
        ctx.run = Some(run);
        ctx.record_caller(CallerStatus::Online);
        ctx.autosave();
        Nav::To(Box::new(Screen::LevelCard(LevelCardScreen::new(ctx))))
    }

    fn handle_confirm(&mut self, key: &KeyEvent, handle: &str) {
        match classify(key) {
            Key::Char('y' | 'Y') | Key::Enter => {
                self.transcript
                    .push(format!("Register as {handle}? [Y/n] Y"));
                self.input =
                    TextInput::new("Real name (it will be used against you): ", MAX_REAL_NAME);
                self.step = Step::RealName(handle.to_string());
            }
            Key::Char('n' | 'N') | Key::Esc => self.reset_to_handle(),
            _ => {}
        }
    }

    fn draw_logo(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        for (i, line) in self.logo.iter().enumerate() {
            let y = area.y + LOGO_ROW + u16::try_from(i).unwrap_or(0);
            if y >= area.y + CALLERS_ROW - 1 {
                break;
            }
            let mut x = area.x + self.logo_x;
            for span in &line.spans {
                let used = put_str(
                    buf,
                    x,
                    y,
                    &span.content,
                    area.right().saturating_sub(x),
                    span.style,
                );
                x += used;
            }
        }
    }

    fn draw_callers(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, ctx: &Ctx) {
        let boxed_area = Rect::new(area.x + 10, area.y + CALLERS_ROW, 60, 7);
        let inner = boxed(
            buf,
            boxed_area,
            Some("LAST CALLERS"),
            BoxKind::Single,
            ctx.fg(Dos::DarkGray),
        );
        if self.callers.is_empty() {
            put_str(
                buf,
                inner.x + 2,
                inner.y + 1,
                "no callers yet. you would be the first.",
                inner.width.saturating_sub(2),
                ctx.fg(Dos::DarkGray),
            );
            return;
        }
        for (i, c) in self.callers.iter().enumerate() {
            let y = inner.y + u16::try_from(i).unwrap_or(0);
            let (label, colour) = match c.status {
                CallerStatus::Online => ("ONLINE", Dos::LightGreen),
                CallerStatus::Lost => ("LOST", Dos::LightRed),
                CallerStatus::Ascended => ("ASCENDED", Dos::Yellow),
            };
            let line = format!(
                " {:<16}  LEVEL {:02}  {}  {}",
                c.handle, c.depth, c.clock, label
            );
            put_str(buf, inner.x, y, &line, inner.width, ctx.fg(Dos::LightGray));
            let at = u16::try_from(line.chars().count() - label.len()).unwrap_or(0);
            put_str(
                buf,
                inner.x + at,
                y,
                label,
                inner.width.saturating_sub(at),
                ctx.fg(colour),
            );
        }
    }
}

impl ScreenImpl for LoginScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        match self.step.clone() {
            Step::Welcome(_) => Self::finish_welcome(ctx),
            Step::Confirm(handle) => {
                self.handle_confirm(&key, &handle);
                Nav::Stay
            }
            step => {
                match self.input.handle_key(&key) {
                    InputEvent::Changed => self.message = None,
                    InputEvent::Cancel => {
                        return if step == Step::Handle {
                            Nav::To(Box::new(Screen::Dialer(DialerScreen::new(ctx))))
                        } else {
                            self.reset_to_handle();
                            Nav::Stay
                        };
                    }
                    InputEvent::Submit(text) => {
                        let text = text.trim().to_string();
                        match step {
                            Step::Handle => self.submit_handle(&text, ctx),
                            Step::RealName(handle) => {
                                let name = if text.is_empty() {
                                    "N/A".to_string()
                                } else {
                                    text
                                };
                                self.transcript.push(format!(
                                    "Real name (it will be used against you): {name}"
                                ));
                                self.input =
                                    TextInput::new("Where did you hear about ABRAXAS? ", MAX_HEARD);
                                self.step = Step::Heard {
                                    handle,
                                    real_name: name,
                                };
                            }
                            Step::Heard { handle, real_name } => {
                                let heard = if text.is_empty() {
                                    "a dream".to_string()
                                } else {
                                    text
                                };
                                return self.register(&handle, &real_name, &heard, ctx);
                            }
                            Step::Confirm(_) | Step::Welcome(_) => {}
                        }
                    }
                    InputEvent::None => {}
                }
                Nav::Stay
            }
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        if let Step::Welcome(t) = &mut self.step {
            *t += dt.as_secs_f32();
            if *t >= WELCOME_SECS {
                return Self::finish_welcome(ctx);
            }
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        fill(buf, area, ctx.text_style());
        self.draw_logo(buf, area);
        self.draw_callers(buf, area, ctx);

        let mut y = area.y + PROMPT_ROW;
        for line in &self.transcript {
            put_str(buf, area.x + 2, y, line, area.width - 4, ctx.text_style());
            y += 1;
        }
        let message_y = match &self.step {
            Step::Confirm(_) | Step::Welcome(_) => y,
            _ => y + 1,
        };
        match &self.step {
            Step::Confirm(handle) => {
                let text = format!("Register as {handle}? [Y/n] ");
                let used = put_str(buf, area.x + 2, y, &text, area.width - 4, ctx.text_style());
                put_str(buf, area.x + 2 + used, y, "_", 1, ctx.fg(Dos::White));
            }
            Step::Welcome(_) => {}
            _ => {
                let rect = Rect::new(area.x + 2, y, area.width - 4, 1);
                self.input.render(buf, rect, ctx.fg(Dos::White));
            }
        }
        if let Some((msg, colour)) = &self.message {
            put_str(
                buf,
                area.x + 2,
                message_y,
                msg,
                area.width - 4,
                ctx.fg(*colour),
            );
        }
    }

    fn in_text_input(&self) -> bool {
        matches!(
            self.step,
            Step::Handle | Step::RealName(_) | Step::Heard { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::Phase;
    use crate::ui::app::{App, install_test_run, test_ctx};
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{Event, KeyCode};
    use std::path::PathBuf;

    fn nav_name(n: &Nav) -> String {
        match n {
            Nav::Stay => "Stay".into(),
            Nav::To(s) => format!("To {}", s.name()),
            Nav::Push(s) => format!("Push {}", s.name()),
            Nav::Back => "Back".into(),
            Nav::Quit => "Quit".into(),
        }
    }

    fn draw(screen: &LoginScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(screen: &LoginScreen, ctx: &Ctx) -> String {
        let buf = draw(screen, ctx);
        (0..25)
            .map(|y| row_text(&buf, buf.area, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Types `s` and presses Enter; returns the nav of the Enter.
    fn enter(screen: &mut LoginScreen, ctx: &mut Ctx, s: &str) -> Nav {
        for c in s.chars() {
            screen.handle_key(press_char(c), ctx);
        }
        screen.handle_key(press(KeyCode::Enter), ctx)
    }

    fn lost(ctx: &Ctx, handle: &str) {
        ctx.store
            .record_caller(CallerRecord {
                handle: handle.into(),
                depth: 4,
                clock: "23:16:00".into(),
                status: CallerStatus::Lost,
            })
            .unwrap();
    }

    #[test]
    fn handles_must_be_two_to_sixteen_characters_of_the_allowed_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = LoginScreen::new(&mut ctx);
        assert!(s.in_text_input());
        assert_eq!(nav_name(&enter(&mut s, &mut ctx, "x")), "Stay");
        assert_eq!(s.message(), Some("A handle is 2 to 16 characters."));
        s.handle_key(press(KeyCode::Backspace), &mut ctx);
        assert_eq!(s.message(), None, "editing clears the complaint");
        assert_eq!(nav_name(&enter(&mut s, &mut ctx, "bad name")), "Stay");
        assert_eq!(s.message(), Some("Use letters, digits, - and _ only."));
        assert!(text(&s, &ctx).contains("Use letters, digits"));
        assert!(ctx.run.is_none());
        assert!(!ctx.store.exists("bad name"));
    }

    #[test]
    fn cast_handles_are_taken_in_any_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        for name in ["PHILEMON", "wren", "Syzygy"] {
            let mut s = LoginScreen::new(&mut ctx);
            assert_eq!(nav_name(&enter(&mut s, &mut ctx, name)), "Stay");
            assert_eq!(s.message(), Some("That handle is taken."), "{name}");
            assert!(matches!(s.step, Step::Handle));
        }
    }

    #[test]
    fn dead_handles_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        lost(&ctx, "Ghost");
        let mut s = LoginScreen::new(&mut ctx);
        assert_eq!(nav_name(&enter(&mut s, &mut ctx, "ghost")), "Stay");
        assert_eq!(s.message(), Some("That handle belongs to the dead."));
        assert!(matches!(s.step, Step::Handle));
    }

    #[test]
    fn registration_asks_three_questions_and_creates_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = LoginScreen::new(&mut ctx);
        assert_eq!(nav_name(&enter(&mut s, &mut ctx, "Newbie")), "Stay");
        assert!(!s.in_text_input());
        assert!(text(&s, &ctx).contains("Register as Newbie? [Y/n]"));
        s.handle_key(press_char('y'), &mut ctx);
        assert!(s.in_text_input());
        assert!(text(&s, &ctx).contains("Real name (it will be used against you):"));
        assert_eq!(nav_name(&enter(&mut s, &mut ctx, "Ada Lovelace")), "Stay");
        assert!(text(&s, &ctx).contains("Where did you hear about ABRAXAS?"));
        let nav = enter(&mut s, &mut ctx, "a payphone");
        assert_eq!(nav_name(&nav), "To LevelCard");

        let run = ctx.run.as_ref().unwrap();
        assert_eq!(run.handle, "Newbie");
        assert_eq!(run.real_name, "Ada Lovelace");
        assert_eq!(run.heard_from, "a payphone");
        assert_eq!(run.seed, 42);
        assert_eq!(run.pack_id, "rollover");
        assert_eq!(run.pack_dir, None);
        assert_eq!(run.depth, 0);
        assert_eq!(run.phase, Phase::Arrival);
        let level = run.levels.get(&0).expect("level 0 skeleton");
        assert!(level.place.is_none(), "generation is the app's job");
        assert_eq!(level.seed.depth, 0);

        let saved = ctx.store.load("newbie").unwrap().expect("save written");
        assert_eq!(saved.real_name, "Ada Lovelace");
        let callers = ctx.store.callers();
        assert_eq!(callers[0].handle, "Newbie");
        assert_eq!(callers[0].status, CallerStatus::Online);
        assert_eq!(callers[0].depth, 0);
    }

    #[test]
    fn registration_takes_the_seed_and_pack_dir_from_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        ctx.opts.seed = Some(7);
        ctx.opts.pack = Some(PathBuf::from("/some/pack"));
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "Newbie");
        s.handle_key(press(KeyCode::Enter), &mut ctx);
        enter(&mut s, &mut ctx, "");
        enter(&mut s, &mut ctx, "");
        let run = ctx.run.as_ref().unwrap();
        assert_eq!(run.seed, 7);
        assert_eq!(run.pack_dir, Some(PathBuf::from("/some/pack")));
        assert_eq!(
            run.real_name, "N/A",
            "an empty answer still gives a usable name"
        );
        assert_eq!(run.heard_from, "a dream");
    }

    #[test]
    fn declining_registration_returns_to_the_handle_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "Newbie");
        s.handle_key(press_char('N'), &mut ctx);
        assert!(s.in_text_input());
        let t = text(&s, &ctx);
        assert!(t.contains("Handle: "));
        assert!(!t.contains("Register as"));
        assert!(ctx.run.is_none());
    }

    #[test]
    fn esc_leaves_the_handle_prompt_for_the_dialer_and_steps_back_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = LoginScreen::new(&mut ctx);
        let nav = s.handle_key(press(KeyCode::Esc), &mut ctx);
        assert_eq!(nav_name(&nav), "To Dialer");

        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "Newbie");
        s.handle_key(press_char('y'), &mut ctx);
        let nav = s.handle_key(press(KeyCode::Esc), &mut ctx);
        assert_eq!(nav_name(&nav), "Stay");
        assert!(matches!(s.step, Step::Handle));
        assert!(ctx.run.is_none());
    }

    fn saved_run(ctx: &mut Ctx, depth: u32, phase: Phase) {
        install_test_run(ctx, depth);
        let run = ctx.run.as_mut().unwrap();
        run.phase = phase;
        let run = ctx.run.take().unwrap();
        ctx.store.save(&run).unwrap();
    }

    fn resume_nav(phase: Phase) -> (String, Ctx) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        saved_run(&mut ctx, 3, phase);
        let mut s = LoginScreen::new(&mut ctx);
        let nav = enter(&mut s, &mut ctx, "tester");
        assert_eq!(nav_name(&nav), "Stay");
        assert!(text(&s, &ctx).contains("Welcome back."));
        assert!(ctx.run.is_some());
        let nav = s.tick(Duration::from_millis(1500), &mut ctx);
        (nav_name(&nav), ctx)
    }

    #[test]
    fn welcome_back_resumes_at_the_level_card_unless_descending_or_ending() {
        for phase in [
            Phase::Arrival,
            Phase::Hub {
                door_done: true,
                choice: Some(1),
            },
        ] {
            let (nav, ctx) = resume_nav(phase);
            assert_eq!(nav, "To LevelCard", "{phase:?}");
            let run = ctx.run.as_ref().unwrap();
            assert_eq!(run.depth, 3);
            assert_eq!(run.phase, phase, "login must not touch the phase");
        }
        let (nav, _) = resume_nav(Phase::Descending { exit: 2 });
        assert_eq!(nav, "To Descent");
        let (nav, ctx) = resume_nav(Phase::Ending);
        assert_eq!(nav, "To Ending");
        assert_eq!(ctx.run.as_ref().unwrap().phase, Phase::Ending);
    }

    #[test]
    fn a_resume_records_the_caller_online_and_any_key_skips_the_greeting() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        saved_run(&mut ctx, 3, Phase::Arrival);
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "TESTER");
        let rec = &ctx.store.callers()[0];
        assert_eq!((rec.handle.as_str(), rec.depth), ("Tester", 3));
        assert_eq!(rec.status, CallerStatus::Online);
        let nav = s.handle_key(press_char(' '), &mut ctx);
        assert_eq!(nav_name(&nav), "To LevelCard");
    }

    #[test]
    fn a_run_from_another_pack_is_refused_when_that_pack_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, 1);
        let mut run = ctx.run.take().unwrap();
        run.pack_id = "other-pack".into();
        ctx.store.save(&run).unwrap();

        // No pack directory recorded: the embedded pack is the only candidate and its id differs.
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "tester");
        assert!(s.message().unwrap().contains("other-pack"));
        assert!(matches!(s.step, Step::Handle));
        assert!(ctx.run.is_none());

        // A recorded directory that does not exist.
        run.pack_dir = Some(dir.path().join("missing-pack"));
        ctx.store.save(&run).unwrap();
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "tester");
        assert!(
            s.message().unwrap().contains("cannot be opened"),
            "{:?}",
            s.message()
        );
        assert!(ctx.run.is_none());
        assert_eq!(ctx.pack.meta.id, "rollover");
    }

    #[test]
    fn a_run_from_a_pack_directory_swaps_in_that_pack() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let pack_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packs/_template");
        let template = pack::open_for_run(Some(&pack_dir)).unwrap();
        let template_id = template.meta.id.clone();
        install_test_run(&mut ctx, 1);
        let mut run = ctx.run.take().unwrap();
        run.pack_id = template_id.clone();
        run.pack_dir = Some(pack_dir);
        ctx.store.save(&run).unwrap();
        let mut s = LoginScreen::new(&mut ctx);
        enter(&mut s, &mut ctx, "tester");
        assert_eq!(ctx.pack.meta.id, template_id);
        assert!(ctx.run.is_some());
        assert!(text(&s, &ctx).contains("Welcome back."));
    }

    #[test]
    fn shows_the_logo_and_the_last_callers() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let mut s = LoginScreen::new(&mut ctx);
        let t = text(&s, &ctx);
        assert!(t.contains("█████╗"), "logo missing");
        assert!(t.contains("what holds both sides"));
        assert!(t.contains("LAST CALLERS"));
        assert!(t.contains("no callers yet"));
        assert!(t.contains("Handle: "));

        lost(&ctx, "Ghost");
        ctx.store
            .record_caller(CallerRecord {
                handle: "Drywall".into(),
                depth: 3,
                clock: "23:12:00".into(),
                status: CallerStatus::Online,
            })
            .unwrap();
        s = LoginScreen::new(&mut ctx);
        let t = text(&s, &ctx);
        let rows: Vec<&str> = t.lines().collect();
        let drywall = rows.iter().position(|r| r.contains("Drywall")).unwrap();
        let ghost = rows.iter().position(|r| r.contains("Ghost")).unwrap();
        assert!(drywall < ghost, "newest caller first");
        assert!(rows[drywall].contains("LEVEL 03") && rows[drywall].contains("23:12:00"));
        assert!(rows[drywall].contains("ONLINE"));
        assert!(rows[ghost].contains("LOST"));
    }

    #[test]
    fn the_flow_through_the_app_registers_and_hands_over_to_the_level_card() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let login = Screen::Login(LoginScreen::new(&mut ctx));
        let mut app = App::with_screen(ctx, login);
        let dt = Duration::from_millis(33);
        let key = |c: KeyCode| Some(Event::Key(press(c)));
        for c in "Tess".chars() {
            app.step(key(KeyCode::Char(c)), dt);
        }
        // `?` types a question mark instead of opening help while the input is focused.
        app.step(key(KeyCode::Char('?')), dt);
        assert!(app.overlay.is_none());
        app.step(key(KeyCode::Backspace), dt);
        app.step(key(KeyCode::Enter), dt);
        app.step(key(KeyCode::Char('y')), dt);
        for c in "Ada".chars() {
            app.step(key(KeyCode::Char(c)), dt);
        }
        app.step(key(KeyCode::Enter), dt);
        app.step(key(KeyCode::Enter), dt);
        assert_eq!(app.screen.name(), "LevelCard");
        assert_eq!(app.ctx.run.as_ref().unwrap().handle, "Tess");
    }
}
