//! The app skeleton end to end through `App::step` and a `TestBackend`.

use std::time::Duration;

use abraxas::ui::app::{App, test_ctx};
use abraxas::ui::canvas::row_text;
use abraxas::ui::keys::press_char;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::Event;

fn screen_text(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.render(f)).unwrap();
    let buf = term.backend().buffer();
    (0..h).map(|y| row_text(buf, buf.area, y) + "\n").collect()
}

#[test]
fn boot_shows_the_bios_then_any_key_reaches_the_dialer() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(test_ctx(dir.path()));

    app.step(None, Duration::from_millis(1500));
    let mid = screen_text(&mut app, 80, 25);
    assert!(mid.contains("HX-BIOS v4.51PG"), "{mid}");
    assert!(mid.contains("Memory Test : "), "{mid}");

    // Let the boot finish its typing but not hand over: the prompt is on screen.
    app.step(None, Duration::from_millis(1700));
    let late = screen_text(&mut app, 80, 25);
    assert!(late.contains("Memory Test : 65536K OK"), "{late}");
    assert!(late.contains("C:\\> TERMINUS"), "{late}");

    app.step(Some(Event::Key(press_char(' '))), Duration::from_millis(16));
    let dialer = screen_text(&mut app, 80, 25);
    assert!(dialer.contains("TERMINUS 3.5"), "{dialer}");
    assert!(dialer.contains("NIGHT SHIFT BBS (Wren)"), "{dialer}");
    assert!(
        !dialer.contains("ABRAXAS"),
        "a fresh install does not list ABRAXAS"
    );
}

#[test]
fn a_terminal_one_row_short_gets_only_the_size_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(test_ctx(dir.path()));
    app.step(None, Duration::from_millis(500));
    let text = screen_text(&mut app, 79, 24);
    assert_eq!(
        text.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "{text}"
    );
    assert!(text.contains("ABRAXAS needs an 80x25 terminal. This one is 79x24."));
}
