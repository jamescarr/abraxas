//! BBS name banners and box frames.
//!
//! A name of [`MAX_BIG_CHARS`] characters or fewer is drawn with `tui_big_text` in half-height
//! pixels (4 rows of `▀▄█`), each row coloured from the act gradient. A longer name is drawn
//! letter-spaced in uppercase inside a `═` box. [`boxed`] draws the double- or single-line frames
//! many screens use.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use tui_big_text::{BigText, PixelSize};

use crate::ui::canvas::{fill, put_str};

/// Rows a banner reserves (big text uses all 4; the boxed form uses the first 3).
pub const HEIGHT: u16 = 4;
/// Longest name drawn as big text.
pub const MAX_BIG_CHARS: usize = 9;

/// Whether `name` is drawn as big text (else boxed).
pub fn is_big(name: &str) -> bool {
    name.chars().count() <= MAX_BIG_CHARS
}

/// The three rows of the boxed form of `name`: `╔═══╗`, `║ N A M E ║`, `╚═══╝`.
pub fn boxed_name_lines(name: &str) -> [String; 3] {
    let spaced = name
        .to_uppercase()
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join(" ");
    let inner = spaced.chars().count() + 2;
    [
        format!("╔{}╗", "═".repeat(inner)),
        format!("║ {spaced} ║"),
        format!("╚{}╝", "═".repeat(inner)),
    ]
}

/// Draw the banner for `name` centred in the top rows of `area`. Big text takes its row colours
/// from gradient stops 2..=5 (top row dimmest); the boxed form is drawn in `chrome`. Returns the
/// number of rows drawn (4 for big text, 3 boxed).
pub fn render_name(
    buf: &mut Buffer,
    area: Rect,
    name: &str,
    gradient: [Color; 6],
    chrome: Color,
    bg: Color,
) -> u16 {
    if area.height == 0 || area.width == 0 {
        return 0;
    }
    if is_big(name) {
        let rows = HEIGHT.min(area.height);
        let target = Rect::new(area.x, area.y, area.width, rows);
        fill(buf, target, Style::new().bg(bg));
        BigText::builder()
            .pixel_size(PixelSize::HalfHeight)
            .alignment(Alignment::Center)
            .lines(vec![Line::from(name.to_string())])
            .style(Style::new().fg(gradient[5]).bg(bg))
            .build()
            .render(target, buf);
        for r in 0..rows {
            let colour = gradient[usize::from(2 + r).min(5)];
            for x in target.left()..target.right() {
                if let Some(cell) = buf.cell_mut((x, target.y + r))
                    && cell.symbol() != " "
                {
                    cell.set_fg(colour);
                }
            }
        }
        rows
    } else {
        let lines = boxed_name_lines(name);
        let mut drawn = 0;
        for (i, line) in lines.iter().enumerate() {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            if y >= area.bottom() {
                break;
            }
            let w = u16::try_from(line.chars().count()).unwrap_or(area.width);
            let x = area.x + area.width.saturating_sub(w) / 2;
            put_str(buf, x, y, line, area.width, Style::new().fg(chrome).bg(bg));
            drawn += 1;
        }
        drawn
    }
}

/// Line style of a [`boxed`] frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    /// `╔═╗║╚╝`
    Double,
    /// `┌─┐│└┘`
    Single,
}

impl BoxKind {
    fn glyphs(self) -> [&'static str; 6] {
        match self {
            BoxKind::Double => ["╔", "╗", "╚", "╝", "═", "║"],
            BoxKind::Single => ["┌", "┐", "└", "┘", "─", "│"],
        }
    }
}

/// Draw a frame around `area` in `style`, with an optional title in the top border
/// (`╔═[ Title ]═══╗`). The inside is cleared with `style`'s background. Returns the inner area
/// (empty when `area` is smaller than 3x3).
pub fn boxed(
    buf: &mut Buffer,
    area: Rect,
    title: Option<&str>,
    kind: BoxKind,
    style: Style,
) -> Rect {
    if area.width < 2 || area.height < 2 {
        return Rect::default();
    }
    let [tl, tr, bl, br, h, v] = kind.glyphs();
    fill(buf, area, style);
    let (l, r, t, b) = (area.left(), area.right() - 1, area.top(), area.bottom() - 1);
    for x in l + 1..r {
        put_str(buf, x, t, h, 1, style);
        put_str(buf, x, b, h, 1, style);
    }
    for y in t + 1..b {
        put_str(buf, l, y, v, 1, style);
        put_str(buf, r, y, v, 1, style);
    }
    put_str(buf, l, t, tl, 1, style);
    put_str(buf, r, t, tr, 1, style);
    put_str(buf, l, b, bl, 1, style);
    put_str(buf, r, b, br, 1, style);
    if let Some(title) = title {
        let text = format!("{h}[ {title} ]");
        put_str(buf, l + 1, t, &text, area.width.saturating_sub(2), style);
    }
    Rect::new(l + 1, t + 1, area.width - 2, area.height.saturating_sub(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::row_text;

    const GRAD: [Color; 6] = [
        Color::Black,
        Color::Blue,
        Color::Red,
        Color::LightRed,
        Color::Yellow,
        Color::White,
    ];

    fn non_blank_rows(buf: &Buffer, area: Rect) -> Vec<u16> {
        (area.top()..area.bottom())
            .filter(|y| !row_text(buf, area, *y).trim().is_empty())
            .collect()
    }

    #[test]
    fn nine_characters_or_fewer_draw_four_rows_of_big_text() {
        for name in ["ABRAXAS", "SOL NIGER"] {
            let area = Rect::new(0, 0, 80, 6);
            let mut buf = Buffer::empty(area);
            let rows = render_name(&mut buf, area, name, GRAD, Color::Red, Color::Black);
            assert_eq!(rows, 4, "{name}");
            assert_eq!(non_blank_rows(&buf, area), vec![0, 1, 2, 3], "{name}");
            // Only half-height glyphs.
            for y in 0..4 {
                for c in row_text(&buf, area, y).chars() {
                    assert!(" ▀▄█".contains(c), "{name}: {c:?}");
                }
            }
        }
    }

    #[test]
    fn big_text_rows_take_their_colour_from_the_gradient() {
        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        render_name(&mut buf, area, "ABRAXAS", GRAD, Color::Red, Color::Black);
        for (y, want) in [
            (0, Color::Red),
            (1, Color::LightRed),
            (2, Color::Yellow),
            (3, Color::White),
        ] {
            let lit: Vec<Color> = (0..80)
                .filter(|x| buf[(*x, y)].symbol() != " ")
                .map(|x| buf[(x, y)].fg)
                .collect();
            assert!(!lit.is_empty());
            assert!(lit.iter().all(|c| *c == want), "row {y}");
        }
    }

    #[test]
    fn longer_names_are_letter_spaced_uppercase_in_a_box() {
        assert!(!is_big("OUROBOROS DATA"));
        let lines = boxed_name_lines("Night Shift");
        assert_eq!(lines[1], "║ N I G H T   S H I F T ║");
        assert_eq!(lines[0].chars().count(), lines[1].chars().count());
        assert!(lines[0].starts_with("╔═") && lines[0].ends_with("═╗"));
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        let rows = render_name(
            &mut buf,
            area,
            "NIGHT SHIFT BBS",
            GRAD,
            Color::Red,
            Color::Black,
        );
        assert_eq!(rows, 3);
        assert_eq!(non_blank_rows(&buf, area), vec![0, 1, 2]);
        let mid = row_text(&buf, area, 1);
        assert!(mid.contains("║ N I G H T   S H I F T   B B S ║"), "{mid:?}");
        assert_eq!(buf[(40, 1)].fg, Color::Red);
        // A 24-character name still fits.
        let widest = boxed_name_lines(&"W".repeat(24));
        assert!(widest[0].chars().count() <= 80);
    }

    #[test]
    fn boxed_frames_draw_corners_title_and_return_the_inner_area() {
        let area = Rect::new(1, 1, 20, 5);
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        let inner = boxed(&mut buf, area, Some("Help"), BoxKind::Double, Style::new());
        assert_eq!(inner, Rect::new(2, 2, 18, 3));
        let full = Rect::new(0, 0, 24, 8);
        assert_eq!(row_text(&buf, full, 1).trim_end(), " ╔═[ Help ]═════════╗");
        assert_eq!(row_text(&buf, full, 3).trim_end(), " ║                  ║");
        assert_eq!(row_text(&buf, full, 5).trim_end(), " ╚══════════════════╝");
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        let inner = boxed(
            &mut buf,
            Rect::new(0, 0, 8, 3),
            None,
            BoxKind::Single,
            Style::new(),
        );
        assert_eq!(inner, Rect::new(1, 1, 6, 1));
        assert_eq!(row_text(&buf, Rect::new(0, 0, 8, 3), 0), "┌──────┐");
        assert_eq!(
            boxed(
                &mut buf,
                Rect::new(0, 0, 1, 1),
                None,
                BoxKind::Single,
                Style::new()
            ),
            Rect::default()
        );
    }
}
