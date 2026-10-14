//! The 80x25 canvas: placement inside the terminal, the too-small message, and the tiny drawing
//! helpers screens use to write straight into a ratatui `Buffer`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Canvas width in columns.
pub const WIDTH: u16 = 80;
/// Canvas height in rows (row 24 is the status bar).
pub const HEIGHT: u16 = 25;
/// The row index of the status bar inside the canvas.
pub const STATUS_ROW: u16 = 24;

/// The 80x25 canvas centred in `terminal`, or `None` when the terminal is smaller than 80x25.
pub fn canvas_rect(terminal: Rect) -> Option<Rect> {
    if terminal.width < WIDTH || terminal.height < HEIGHT {
        return None;
    }
    Some(Rect::new(
        terminal.x + (terminal.width - WIDTH) / 2,
        terminal.y + (terminal.height - HEIGHT) / 2,
        WIDTH,
        HEIGHT,
    ))
}

/// The message shown on a terminal that is too small.
pub fn too_small_message(width: u16, height: u16) -> String {
    format!("ABRAXAS needs an 80x25 terminal. This one is {width}x{height}.")
}

/// Draw the too-small message centred in `area`, on black.
pub fn render_too_small(buf: &mut Buffer, area: Rect, style: Style) {
    let msg = too_small_message(area.width, area.height);
    let len = u16::try_from(msg.chars().count()).unwrap_or(u16::MAX);
    let x = area.x + area.width.saturating_sub(len) / 2;
    let y = area.y + area.height / 2;
    // A narrow terminal clips the message rather than wrapping it.
    let max = area.width.saturating_sub(x - area.x);
    buf.set_stringn(x, y, &msg, usize::from(max), style);
}

/// Write `text` at `(x, y)` (absolute buffer coordinates), clipped to `max_width` columns.
/// Returns the number of columns written.
pub fn put_str(buf: &mut Buffer, x: u16, y: u16, text: &str, max_width: u16, style: Style) -> u16 {
    let (nx, _) = buf.set_stringn(x, y, text, usize::from(max_width), style);
    nx - x
}

/// Write `text` centred horizontally inside `area` on row `y` (absolute).
pub fn put_centered(buf: &mut Buffer, area: Rect, y: u16, text: &str, style: Style) {
    let len = u16::try_from(text.chars().count()).unwrap_or(u16::MAX);
    let x = area.x + area.width.saturating_sub(len) / 2;
    put_str(buf, x, y, text, area.width, style);
}

/// Paint `style` over `area` and blank its symbols.
pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
    }
}

/// Text of row `y` (absolute) of `area` as a string, for tests and diagnostics.
pub fn row_text(buf: &Buffer, area: Rect, y: u16) -> String {
    (area.left()..area.right())
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn centres_the_canvas() {
        assert_eq!(
            canvas_rect(Rect::new(0, 0, 100, 40)),
            Some(Rect::new(10, 7, 80, 25))
        );
        assert_eq!(
            canvas_rect(Rect::new(0, 0, 80, 25)),
            Some(Rect::new(0, 0, 80, 25))
        );
        // An odd margin rounds towards the top-left.
        assert_eq!(
            canvas_rect(Rect::new(0, 0, 81, 26)),
            Some(Rect::new(0, 0, 80, 25))
        );
    }

    #[test]
    fn too_small_terminals_get_no_canvas() {
        assert_eq!(canvas_rect(Rect::new(0, 0, 79, 24)), None);
        assert_eq!(canvas_rect(Rect::new(0, 0, 79, 40)), None);
        assert_eq!(canvas_rect(Rect::new(0, 0, 200, 24)), None);
    }

    #[test]
    fn too_small_message_renders_centred() {
        let mut term = Terminal::new(TestBackend::new(79, 24)).unwrap();
        term.draw(|f| {
            let area = f.area();
            render_too_small(f.buffer_mut(), area, Style::new());
        })
        .unwrap();
        let buf = term.backend().buffer();
        let row = row_text(buf, buf.area, 12);
        assert_eq!(
            row.trim(),
            "ABRAXAS needs an 80x25 terminal. This one is 79x24."
        );
        // Centred: equal margins to within one column.
        let left = row.len() - row.trim_start().len();
        let right = row.len() - row.trim_end().len();
        assert!(left.abs_diff(right) <= 1, "{left} vs {right}");
    }

    #[test]
    fn fill_and_put_str_clip_to_the_buffer() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        let n = put_str(&mut buf, 6, 0, "abcdefgh", 10, Style::new());
        assert_eq!(n, 4);
        assert_eq!(row_text(&buf, buf.area, 0), "      abcd");
        fill(&mut buf, Rect::new(5, 0, 20, 5), Style::new());
        assert_eq!(row_text(&buf, buf.area, 0), "          ");
    }
}
