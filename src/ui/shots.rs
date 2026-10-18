//! Test-only SVG encoder for rendered frames. The README screenshots are 80x25 buffers drawn by
//! `App::render` into a `TestBackend` (exactly what the tests look at) and written as SVG, so
//! they need no font rasteriser and stay crisp at any zoom. Text is emitted as `<text>` runs
//! pinned to their cell width; CP437 block elements are emitted as rectangles.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::style::Color;

use crate::palette::Dos;

/// Cell size in SVG units (80x25 → 720x450).
pub(crate) const CELL_W: f32 = 9.0;
pub(crate) const CELL_H: f32 = 18.0;
/// Text baseline inside a cell and the matching font size.
const BASELINE: f32 = 14.0;
const FONT_SIZE: u32 = 15;

/// A CP437 block element. These are drawn as rectangles, not glyphs: a font's block glyph is
/// only as tall as its ascent plus descent, which is less than the cell (about 15 of 18 px in
/// Consolas), so stacked half blocks (the logo, the menu banner, the hexagram, the bars) would
/// show horizontal seams in every viewer's font. `textLength` only fixes the width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Full,
    Upper,
    Lower,
    Left,
    Right,
    Light,
    Medium,
    Dark,
}

impl Block {
    fn of(symbol: &str) -> Option<Block> {
        Some(match symbol {
            "█" => Block::Full,
            "▀" => Block::Upper,
            "▄" => Block::Lower,
            "▌" => Block::Left,
            "▐" => Block::Right,
            "░" => Block::Light,
            "▒" => Block::Medium,
            "▓" => Block::Dark,
            _ => return None,
        })
    }

    /// The part of the cell painted, as fractions `(x, y, w, h)`.
    fn geometry(self) -> (f32, f32, f32, f32) {
        match self {
            Block::Full | Block::Light | Block::Medium | Block::Dark => (0.0, 0.0, 1.0, 1.0),
            Block::Upper => (0.0, 0.0, 1.0, 0.5),
            Block::Lower => (0.0, 0.5, 1.0, 0.5),
            Block::Left => (0.0, 0.0, 0.5, 1.0),
            Block::Right => (0.5, 0.0, 0.5, 1.0),
        }
    }

    /// Foreground opacity: the shades blend over the background.
    fn opacity(self) -> f32 {
        match self {
            Block::Light => 0.25,
            Block::Medium => 0.5,
            Block::Dark => 0.75,
            _ => 1.0,
        }
    }

    /// Adjacent cells of a full-width block merge into one rectangle.
    fn spans_cell_width(self) -> bool {
        !matches!(self, Block::Left | Block::Right)
    }
}

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

fn rect(out: &mut String, x: f32, y: f32, w: f32, h: f32, fill: &str, opacity: f32) {
    out.push_str(&format!(
        "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\""
    ));
    if opacity < 1.0 {
        out.push_str(&format!(" fill-opacity=\"{opacity}\""));
    }
    out.push_str("/>\n");
}

/// `buf` as an SVG document: a black canvas, one `<rect>` per run of coloured background, one
/// `<rect>` per run of equal block elements and one `<text>` per run of equal colours.
/// `textLength` pins every text run to its cell width, so columns line up in whatever
/// monospace font the viewer has.
pub(crate) fn svg(buf: &Buffer) -> String {
    let area = buf.area;
    let (w, h) = (
        f32::from(area.width) * CELL_W,
        f32::from(area.height) * CELL_H,
    );
    let black = hex(Dos::Black.rgb());
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\" font-family=\"Menlo, Consolas, 'DejaVu Sans Mono', monospace\" \
         font-size=\"{FONT_SIZE}\" style=\"white-space:pre\">\n"
    );
    rect(&mut out, 0.0, 0.0, w, h, &black, 1.0);
    for y in 0..area.height {
        let mut x = 0u16;
        while x < area.width {
            let first = &buf[(area.x + x, area.y + y)];
            let fg = rgb(first.fg, Dos::LightGray);
            let bg = rgb(first.bg, Dos::Black);
            let block = Block::of(first.symbol());
            let start = x;
            let mut text = String::new();
            while x < area.width {
                let cell = &buf[(area.x + x, area.y + y)];
                if rgb(cell.fg, Dos::LightGray) != fg
                    || rgb(cell.bg, Dos::Black) != bg
                    || Block::of(cell.symbol()) != block
                {
                    break;
                }
                text.push_str(cell.symbol());
                x += 1;
            }
            let px = f32::from(start) * CELL_W;
            let py = f32::from(y) * CELL_H;
            let pw = f32::from(x - start) * CELL_W;
            let bg = hex(bg);
            if bg != black {
                rect(&mut out, px, py, pw, CELL_H, &bg, 1.0);
            }
            let fg = hex(fg);
            match block {
                Some(b) if b.spans_cell_width() => {
                    let (_, by, _, bh) = b.geometry();
                    rect(
                        &mut out,
                        px,
                        py + by * CELL_H,
                        pw,
                        bh * CELL_H,
                        &fg,
                        b.opacity(),
                    );
                }
                Some(b) => {
                    let (bx, by, bw, bh) = b.geometry();
                    for cell in start..x {
                        let cx = f32::from(cell) * CELL_W + bx * CELL_W;
                        rect(
                            &mut out,
                            cx,
                            py + by * CELL_H,
                            bw * CELL_W,
                            bh * CELL_H,
                            &fg,
                            1.0,
                        );
                    }
                }
                None if text.chars().any(|c| c != ' ') => {
                    out.push_str(&format!(
                        "<text x=\"{px}\" y=\"{}\" fill=\"{fg}\" textLength=\"{pw}\" \
                         lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\">{}</text>\n",
                        py + BASELINE,
                        escape(&text)
                    ));
                }
                None => {}
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
    fn svg_paints_runs_blocks_and_escapes() {
        let p = Palette::new(true);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        buf[(0, 0)]
            .set_symbol("<")
            .set_style(p.style(Dos::LightRed, Dos::Black));
        buf[(1, 0)].set_symbol("a");
        buf[(2, 0)].set_symbol("b");
        buf[(3, 0)].set_bg(p.color(Dos::Blue));
        buf[(4, 0)].set_symbol("▀").set_fg(p.color(Dos::White));
        buf[(5, 0)].set_symbol("▀").set_fg(p.color(Dos::White));
        buf[(6, 0)].set_symbol("░").set_fg(p.color(Dos::Yellow));
        buf[(7, 0)].set_symbol("▐").set_fg(p.color(Dos::White));
        let out = svg(&buf);
        // The `<` run, the default-coloured `ab` run; nothing for the blank cell or the blocks.
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
        assert!(
            !out.contains(['▀', '░', '▐']),
            "blocks are rectangles:\n{out}"
        );
        // The canvas, the blue cell, the two merged upper halves, the shade, the right half.
        assert_eq!(out.matches("<rect").count(), 5, "{out}");
        assert!(
            out.contains("<rect x=\"27\" y=\"0\" width=\"9\" height=\"18\" fill=\"#0000aa\"/>"),
            "{out}"
        );
        assert!(
            out.contains("<rect x=\"36\" y=\"0\" width=\"18\" height=\"9\" fill=\"#ffffff\"/>"),
            "{out}"
        );
        assert!(
            out.contains(
                "<rect x=\"54\" y=\"0\" width=\"9\" height=\"18\" fill=\"#ffff55\" \
                 fill-opacity=\"0.25\"/>"
            ),
            "{out}"
        );
        assert!(
            out.contains("<rect x=\"67.5\" y=\"0\" width=\"4.5\" height=\"18\" fill=\"#ffffff\"/>"),
            "{out}"
        );
    }
}
