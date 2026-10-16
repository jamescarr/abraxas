//! The whole game loop, driven through `App::step` on a 80x25 `TestBackend` with the offline
//! director: boot, dial, register, play level 0's door, choose, jack out, descend to level 1.

use std::time::Duration;

use abraxas::ui::app::{App, test_ctx};
use abraxas::ui::canvas::row_text;
use abraxas::ui::keys::{press, press_char};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode};

const FRAME: Duration = Duration::from_millis(100);

/// A test context in calm mode: screens are rendered only when asserted on, so timed effects
/// (glitch, dissolve) would otherwise still be mid-flight in the captured frames.
fn calm_ctx(dir: &std::path::Path) -> abraxas::ui::app::Ctx {
    let mut ctx = test_ctx(dir);
    ctx.config.display.calm = true;
    ctx
}

fn screen_text(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(80, 25)).unwrap();
    term.draw(|f| app.render(f)).unwrap();
    let buf = term.backend().buffer();
    (0..25).map(|y| row_text(buf, buf.area, y) + "\n").collect()
}

fn key(app: &mut App, code: KeyCode) {
    app.step(Some(Event::Key(press(code))), FRAME);
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.step(Some(Event::Key(press_char(c))), FRAME);
    }
}

/// Step with no input until the current screen is `name` (or fail after `max_secs`).
fn advance_to(app: &mut App, name: &str, max_secs: u64) {
    let frames = max_secs * 10;
    for _ in 0..frames {
        if app.screen.name() == name {
            return;
        }
        app.step(None, FRAME);
    }
    panic!(
        "never reached {name}; stuck on {}:\n{}",
        app.screen.name(),
        screen_text(app)
    );
}

/// Generation requested while pumping is delivered on the following frame; let a few pass.
fn settle(app: &mut App) {
    for _ in 0..3 {
        app.step(None, FRAME);
    }
}

fn expect(app: &mut App, needle: &str) {
    let text = screen_text(app);
    assert!(
        text.contains(needle),
        "expected {needle:?} on {}:\n{text}",
        app.screen.name()
    );
}

#[test]
fn a_new_caller_plays_level_zero_and_falls_to_level_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(calm_ctx(dir.path()));

    // Boot -> Dialer.
    app.step(None, Duration::from_millis(500));
    expect(&mut app, "HX-BIOS");
    app.step(Some(Event::Key(press_char(' '))), FRAME);
    assert_eq!(app.screen.name(), "Dialer");

    // Dial Wren's board; the line crosses; any key skips the modem show.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.screen.name(), "Dialing");
    for _ in 0..80 {
        if app.screen.name() == "Login" {
            break;
        }
        app.step(Some(Event::Key(press_char(' '))), FRAME);
    }
    assert_eq!(app.screen.name(), "Login");
    expect(&mut app, "Handle:");

    // Register TESTER.
    type_text(&mut app, "TESTER");
    key(&mut app, KeyCode::Enter);
    expect(&mut app, "Register as TESTER? [Y/n]");
    type_text(&mut app, "y");
    expect(&mut app, "Real name");
    type_text(&mut app, "Test Person");
    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "a dream");
    key(&mut app, KeyCode::Enter);

    // Level card: LEVEL 0, THE LOBBY (the authored anchor).
    advance_to(&mut app, "LevelCard", 5);
    settle(&mut app);
    expect(&mut app, "LEVEL 00");
    expect(&mut app, "THE LOBBY");

    // Menu, then the door: the portal's WORDLOCK is dismissed with Esc, `xyzzy` completes the
    // objective in dev mode and the payload beat appears.
    app.step(Some(Event::Key(press_char(' '))), FRAME);
    assert_eq!(app.screen.name(), "Menu");
    app.step(Some(Event::Key(press_char('d'))), FRAME);
    assert_eq!(app.screen.name(), "Run");
    expect(&mut app, "ENTER PASSWORD NOW");
    key(&mut app, KeyCode::Esc);
    type_text(&mut app, "xyzzy");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.screen.name(), "Beat");
    expect(&mut app, "WREN-00.LOG");

    // Any key finishes the typing, another opens the choice; option 1 returns to the shell.
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.screen.name(), "Choice");
    type_text(&mut app, "1");
    assert_eq!(app.screen.name(), "Run");
    expect(&mut app, "The trace is still running. Jack out.");
    type_text(&mut app, "logout");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.screen.name(), "Menu");

    // Goodbye -> the first exit -> the descent -> level 1.
    app.step(Some(Event::Key(press_char('g'))), FRAME);
    assert_eq!(app.screen.name(), "Exits");
    type_text(&mut app, "1");
    assert_eq!(app.screen.name(), "Descent");
    advance_to(&mut app, "LevelCard", 40);
    settle(&mut app);
    expect(&mut app, "LEVEL 01");
    let run = app.ctx.run.as_ref().unwrap();
    assert_eq!(run.depth, 1);
    assert_eq!(run.stats.integration, 1, "the integrate option was chosen");
}

#[test]
fn quitting_mid_level_resumes_on_the_same_level_after_login() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut app = App::new(calm_ctx(dir.path()));
        app.step(None, Duration::from_millis(500));
        app.step(Some(Event::Key(press_char(' '))), FRAME);
        key(&mut app, KeyCode::Enter);
        for _ in 0..80 {
            if app.screen.name() == "Login" {
                break;
            }
            app.step(Some(Event::Key(press_char(' '))), FRAME);
        }
        type_text(&mut app, "TESTER");
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "y");
        key(&mut app, KeyCode::Enter);
        type_text(&mut app, "x");
        key(&mut app, KeyCode::Enter);
        advance_to(&mut app, "LevelCard", 5);
        settle(&mut app);
        app.step(Some(Event::Key(press_char(' '))), FRAME);
        assert_eq!(app.screen.name(), "Menu");
        // Ctrl+Q asks, Y saves and quits.
        app.step(Some(Event::Key(abraxas::ui::keys::press_ctrl('q'))), FRAME);
        expect(&mut app, "Hang up? [Y/n]");
        type_text(&mut app, "y");
        assert!(app.quit);
    }

    // A fresh process: the Dialer now lists ABRAXAS; dialling it and logging in as TESTER
    // lands on the level card of the saved depth.
    let mut app = App::new(calm_ctx(dir.path()));
    app.step(None, Duration::from_millis(500));
    app.step(Some(Event::Key(press_char(' '))), FRAME);
    expect(&mut app, "ABRAXAS");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    for _ in 0..80 {
        if app.screen.name() == "Login" {
            break;
        }
        app.step(Some(Event::Key(press_char(' '))), FRAME);
    }
    type_text(&mut app, "TESTER");
    key(&mut app, KeyCode::Enter);
    advance_to(&mut app, "LevelCard", 5);
    settle(&mut app);
    expect(&mut app, "LEVEL 00");
}
