//! Horizontal progress bars: `████░░░░ 41%`. Used by ZMODEM, the trace header and EGO bars.

use ratatui::buffer::Buffer;
use ratatui::style::Style;

use crate::ui::canvas::put_str;

/// A bar of exactly `width` cells (`█` filled, `░` empty) for `frac` in 0..=1 (clamped; NaN is 0).
pub fn bar(frac: f32, width: usize) -> String {
    let frac = if frac.is_nan() {
        0.0
    } else {
        frac.clamp(0.0, 1.0)
    };
    let filled = ((frac * width as f32).round() as usize).min(width);
    let mut s = "█".repeat(filled);
    s.push_str(&"░".repeat(width - filled));
    s
}

/// `41%` for 0.41 (rounded, clamped to 0..=100).
pub fn percent_label(frac: f32) -> String {
    let frac = if frac.is_nan() {
        0.0
    } else {
        frac.clamp(0.0, 1.0)
    };
    format!("{}%", (frac * 100.0).round() as u32)
}

/// Styles of a drawn bar.
#[derive(Debug, Clone, Copy)]
pub struct BarStyle {
    pub filled: Style,
    pub empty: Style,
}

/// Draw a bar of `bar_width` cells at `(x, y)` (absolute), filled cells in `filled`, empty cells
/// in `empty`, followed by ` nn%` in `filled` style when `show_percent`. Returns the columns used.
pub fn render(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    bar_width: u16,
    frac: f32,
    style: BarStyle,
    show_percent: bool,
) -> u16 {
    let BarStyle { filled, empty } = style;
    let b = bar(frac, usize::from(bar_width));
    let n_filled = u16::try_from(b.chars().filter(|c| *c == '█').count()).unwrap_or(0);
    let mut used = put_str(
        buf,
        x,
        y,
        &b.chars().take(usize::from(n_filled)).collect::<String>(),
        bar_width,
        filled,
    );
    used += put_str(
        buf,
        x + used,
        y,
        &b.chars().skip(usize::from(n_filled)).collect::<String>(),
        bar_width - n_filled,
        empty,
    );
    if show_percent {
        used += put_str(
            buf,
            x + used,
            y,
            &format!(" {}", percent_label(frac)),
            5,
            filled,
        );
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::row_text;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    #[test]
    fn bar_rounds_and_clamps() {
        assert_eq!(bar(0.0, 8), "░░░░░░░░");
        assert_eq!(bar(1.0, 8), "████████");
        assert_eq!(bar(0.41, 10), "████░░░░░░");
        assert_eq!(bar(2.0, 4), "████");
        assert_eq!(bar(-1.0, 4), "░░░░");
        assert_eq!(bar(f32::NAN, 4), "░░░░");
        assert_eq!(bar(0.5, 0), "");
    }

    #[test]
    fn percent_label_rounds() {
        assert_eq!(percent_label(0.414), "41%");
        assert_eq!(percent_label(0.416), "42%");
        assert_eq!(percent_label(3.0), "100%");
    }

    #[test]
    fn render_styles_filled_and_empty_cells_and_appends_percent() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let filled = Style::new().fg(Color::Green);
        let empty = Style::new().fg(Color::DarkGray);
        let style = BarStyle { filled, empty };
        let used = render(&mut buf, 1, 0, 10, 0.5, style, true);
        assert_eq!(used, 14);
        assert_eq!(row_text(&buf, area, 0), " █████░░░░░ 50%     ");
        assert_eq!(buf[(1, 0)].fg, Color::Green);
        assert_eq!(buf[(6, 0)].fg, Color::DarkGray);
        let used = render(&mut buf, 0, 0, 4, 1.0, style, false);
        assert_eq!(used, 4);
    }
}
