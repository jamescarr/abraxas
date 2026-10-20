//! The help overlay: keys and the host-shell commands. Drawn by the app over any screen
//! (`Overlay::Help`); it is not a `Screen`.

use std::path::{Path, PathBuf};

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
    ("Tab", "Complete / hint"),
    ("Ctrl+L", "Shell: clear"),
    ("1  2", "WORDLOCK programs"),
    ("Space Enter", "ROUTER: rotate"),
    ("X", "DUEL: accept"),
    ("1-4", "DUEL: zine attacks"),
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
    let label = "Hidden line: ";
    let hidden = match ctx.hidden.path() {
        Some(path) => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let room = usize::from(inner.width.saturating_sub(2)).saturating_sub(label.len());
            format!("{label}{}", shown_path(path, home.as_deref(), room))
        }
        None => format!("{label}unavailable"),
    };
    put_str(
        buf,
        inner.x + 1,
        inner.bottom().saturating_sub(2),
        &hidden,
        inner.width.saturating_sub(2),
        p.style(Dos::LightGray, Dos::Blue),
    );
    put_str(
        buf,
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        "Esc closes this help.",
        inner.width.saturating_sub(2),
        p.style(Dos::LightCyan, Dos::Blue),
    );
}

/// A socket path as the help line shows it: the home directory as `$HOME` (the text then pastes
/// into a quoted shell word), and a path still longer than `max` characters keeps its tail
/// behind `...`.
fn shown_path(path: &Path, home: Option<&Path>, max: usize) -> String {
    let text = match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("$HOME/{}", rest.display()),
        None => path.display().to_string(),
    };
    let len = text.chars().count();
    if len <= max {
        return text;
    }
    let tail: String = text.chars().skip(len - max.saturating_sub(3)).collect();
    format!("...{tail}")
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
        assert!(all.contains("Hidden line: unavailable"));
    }

    #[test]
    fn the_default_macos_socket_path_fits_with_home_abbreviated() {
        let home = Path::new("/Users/jamescarr");
        let path = home.join("Library/Application Support/abraxas/abraxas.sock");
        let room = 72 - "Hidden line: ".len();
        assert!(
            path.display().to_string().chars().count() > room,
            "it needs the shortening"
        );
        assert_eq!(
            shown_path(&path, Some(home), room),
            "$HOME/Library/Application Support/abraxas/abraxas.sock"
        );
        let elsewhere = Path::new("/tmp/abx/abraxas.sock");
        assert_eq!(
            shown_path(elsewhere, Some(home), room),
            "/tmp/abx/abraxas.sock"
        );
    }

    #[test]
    fn a_path_too_long_even_so_keeps_its_tail() {
        let path = Path::new("/very/long/directory/name/that/goes/on/and/on/abraxas.sock");
        let shown = shown_path(path, None, 20);
        assert_eq!(shown.chars().count(), 20);
        assert!(shown.starts_with("..."), "{shown}");
        assert!(shown.ends_with("on/abraxas.sock"), "{shown}");
    }

    #[test]
    fn help_shows_the_socket_path_when_listening() {
        let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        let socket = dir.path().join("s.sock");
        ctx.hidden.listen(socket.clone()).unwrap();
        let area = Rect::new(0, 0, 80, 25);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &ctx);
        let all: String = (0..25).map(|y| row_text(&buf, area, y) + "\n").collect();
        let row = all.lines().find(|l| l.contains("Hidden line:")).unwrap();
        assert!(row.contains("s.sock"), "{row}");
        assert!(!row.contains("unavailable"), "{row}");
    }
}
