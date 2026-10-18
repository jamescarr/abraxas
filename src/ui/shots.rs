//! Test-only SVG encoder for rendered frames. The README screenshots are 80x25 buffers drawn by
//! `App::render` into a `TestBackend` (exactly what the tests look at) and written as SVG, so
//! they need no font rasteriser and stay crisp at any zoom.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::style::Color;

use crate::palette::Dos;

/// Cell size in SVG units (80x25 → 720x450).
pub(crate) const CELL_W: u32 = 9;
pub(crate) const CELL_H: u32 = 18;
/// Text baseline inside a cell and the matching font size.
const BASELINE: u32 = 14;
const FONT_SIZE: u32 = 15;

/// The colour of a cell as RGB: truecolor as is, an indexed colour through the DOS table, and
/// anything else (`Reset`) as `fallback`.
fn rgb(c: Color, fallback: Dos) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => Dos::ALL
            .iter()
            .copied()
            .find(|d| d.ansi_index() == i)
            .unwrap_or(fallback)
            .rgb(),
        _ => fallback.rgb(),
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `buf` as an SVG document: a black canvas, one `<rect>` per run of coloured background and one
/// `<text>` per run of equal colours. `textLength` pins every run to its cell width, so columns
/// line up in whatever monospace font the viewer has.
pub(crate) fn svg(buf: &Buffer) -> String {
    let area = buf.area;
    let (w, h) = (
        u32::from(area.width) * CELL_W,
        u32::from(area.height) * CELL_H,
    );
    let black = hex(Dos::Black.rgb());
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\" font-family=\"Menlo, Consolas, 'DejaVu Sans Mono', monospace\" \
         font-size=\"{FONT_SIZE}\" style=\"white-space:pre\">\n\
         <rect width=\"{w}\" height=\"{h}\" fill=\"{black}\"/>\n"
    );
    for y in 0..area.height {
        let mut x = 0u16;
        while x < area.width {
            let first = &buf[(area.x + x, area.y + y)];
            let fg = rgb(first.fg, Dos::LightGray);
            let bg = rgb(first.bg, Dos::Black);
            let start = x;
            let mut text = String::new();
            while x < area.width {
                let cell = &buf[(area.x + x, area.y + y)];
                if rgb(cell.fg, Dos::LightGray) != fg || rgb(cell.bg, Dos::Black) != bg {
                    break;
                }
                text.push_str(cell.symbol());
                x += 1;
            }
            let px = u32::from(start) * CELL_W;
            let py = u32::from(y) * CELL_H;
            let pw = u32::from(x - start) * CELL_W;
            let bg = hex(bg);
            if bg != black {
                out.push_str(&format!(
                    "<rect x=\"{px}\" y=\"{py}\" width=\"{pw}\" height=\"{CELL_H}\" fill=\"{bg}\"/>\n"
                ));
            }
            if text.chars().any(|c| c != ' ') {
                out.push_str(&format!(
                    "<text x=\"{px}\" y=\"{}\" fill=\"{}\" textLength=\"{pw}\" \
                     lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\">{}</text>\n",
                    py + BASELINE,
                    hex(fg),
                    escape(&text)
                ));
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

/// Write `buf` as `dir/<name>.svg`, creating `dir`.
///
/// # Panics
/// On any I/O error (this only runs from tests).
pub(crate) fn write(dir: &Path, name: &str, buf: &Buffer) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        panic!("creating {}: {e}", dir.display());
    }
    let path = dir.join(format!("{name}.svg"));
    if let Err(e) = std::fs::write(&path, svg(buf)) {
        panic!("writing {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Palette;
    use ratatui::layout::Rect;

    #[test]
    fn svg_paints_runs_and_escapes() {
        let p = Palette::new(true);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        buf[(0, 0)]
            .set_symbol("<")
            .set_style(p.style(Dos::LightRed, Dos::Black));
        buf[(1, 0)].set_symbol("a");
        buf[(2, 0)].set_symbol("b");
        buf[(3, 0)].set_bg(p.color(Dos::Blue));
        let out = svg(&buf);
        // The `<` run, the default-coloured `ab` run; nothing for the blank cell.
        assert_eq!(out.matches("<text").count(), 2, "{out}");
        assert!(out.contains(">&lt;</text>"), "{out}");
        assert!(
            out.contains("<text x=\"0\" y=\"14\" fill=\"#ff5555\" textLength=\"9\""),
            "{out}"
        );
        assert!(
            out.contains("<text x=\"9\" y=\"14\" fill=\"#aaaaaa\" textLength=\"18\""),
            "{out}"
        );
        // The canvas and the one blue cell.
        assert_eq!(out.matches("<rect").count(), 2, "{out}");
        assert!(
            out.contains("<rect x=\"27\" y=\"0\" width=\"9\" height=\"18\" fill=\"#0000aa\"/>"),
            "{out}"
        );
    }
}
