//! `Transmission`: text that arrives at modem speed.
//!
//! At `baud` B text reveals at B/10 characters per second; baud 0 reveals everything at once.
//! Any key press completes the current transmission ([`Transmission::skip`]). Text may keep
//! arriving while it is being typed (`push_str`, for streamed AI output) and the transmission is
//! only [`done`](Transmission::is_done) once the input is closed as well as fully revealed.
//!
//! Wrapping happens on the full text, so words never jump lines while typing. Progress is
//! counted over the wrapped characters, with one character per line break.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::text::wrap;
use crate::ui::canvas::put_str;

const NANOS: u128 = 1_000_000_000;

#[derive(Debug, Clone)]
pub struct Transmission {
    text: String,
    /// Characters revealed so far, counted over wrapped output (line break = 1).
    shown: usize,
    /// Sub-character progress carried between ticks, in nanosecond-characters.
    carry: u128,
    /// Characters per second; `None` = instant.
    cps: Option<u64>,
    /// No more text will be pushed.
    closed: bool,
}

impl Transmission {
    /// A complete text to type out at `baud` (0 = instant).
    pub fn new(text: impl Into<String>, baud: u32) -> Self {
        let mut t = Self::streaming(baud);
        t.text = text.into();
        t.closed = true;
        t
    }

    /// An open transmission: feed it with [`push_str`](Self::push_str), then
    /// [`close`](Self::close) it.
    pub fn streaming(baud: u32) -> Self {
        Self {
            text: String::new(),
            shown: 0,
            carry: 0,
            cps: (baud > 0).then(|| u64::from((baud / 10).max(1))),
            closed: false,
        }
    }

    /// Append received text. Ignored once closed.
    pub fn push_str(&mut self, s: &str) {
        if !self.closed {
            self.text.push_str(s);
        }
    }

    /// Replace the whole text with `text` (e.g. the final text of a streamed reply) and close the
    /// input. Characters already revealed stay revealed.
    pub fn set_final(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.closed = true;
    }

    /// No more text will arrive.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Let `dt` pass, revealing characters.
    pub fn tick(&mut self, dt: Duration) {
        match self.cps {
            None => self.shown = usize::MAX,
            Some(cps) => {
                self.carry += dt.as_nanos() * u128::from(cps);
                let whole = self.carry / NANOS;
                self.carry %= NANOS;
                self.shown = self
                    .shown
                    .saturating_add(usize::try_from(whole).unwrap_or(usize::MAX));
            }
        }
    }

    /// Reveal everything received so far (any key does this).
    pub fn skip(&mut self) {
        self.shown = usize::MAX;
        self.carry = 0;
    }

    /// Everything revealed *and* the input closed.
    pub fn is_done(&self) -> bool {
        self.closed && self.is_caught_up()
    }

    /// Everything received so far is revealed (more may still arrive).
    pub fn is_caught_up(&self) -> bool {
        self.cps.is_none() || self.shown >= self.total_chars(usize::MAX)
    }

    /// The full text received so far.
    pub fn full_text(&self) -> &str {
        &self.text
    }

    /// Characters revealed so far (for the `RX n bytes` counter), at most the received length.
    pub fn shown_chars(&self) -> usize {
        self.shown.min(self.total_chars(usize::MAX))
    }

    fn total_chars(&self, width: usize) -> usize {
        let lines = wrap(&self.text, width);
        lines.iter().map(|l| l.chars().count()).sum::<usize>() + lines.len().saturating_sub(1)
    }

    /// The revealed part of the text, wrapped to `width` columns.
    pub fn visible_lines(&self, width: u16) -> Vec<String> {
        let mut left = self.shown;
        let mut out = Vec::new();
        for line in wrap(&self.text, usize::from(width)) {
            let n = line.chars().count();
            if left >= n {
                out.push(line);
                left -= n;
                if left == 0 {
                    break;
                }
                // The line break itself costs one character.
                left -= 1;
            } else {
                if left > 0 || out.is_empty() {
                    out.push(line.chars().take(left).collect());
                }
                break;
            }
        }
        out
    }

    /// Draw the revealed text into `area` (wrapped to its width). When it needs more rows than
    /// the area has, the last rows are shown (the text scrolls like a terminal).
    pub fn render(&self, buf: &mut Buffer, area: Rect, style: Style) {
        if area.is_empty() {
            return;
        }
        let lines = self.visible_lines(area.width);
        let skip = lines.len().saturating_sub(usize::from(area.height));
        for (i, line) in lines.iter().skip(skip).enumerate() {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            put_str(buf, area.x, y, line, area.width, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn baud_zero_is_instant() {
        let mut t = Transmission::new("hello world", 0);
        assert!(t.is_done());
        t.tick(Duration::ZERO);
        assert_eq!(t.visible_lines(80), vec!["hello world"]);
    }

    #[test]
    fn nine_six_hundred_baud_types_960_chars_per_second() {
        let text = "x".repeat(2000);
        let mut t = Transmission::new(text, 9600);
        t.tick(secs(0.5));
        assert_eq!(t.shown_chars(), 480);
        t.tick(secs(0.25));
        assert_eq!(t.shown_chars(), 720);
        assert!(!t.is_done());
        t.tick(secs(2.0));
        assert!(t.is_done());
        assert_eq!(t.shown_chars(), 2000);
    }

    #[test]
    fn fractional_characters_carry_across_ticks() {
        // 300 baud = 30 chars/s; 33 ms frames give 0.99 chars each.
        let mut t = Transmission::new("a".repeat(100), 300);
        for _ in 0..100 {
            t.tick(Duration::from_millis(33));
        }
        // 3.3 s * 30 = 99 characters (not 0 and not 100).
        assert_eq!(t.shown_chars(), 99);
    }

    #[test]
    fn any_key_completes_the_transmission() {
        let mut t = Transmission::new("slow text arriving", 300);
        t.tick(secs(0.1));
        assert!(!t.is_done());
        t.skip();
        assert!(t.is_done());
        assert_eq!(t.visible_lines(80), vec!["slow text arriving"]);
    }

    #[test]
    fn streaming_is_not_done_until_closed() {
        let mut t = Transmission::streaming(0);
        t.push_str("first ");
        t.tick(Duration::ZERO);
        assert_eq!(t.visible_lines(80), vec!["first"]);
        assert!(!t.is_done());
        t.push_str("second");
        t.close();
        t.tick(Duration::ZERO);
        assert_eq!(t.visible_lines(80), vec!["first second"]);
        assert!(t.is_done());
        t.push_str(" ignored");
        assert_eq!(t.full_text(), "first second");
    }

    #[test]
    fn skip_on_a_stream_also_reveals_text_that_arrives_later() {
        let mut t = Transmission::streaming(100);
        t.push_str("abc");
        t.skip();
        assert!(t.is_caught_up());
        t.push_str("def");
        // Skip is sticky: the rest of the stream shows as it arrives.
        assert_eq!(t.visible_lines(80), vec!["abcdef"]);
    }

    #[test]
    fn wrapping_is_stable_while_typing() {
        let mut t = Transmission::new("aaaa bbbb cccc", 100); // 10 cps
        t.tick(secs(0.7));
        // The full text wraps at width 9 as "aaaa bbbb" / "cccc"; partial text never reflows.
        assert_eq!(t.visible_lines(9), vec!["aaaa bb"]);
        t.tick(secs(0.3));
        assert_eq!(t.visible_lines(9), vec!["aaaa bbbb"]);
        t.tick(secs(0.1));
        assert_eq!(t.visible_lines(9), vec!["aaaa bbbb", "c"]);
        t.skip();
        assert_eq!(t.visible_lines(9), vec!["aaaa bbbb", "cccc"]);
    }

    #[test]
    fn render_scrolls_to_the_newest_rows() {
        let mut t = Transmission::new("one\ntwo\nthree", 0);
        t.tick(Duration::ZERO);
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        t.render(&mut buf, area, Style::new());
        assert_eq!(crate::ui::canvas::row_text(&buf, area, 0).trim(), "two");
        assert_eq!(crate::ui::canvas::row_text(&buf, area, 1).trim(), "three");
    }
}
