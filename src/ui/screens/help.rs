//! The help overlay: keys and the host-shell commands. Drawn by the app over any screen
//! (`Overlay::Help`); it is not a `Screen`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::palette::Dos;
use crate::ui::app::Ctx;
use crate::ui::canvas::put_str;
use crate::ui::widgets::banner::{BoxKind, boxed};

/// `(keys, meaning)` rows of the global key list.
pub const KEYS: &[(&str, &str)] = &[
    ("F1 or ?", "This help"),
    ("Ctrl+C Ctrl+Q", "Hang up (asks)"),
    ("Alt+X", "Hang up (asks)"),
    ("Esc", "Back / cancel"),
    ("Arrows Enter", "Move, choose"),
    ("Letters", "Menu hotkeys"),
    ("PgUp PgDn Space", "Scroll text"),
    ("Any key", "Finish a transmission"),
    ("Up Down", "Shell: history"),
    ("Tab", "Shell: complete"),
    ("Ctrl+L", "Shell: clear"),
    ("1  2", "WORDLOCK programs"),
    ("Space Enter", "ROUTER: rotate"),
    ("X", "DUEL: accept"),
];

/// `(command, meaning)` rows of the host shell.
pub const COMMANDS: &[(&str, &str)] = &[
    ("help", "list commands"),
    ("scan", "draw the node map"),
    ("ls", "list files here"),
    ("cat FILE", "print a file"),
    ("connect NODE", "move (also: cd)"),
    ("get FILE", "download a file"),
    ("put FILE", "upload a file"),
    ("ps", "list processes"),
    ("kill PID", "kill a process"),
    ("bounce", "reroute the trace"),
    ("run PROG", "ghost, mandala"),
    ("deck", "your programs"),
    ("whoami", "who you are"),
    ("date", "the in-world clock"),
    ("clear", "clear the screen"),
    ("logout", "leave (exit, jackout)"),
];

/// Draw the help box centred in `canvas`.
pub fn render(buf: &mut Buffer, canvas: Rect, ctx: &Ctx) {
    let p = &ctx.palette;
    let area = Rect::new(
        canvas.x + 2,
        canvas.y + 1,
        canvas.width - 4,
        canvas.height - 3,
    );
    let frame = p.style(Dos::LightCyan, Dos::Blue);
    let inner = boxed(buf, area, Some("Help"), BoxKind::Double, frame);
    let heading = p.style(Dos::Yellow, Dos::Blue);
    let key = p.style(Dos::White, Dos::Blue);
    let text = p.style(Dos::LightGray, Dos::Blue);
    let half = 39;
    put_str(buf, inner.x + 1, inner.y, "KEYS", half, heading);
    put_str(buf, inner.x + half, inner.y, "HOST COMMANDS", 20, heading);
    for (i, (k, v)) in KEYS.iter().enumerate() {
        let y = inner.y + 2 + u16::try_from(i).unwrap_or(0);
        put_str(buf, inner.x + 1, y, k, 16, key);
        put_str(buf, inner.x + 17, y, v, half - 18, text);
    }
    for (i, (k, v)) in COMMANDS.iter().enumerate() {
        let y = inner.y + 2 + u16::try_from(i).unwrap_or(0);
        put_str(buf, inner.x + half, y, k, 14, key);
        put_str(
            buf,
            inner.x + half + 14,
            y,
            v,
            half.saturating_sub(15),
            text,
        );
    }
    put_str(
        buf,
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        "Esc closes this help.",
        inner.width.saturating_sub(2),
        p.style(Dos::LightCyan, Dos::Blue),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_ctx;
    use crate::ui::canvas::row_text;

    #[test]
    fn help_fits_the_canvas_and_lists_keys_and_commands() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path());
        let area = Rect::new(0, 0, 80, 25);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &ctx);
        let all: String = (0..25).map(|y| row_text(&buf, area, y) + "\n").collect();
        for (k, v) in KEYS.iter().chain(COMMANDS) {
            // Each entry is drawn in full: nothing clipped by the box.
            assert!(all.contains(k), "missing key text {k:?}");
            assert!(all.contains(v), "missing description {v:?}");
        }
        assert!(all.contains("Esc closes this help."));
    }
}
