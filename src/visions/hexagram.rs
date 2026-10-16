//! The hexagram casting animation.

use std::time::Duration;

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::put;

const YANG: &str = "███████████";
const YIN: &str = "████   ████";
const LINE_W: u16 = 11;
const APPEAR_MS: u128 = 400;
const FLICKER_MS: u128 = 300;
const TOGGLE_MS: u128 = 60;
const TYPE_MS: u128 = 40;
/// Total rendered height: six lines, a blank row, the title.
pub const HEIGHT: u16 = 8;
const MIN_WIDTH: usize = 14;

/// `29 THE ABYSMAL`.
pub fn hexagram_title(number: u8) -> String {
    crate::oracle::title(number)
}

#[derive(Debug, Clone)]
pub struct HexagramCast {
    lines: [bool; 6],
    title: String,
    calm: bool,
    elapsed: Duration,
}

impl HexagramCast {
    pub fn new(number: u8, calm: bool) -> Self {
        let title = hexagram_title(number);
        let mut cast = Self {
            lines: crate::oracle::lines(number),
            title,
            calm,
            elapsed: Duration::ZERO,
        };
        if calm {
            cast.elapsed = Duration::from_millis(cast.done_ms() as u64);
        }
        cast
    }

    fn done_ms(&self) -> u128 {
        APPEAR_MS * 6 + FLICKER_MS + TYPE_MS * self.title.chars().count() as u128
    }

    pub fn tick(&mut self, dt: Duration) {
        if !self.calm {
            self.elapsed += dt;
        }
    }

    pub fn is_done(&self) -> bool {
        self.elapsed.as_millis() >= self.done_ms()
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }

    /// What line `i` (0 = bottom) shows now: `None` if not yet cast, else yang (true) / yin.
    fn line_state(&self, i: usize) -> Option<bool> {
        let ms = self.elapsed.as_millis();
        let appear = APPEAR_MS * (i as u128 + 1);
        if ms < appear {
            return None;
        }
        let truth = self.lines[i];
        let since = ms - appear;
        if since < FLICKER_MS && (since / TOGGLE_MS).is_multiple_of(2) {
            Some(!truth)
        } else {
            Some(truth)
        }
    }

    fn typed_chars(&self) -> usize {
        let start = APPEAR_MS * 6 + FLICKER_MS;
        let ms = self.elapsed.as_millis();
        if ms < start {
            0
        } else {
            (((ms - start) / TYPE_MS) as usize).min(self.title.chars().count())
        }
    }

    /// Draws the lines top first, a blank row, then the centred title.
    pub fn render(&self, area: Rect, buf: &mut Buffer, accent: Color) {
        let title_len = self.title.chars().count();
        let block_w = MIN_WIDTH.max(title_len);
        let line_off = (block_w - usize::from(LINE_W)) / 2;
        let title_off = (block_w - title_len) / 2;
        let bounds = buf.area;
        let at = |col: usize, row: u16| -> Option<(u16, u16)> {
            let x = area.x.checked_add(u16::try_from(col).ok()?)?;
            let y = area.y.checked_add(row)?;
            (col < usize::from(area.width) && row < area.height && bounds.contains((x, y).into()))
                .then_some((x, y))
        };
        // Clear the block first so flicker/typing never leaves stale glyphs.
        for row in 0..HEIGHT {
            for col in 0..block_w {
                if let Some((x, y)) = at(col, row) {
                    put(buf, x, y, ' ', accent);
                }
            }
        }
        for row in 0..6u16 {
            let i = 5 - usize::from(row);
            let Some(yang) = self.line_state(i) else {
                continue;
            };
            let glyphs = if yang { YANG } else { YIN };
            for (k, ch) in glyphs.chars().enumerate() {
                if let Some((x, y)) = at(line_off + k, row) {
                    put(buf, x, y, ch, accent);
                }
            }
        }
        for (k, ch) in self.title.chars().take(self.typed_chars()).enumerate() {
            if let Some((x, y)) = at(title_off + k, 7) {
                put(buf, x, y, ch, accent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visions::buffer_to_string;

    fn frame(cast: &HexagramCast) -> Vec<String> {
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);
        cast.render(area, &mut buf, Color::White);
        buffer_to_string(&buf, area)
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn at_ms(n: u8, ms: u64) -> HexagramCast {
        let mut c = HexagramCast::new(n, false);
        c.tick(Duration::from_millis(ms));
        c
    }

    fn visible(rows: &[String]) -> usize {
        rows[..6].iter().filter(|r| r.contains('█')).count()
    }

    #[test]
    fn lines_appear_bottom_to_top_every_400ms() {
        assert_eq!(visible(&frame(&at_ms(1, 399))), 0);
        let rows = frame(&at_ms(1, 400));
        assert_eq!(visible(&rows), 1);
        assert!(rows[5].contains('█'), "first line is the bottom one");
        assert_eq!(visible(&frame(&at_ms(1, 1000))), 2);
        assert_eq!(visible(&frame(&at_ms(1, 2400))), 6);
    }

    #[test]
    fn newest_line_flickers_for_300ms_in_60ms_steps_then_settles() {
        // Hexagram 2: bottom line is yin. During flicker, toggles between yang and yin.
        let shape = |ms| frame(&at_ms(2, ms))[5].clone();
        assert!(
            shape(400).contains("███████████"),
            "phase 0 shows the opposite"
        );
        assert!(shape(459).contains("███████████"));
        assert!(shape(460).contains("████   ████"), "phase 1 after 60 ms");
        assert!(shape(520).contains("███████████"));
        assert!(
            shape(699).contains("███████████"),
            "phase 4 still flickering"
        );
        assert!(shape(700).contains("████   ████"), "settled at 300 ms");
        assert!(shape(1000).contains("████   ████"));
    }

    #[test]
    fn title_types_out_after_lines_and_is_done_only_after() {
        let before = frame(&at_ms(29, 2700));
        assert!(before[7].trim().is_empty());
        let mid = frame(&at_ms(29, 2700 + 40 * 7));
        assert_eq!(mid[7].trim(), "29 THE");
        let full_ms = 2700 + 40 * 14;
        assert!(!at_ms(29, full_ms - 1).is_done());
        assert!(at_ms(29, full_ms).is_done());
        assert_eq!(frame(&at_ms(29, full_ms))[7].trim(), "29 THE ABYSMAL");
        assert!(at_ms(29, full_ms).elapsed() < 3.5);
    }

    #[test]
    fn calm_shows_everything_immediately_and_stays() {
        let mut c = HexagramCast::new(29, true);
        assert!(c.is_done());
        let a = frame(&c);
        c.tick(Duration::from_secs(5));
        assert_eq!(a, frame(&c));
        assert_eq!(visible(&a), 6);
        assert_eq!(a[7].trim(), "29 THE ABYSMAL");
    }

    #[test]
    fn render_clips_small_and_offset_areas() {
        let c = at_ms(63, 5000);
        for area in [
            Rect::new(0, 0, 0, 0),
            Rect::new(0, 0, 1, 1),
            Rect::new(15, 5, 20, 20),
        ] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 20, 8));
            c.render(area, &mut buf, Color::White);
        }
    }
}
