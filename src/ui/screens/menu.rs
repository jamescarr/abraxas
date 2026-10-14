//! The BBS main menu: the board's banner, the hotkey list, the sysop news and the command line.

use std::time::Duration;

use rand::RngExt;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{
    ArchiveScreen, BoardScreen, ChatScreen, ExitsScreen, FilesScreen, Nav, PsycheScreen, RunScreen,
    Screen, ScreenImpl, UsersScreen,
};
use crate::game::state::Phase;
use crate::pack::model::Archetype;
use crate::palette::Dos;
use crate::text::{truncate, wrap};
use crate::ui::app::{Ctx, FxCommand};
use crate::ui::canvas::{fill, put_centered, put_str};
use crate::ui::widgets::banner::{BoxKind, boxed, render_name};
use crate::ui::widgets::menu::{Menu, MenuEvent, MenuItem};

const TAGLINE_ROW: u16 = 4;
const INFO_ROW: u16 = 5;
const MENU_ROW: u16 = 7;
const NEWS_ROW: u16 = 16;
const MESSAGE_ROW: u16 = 21;
const PROMPT_ROW: u16 = 22;
const MENU_X: u16 = 2;
const MENU_W: u16 = 76;
/// Longest objective summary in the Door item (column width 38 - key 3 - space - `Door: `, minus a gap).
const DOOR_OBJECTIVE_CHARS: usize = 26;
const NEWS_LINES: usize = 3;
/// Seconds of screen time between Trickster glitch bursts.
const GLITCH_GAP: std::ops::Range<f32> = 20.0..40.0;
const GLITCH_MS: u64 = 1500;

const DOOR_DONE_MSG: &str = "The door is open. Nothing left inside.";
const SEALED_MSG: &str = "The exits are sealed until the door is opened.";

/// The Menu screen.
pub struct MenuScreen {
    menu: Menu,
    message: Option<&'static str>,
    /// Screen time elapsed and when the next Trickster glitch fires (`None`: no Trickster).
    t: f32,
    next_glitch: Option<f32>,
}

impl MenuScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let trickster = ctx
            .focus_character()
            .is_some_and(|c| c.archetype == Archetype::Trickster);
        let next_glitch = trickster.then(|| ctx.rng.random_range(GLITCH_GAP));
        let mut screen = Self {
            menu: Menu::new(Vec::new()),
            message: None,
            t: 0.0,
            next_glitch,
        };
        screen.menu = Self::build_menu(ctx);
        screen
    }

    fn door_done(ctx: &Ctx) -> bool {
        matches!(
            ctx.run_ref().map(|r| r.phase),
            Some(Phase::Hub {
                door_done: true,
                ..
            })
        )
    }

    fn build_menu(ctx: &Ctx) -> Menu {
        let objective = ctx.place().map_or(String::new(), |p| {
            truncate(&p.host.objective, DOOR_OBJECTIVE_CHARS)
        });
        let done = Self::door_done(ctx);
        let door = if done {
            "Door: (done)".to_string()
        } else {
            format!("Door: {objective}")
        };
        let mut door_item = MenuItem::new('d', door);
        if done {
            door_item = door_item.disabled();
        }
        let mut goodbye = MenuItem::new('g', "Goodbye");
        if !done {
            goodbye = goodbye.disabled();
        }
        Menu::new(vec![
            MenuItem::new('m', "Message bases"),
            MenuItem::new('f', "File library"),
            MenuItem::new('c', "Page the SysOp"),
            MenuItem::new('u', "User list"),
            door_item,
            MenuItem::new('a', "Archive"),
            MenuItem::new('s', "Self"),
            goodbye,
        ])
        .with_columns(2)
        .with_row_height(2)
    }

    /// Re-read the door state: the run may have changed while a leaf screen or the shell was on
    /// top of this one.
    fn sync(&mut self, ctx: &Ctx) {
        let done = Self::door_done(ctx);
        let door_disabled = self.menu.items().iter().any(|i| i.key == 'd' && !i.enabled);
        if done != door_disabled {
            let selected = self.menu.selected();
            let mut menu = Self::build_menu(ctx);
            if let Some(key) = self.menu.items().get(selected).map(|i| i.key) {
                menu.select_key(key);
            }
            self.menu = menu;
        }
    }

    /// The line above the command prompt, if any.
    pub fn message(&self) -> Option<&str> {
        self.message
    }

    fn activate(&mut self, key: char, ctx: &mut Ctx) -> Nav {
        let screen = match key {
            'm' => Screen::Board(BoardScreen::new(ctx)),
            'f' => Screen::Files(FilesScreen::new(ctx)),
            'c' => Screen::Chat(ChatScreen::new(ctx)),
            'u' => Screen::Users(UsersScreen::new(ctx)),
            'a' => Screen::Archive(ArchiveScreen::new(ctx)),
            's' => Screen::Psyche(PsycheScreen::new(ctx)),
            'd' => Screen::Run(RunScreen::new(ctx)),
            'g' => Screen::Exits(ExitsScreen::new(ctx)),
            _ => return Nav::Stay,
        };
        if key == 'g' {
            Nav::To(Box::new(screen))
        } else {
            Nav::Push(Box::new(screen))
        }
    }

    fn draw_news(&self, buf: &mut ratatui::buffer::Buffer, area: Rect, ctx: &Ctx) {
        let frame = Rect::new(area.x + MENU_X, area.y + NEWS_ROW, MENU_W, 5);
        let inner = boxed(
            buf,
            frame,
            Some("SYSOP NEWS"),
            BoxKind::Single,
            ctx.fg(Dos::DarkGray),
        );
        let Some(place) = ctx.place() else {
            return;
        };
        let width = usize::from(inner.width).saturating_sub(2);
        let mut lines = wrap(&place.level_card, width);
        if lines.len() > NEWS_LINES {
            lines.truncate(NEWS_LINES);
            if let Some(last) = lines.last_mut() {
                *last = truncate(&format!("{last} ..."), width);
            }
        }
        for (i, line) in lines.iter().enumerate() {
            let y = inner.y + u16::try_from(i).unwrap_or(0);
            put_str(buf, inner.x + 1, y, line, inner.width - 2, ctx.text_style());
        }
    }
}

impl ScreenImpl for MenuScreen {
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Nav {
        self.sync(ctx);
        self.message = None;
        match self.menu.handle_key(&key) {
            MenuEvent::Activated(c) => self.activate(c, ctx),
            MenuEvent::Disabled('d') => {
                self.message = Some(DOOR_DONE_MSG);
                Nav::Stay
            }
            MenuEvent::Disabled('g') => {
                self.message = Some(SEALED_MSG);
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) -> Nav {
        self.sync(ctx);
        self.t += dt.as_secs_f32();
        if let Some(next) = self.next_glitch
            && self.t >= next
        {
            if !ctx.calm() {
                ctx.fx.push(FxCommand::Glitch { ms: GLITCH_MS });
            }
            self.next_glitch = Some(next + ctx.rng.random_range(GLITCH_GAP));
        }
        Nav::Stay
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let buf = frame.buffer_mut();
        fill(buf, area, ctx.text_style());
        let (Some(run), Some(level), Some(place)) = (ctx.run_ref(), ctx.level(), ctx.place())
        else {
            return;
        };
        let black = ctx.palette.color(Dos::Black);
        render_name(
            buf,
            Rect::new(area.x, area.y, area.width, 4),
            &place.bbs_name,
            ctx.gradient(),
            ctx.chrome(),
            black,
        );
        let chrome = ratatui::style::Style::new().fg(ctx.chrome()).bg(black);
        put_centered(buf, area, area.y + TAGLINE_ROW, &place.tagline, chrome);

        let info = format!(
            " SysOp: {} │ Level {:02} │ Class {} ",
            place.sysop.handle, run.depth, level.seed.base_danger
        );
        let len = info.chars().count();
        let pad = usize::from(area.width).saturating_sub(len);
        let (left, right) = (pad / 2, pad - pad / 2);
        let line = format!("{}{info}{}", "═".repeat(left), "═".repeat(right));
        put_str(buf, area.x, area.y + INFO_ROW, &line, area.width, chrome);

        self.menu.render(
            buf,
            Rect::new(area.x + MENU_X, area.y + MENU_ROW, MENU_W, 8),
            &ctx.palette,
            ctx.chrome(),
        );
        self.draw_news(buf, area, ctx);

        if let Some(msg) = self.message {
            put_str(
                buf,
                area.x + MENU_X,
                area.y + MESSAGE_ROW,
                msg,
                MENU_W,
                ctx.fg(Dos::Yellow),
            );
        }
        let used = put_str(
            buf,
            area.x + MENU_X,
            area.y + PROMPT_ROW,
            "Command (?=Help): ",
            MENU_W,
            ctx.text_style(),
        );
        put_str(
            buf,
            area.x + MENU_X + used,
            area.y + PROMPT_ROW,
            "_",
            1,
            ctx.fg(Dos::White),
        );
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
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    fn nav_name(n: &Nav) -> String {
        match n {
            Nav::Stay => "Stay".into(),
            Nav::To(s) => format!("To {}", s.name()),
            Nav::Push(s) => format!("Push {}", s.name()),
            Nav::Back => "Back".into(),
            Nav::Quit => "Quit".into(),
        }
    }

    fn draw(screen: &MenuScreen, ctx: &Ctx) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
        term.draw(|f| screen.render(f, f.area(), ctx)).unwrap();
        term.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..25).map(|y| row_text(buf, buf.area, y)).collect()
    }

    fn menu_at(depth: u32, door_done: bool) -> (Ctx, MenuScreen, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        ctx.run.as_mut().unwrap().phase = Phase::Hub {
            door_done,
            choice: None,
        };
        let s = MenuScreen::new(&mut ctx);
        (ctx, s, dir)
    }

    fn glitches(ctx: &mut Ctx) -> usize {
        let n = ctx
            .fx
            .iter()
            .filter(|f| matches!(f, FxCommand::Glitch { ms: 1500 }))
            .count();
        ctx.fx.clear();
        n
    }

    #[test]
    fn hotkeys_open_the_leaf_screens_and_the_shell_on_top_of_the_menu() {
        let (mut ctx, mut s, _d) = menu_at(3, false);
        for (key, want) in [
            ('m', "Push Board"),
            ('F', "Push Files"),
            ('c', "Push Chat"),
            ('u', "Push Users"),
            ('A', "Push Archive"),
            ('s', "Push Psyche"),
            ('d', "Push Run"),
        ] {
            let nav = s.handle_key(press_char(key), &mut ctx);
            assert_eq!(nav_name(&nav), want, "key {key}");
        }
        assert!(s.message().is_none());
    }

    #[test]
    fn enter_activates_the_highlighted_item_and_esc_is_ignored() {
        let (mut ctx, mut s, _d) = menu_at(3, false);
        assert_eq!(
            nav_name(&s.handle_key(press(KeyCode::Esc), &mut ctx)),
            "Stay"
        );
        assert_eq!(
            nav_name(&s.handle_key(press(KeyCode::Enter), &mut ctx)),
            "Push Board"
        );
        s.handle_key(press(KeyCode::Right), &mut ctx);
        assert_eq!(
            nav_name(&s.handle_key(press(KeyCode::Enter), &mut ctx)),
            "Push Files"
        );
    }

    #[test]
    fn goodbye_is_sealed_until_the_door_is_done() {
        let (mut ctx, mut s, _d) = menu_at(3, false);
        assert_eq!(nav_name(&s.handle_key(press_char('g'), &mut ctx)), "Stay");
        assert_eq!(s.message(), Some(SEALED_MSG));
        let r = rows(&draw(&s, &ctx));
        assert_eq!(
            r[21].trim(),
            "The exits are sealed until the door is opened."
        );
        // The next key clears the message.
        s.handle_key(press_char('z'), &mut ctx);
        assert!(rows(&draw(&s, &ctx))[21].trim().is_empty());
    }

    #[test]
    fn a_finished_door_opens_goodbye_and_refuses_a_second_visit() {
        let (mut ctx, mut s, _d) = menu_at(3, true);
        assert_eq!(nav_name(&s.handle_key(press_char('d'), &mut ctx)), "Stay");
        assert_eq!(
            rows(&draw(&s, &ctx))[21].trim(),
            "The door is open. Nothing left inside."
        );
        assert_eq!(
            nav_name(&s.handle_key(press_char('G'), &mut ctx)),
            "To Exits"
        );
    }

    #[test]
    fn the_menu_follows_the_door_state_after_a_screen_on_top_finishes_it() {
        let (mut ctx, mut s, _d) = menu_at(3, false);
        assert_eq!(nav_name(&s.handle_key(press_char('g'), &mut ctx)), "Stay");
        ctx.run.as_mut().unwrap().phase = Phase::Hub {
            door_done: true,
            choice: Some(0),
        };
        // The Menu instance survives under the Run screen; the next tick notices.
        s.tick(Duration::from_millis(33), &mut ctx);
        let buf = draw(&s, &ctx);
        let door = &rows(&buf)[11];
        assert!(door.contains("[D] Door: (done)"), "{door:?}");
        let dark = ctx.palette.color(Dos::DarkGray);
        assert_eq!(buf[(6, 11)].fg, dark, "a finished door is dark gray");
        assert_eq!(
            nav_name(&s.handle_key(press_char('g'), &mut ctx)),
            "To Exits"
        );
    }

    #[test]
    fn renders_the_board_exactly_as_the_wireframe() {
        let (ctx, s, _d) = menu_at(3, false);
        let r = rows(&draw(&s, &ctx));
        let place = ctx.place().unwrap();
        // 9-character names are big text: four rows of block glyphs.
        assert_eq!(place.bbs_name, "SOL NIGER");
        assert!(r[..4].iter().all(|l| l.contains(['█', '▀', '▄'])));
        assert_eq!(r[4].trim(), place.tagline);
        assert!(r[5].starts_with("═"), "{:?}", r[5]);
        assert!(r[5].ends_with("═"));
        assert!(
            r[5].contains(&format!(
                " SysOp: {} │ Level 03 │ Class 2 ",
                place.sysop.handle
            )),
            "{:?}",
            r[5]
        );
        assert!(r[7].contains("[M] Message bases") && r[7].contains("[F] File library"));
        assert!(r[9].contains("[C] Page the SysOp") && r[9].contains("[U] User list"));
        assert!(r[11].contains("[D] Door: ") && r[11].contains("[A] Archive"));
        assert!(r[13].contains("[S] Self") && r[13].contains("[G] Goodbye"));
        let objective: String = place.host.objective.chars().take(20).collect();
        assert!(r[11].contains(&objective), "{:?}", r[11]);
        assert!(r[16].contains("SYSOP NEWS"));
        let first_word = place.level_card.split_whitespace().next().unwrap();
        assert!(r[17].contains(first_word), "{:?}", r[17]);
        assert_eq!(r[22].trim(), "Command (?=Help): _");
    }

    #[test]
    fn long_news_is_cut_to_three_lines_with_an_ellipsis() {
        let (mut ctx, s, _d) = menu_at(3, false);
        let long = "wet carpet and a humming light ".repeat(20);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .place
            .as_mut()
            .unwrap()
            .level_card = long;
        let r = rows(&draw(&s, &ctx));
        assert!(r[17].contains("wet carpet"));
        assert!(r[19].contains("..."), "{:?}", r[19]);
        assert!(r[20].starts_with("  └"), "the box closes on row 20");
    }

    #[test]
    fn the_door_item_truncates_a_long_objective() {
        let (mut ctx, _s, _d) = menu_at(3, false);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .place
            .as_mut()
            .unwrap()
            .host
            .objective =
            "Retrieve the black sun ledger from the deepest vault of the exchange".into();
        let s = MenuScreen::new(&mut ctx);
        let line = rows(&draw(&s, &ctx))[11].clone();
        assert!(
            line.contains("Door: Retrieve the black sun ..."),
            "{line:?}"
        );
        assert!(
            line.contains("[A] Archive"),
            "the second column stays intact"
        );
    }

    fn glitch_times(ctx: &mut Ctx, s: &mut MenuScreen, secs: f32) -> Vec<f32> {
        let dt = Duration::from_millis(250);
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < secs {
            t += 0.25;
            s.tick(dt, ctx);
            for _ in 0..glitches(ctx) {
                out.push(t);
            }
        }
        out
    }

    #[test]
    fn the_trickster_glitches_the_menu_every_twenty_to_forty_seconds() {
        let (mut ctx, _s, _d) = menu_at(3, false);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .seed
            .focus = Some("mercurius".into());
        let mut s = MenuScreen::new(&mut ctx);
        let times = glitch_times(&mut ctx, &mut s, 400.0);
        assert!(times.len() >= 10, "{times:?}");
        assert!((20.0..=40.25).contains(&times[0]), "{times:?}");
        for w in times.windows(2) {
            let gap = w[1] - w[0];
            assert!((19.75..=40.25).contains(&gap), "{times:?}");
        }
    }

    #[test]
    fn no_glitches_in_calm_mode_or_without_a_trickster() {
        let (mut ctx, _s, _d) = menu_at(3, false);
        ctx.run
            .as_mut()
            .unwrap()
            .levels
            .get_mut(&3)
            .unwrap()
            .seed
            .focus = Some("mercurius".into());
        ctx.config.display.calm = true;
        let mut s = MenuScreen::new(&mut ctx);
        assert!(glitch_times(&mut ctx, &mut s, 200.0).is_empty());

        let (mut ctx, mut s, _d) = menu_at(3, false);
        assert_eq!(ctx.level().unwrap().seed.focus.as_deref(), Some("shadow"));
        assert!(glitch_times(&mut ctx, &mut s, 200.0).is_empty());
    }
}
