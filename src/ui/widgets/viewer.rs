//! A scrollable text viewer with a `--More--` footer, for message bodies, text files and help.
//!
//! Content is a list of styled lines. [`Viewer::from_text`] accepts pipe-coded text (see
//! `text::parse_pipe`) and word-wraps it to the viewer width while keeping colours.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::palette::{Dos, Palette};
use crate::ui::canvas::put_str;
use crate::ui::keys::{Key, classify};

#[derive(Debug, Clone)]
pub struct Viewer {
    lines: Vec<Line<'static>>,
    /// Content rows per page (the area height minus the footer row).
    rows: usize,
    top: usize,
}

/// Word-wrap one styled line to `width` columns (greedy, breaking at spaces; over-long words are
/// split).
fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            let st = line.style.patch(s.style);
            s.content.chars().map(move |c| (c, st))
        })
        .collect();
    if width == 0 || cells.len() <= width {
        return vec![line.clone()];
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut start = 0;
    while cells.len() - start > width {
        let window = &cells[start..=start + width];
        // Break after the last space inside the window; else split the word.
        let brk = window
            .iter()
            .rposition(|(c, _)| *c == ' ')
            .filter(|&i| i > 0);
        let (end, next) = match brk {
            Some(i) => (start + i, start + i + 1),
            None => (start + width, start + width),
        };
        out.push(cells_to_line(&cells[start..end]));
        start = next;
    }
    out.push(cells_to_line(&cells[start..]));
    out
}

fn cells_to_line(cells: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<ratatui::text::Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = cells.first().map(|c| c.1).unwrap_or_default();
    for (c, st) in cells {
        if *st != run_style && !run.is_empty() {
            spans.push(ratatui::text::Span::styled(
                std::mem::take(&mut run),
                run_style,
            ));
        }
        run_style = *st;
        run.push(*c);
    }
    if !run.is_empty() {
        spans.push(ratatui::text::Span::styled(run, run_style));
    }
    Line::from(spans)
}

impl Viewer {
    /// A viewer over ready-made lines; `rows` is the number of content rows per page.
    pub fn new(lines: Vec<Line<'static>>, rows: usize) -> Self {
        Self {
            lines,
            rows: rows.max(1),
            top: 0,
        }
    }

    /// Pipe-coded `text`, wrapped to `width` columns, shown `rows` rows at a time.
    pub fn from_text(text: &str, width: u16, rows: usize, palette: &Palette) -> Self {
        let lines = palette
            .pipe_lines(text)
            .iter()
            .flat_map(|l| wrap_line(l, usize::from(width)))
            .collect();
        Self::new(lines, rows)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Index of the first visible line.
    pub fn top(&self) -> usize {
        self.top
    }

    fn max_top(&self) -> usize {
        self.lines.len().saturating_sub(self.rows)
    }

    pub fn at_end(&self) -> bool {
        self.top >= self.max_top()
    }

    /// Percent of the text scrolled into view (the bottom visible line over the total), 100 at
    /// the end.
    pub fn percent(&self) -> u32 {
        if self.lines.is_empty() {
            return 100;
        }
        let seen = (self.top + self.rows).min(self.lines.len());
        u32::try_from(seen * 100 / self.lines.len()).unwrap_or(100)
    }

    /// Handle ↑/↓, PgUp/PgDn, Space, Home and End. Returns whether the key was a scroll key.
    pub fn handle_key(&mut self, ev: &KeyEvent) -> bool {
        let max = self.max_top();
        match classify(ev) {
            Key::Up => self.top = self.top.saturating_sub(1),
            Key::Down => self.top = (self.top + 1).min(max),
            Key::PgUp => self.top = self.top.saturating_sub(self.rows),
            Key::PgDn | Key::Char(' ') => self.top = (self.top + self.rows).min(max),
            Key::Home => self.top = 0,
            Key::End => self.top = max,
            _ => return false,
        }
        true
    }

    /// Draw the visible lines into `area` (rows 0..rows) and the footer on the row after them:
    /// `--More-- (nn%)` while there is more, `-- End --` once the last line is visible.
    pub fn render(&self, buf: &mut Buffer, area: Rect, palette: &Palette) {
        if area.is_empty() {
            return;
        }
        let rows = self.rows.min(usize::from(area.height.saturating_sub(1)));
        for (i, line) in self.lines.iter().skip(self.top).take(rows).enumerate() {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            let mut x = area.x;
            for span in &line.spans {
                let room = area.right().saturating_sub(x);
                x += put_str(buf, x, y, &span.content, room, span.style);
            }
        }
        let footer = if self.at_end() {
            "-- End --  (Esc to leave)".to_string()
        } else {
            format!(
                "--More-- ({}%)  Space/PgDn next, Esc to leave",
                self.percent()
            )
        };
        let fy = area.y + u16::try_from(rows).unwrap_or(0);
        put_str(
            buf,
            area.x,
            fy,
            &footer,
            area.width,
            palette.style(Dos::Black, Dos::LightGray),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::canvas::row_text;
    use crate::ui::keys::{press, press_char};
    use ratatui::crossterm::event::KeyCode;

    fn viewer(n: usize, rows: usize) -> Viewer {
        let text: String = (0..n).map(|i| format!("line {i}\n")).collect();
        Viewer::from_text(&text, 40, rows, &Palette::new(false))
    }

    #[test]
    fn paging_clamps_at_both_ends() {
        let mut v = viewer(25, 10);
        assert_eq!(v.line_count(), 25);
        assert!(v.handle_key(&press(KeyCode::PageDown)));
        assert_eq!(v.top(), 10);
        assert!(v.handle_key(&press_char(' ')));
        assert_eq!(v.top(), 15);
        assert!(v.at_end());
        v.handle_key(&press(KeyCode::PageDown));
        assert_eq!(v.top(), 15);
        v.handle_key(&press(KeyCode::Up));
        assert_eq!(v.top(), 14);
        v.handle_key(&press(KeyCode::PageUp));
        assert_eq!(v.top(), 4);
        v.handle_key(&press(KeyCode::PageUp));
        assert_eq!(v.top(), 0);
        assert!(!v.handle_key(&press_char('q')));
    }

    #[test]
    fn percent_counts_the_bottom_visible_line() {
        let mut v = viewer(20, 10);
        assert_eq!(v.percent(), 50);
        v.handle_key(&press(KeyCode::End));
        assert_eq!(v.percent(), 100);
        assert_eq!(viewer(3, 10).percent(), 100);
    }

    #[test]
    fn footer_says_more_until_the_end() {
        let mut v = viewer(30, 5);
        let area = Rect::new(0, 0, 60, 6);
        let mut buf = Buffer::empty(area);
        v.render(&mut buf, area, &Palette::new(false));
        assert!(row_text(&buf, area, 0).starts_with("line 0"));
        assert!(row_text(&buf, area, 4).starts_with("line 4"));
        assert!(row_text(&buf, area, 5).starts_with("--More-- (16%)"));
        v.handle_key(&press(KeyCode::End));
        let mut buf = Buffer::empty(area);
        v.render(&mut buf, area, &Palette::new(false));
        assert!(row_text(&buf, area, 0).starts_with("line 25"));
        assert!(row_text(&buf, area, 5).starts_with("-- End --"));
    }

    #[test]
    fn wrapping_keeps_colours_across_the_break() {
        let p = Palette::new(true);
        let v = Viewer::from_text("|12red words |10then green words here", 12, 10, &p);
        let texts: Vec<String> = v
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["red words", "then green", "words here"]);
        let red = p.style(Dos::LightRed, Dos::Black);
        let green = p.style(Dos::LightGreen, Dos::Black);
        assert_eq!(v.lines[0].spans[0].style, red);
        assert_eq!(v.lines[1].spans[0].style, green);
        assert_eq!(v.lines[2].spans[0].style, green);
    }

    #[test]
    fn overlong_words_are_split() {
        let v = Viewer::from_text("abcdefghij", 4, 10, &Palette::new(false));
        let texts: Vec<String> = v
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["abcd", "efgh", "ij"]);
    }
}
