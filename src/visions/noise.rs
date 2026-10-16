//! Noise: line noise confetti.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{mix, put};

const CHARS: &[u8] = b"}{~^`'\"<>[]#$%&*";
const COLORS: [Color; 4] = [Color::DarkGray, Color::Gray, Color::Cyan, Color::LightCyan];
/// Noise frames per second.
const RATE: f32 = 20.0;

/// Colours noise may use.
#[cfg(test)]
pub(super) fn colors() -> [Color; 4] {
    COLORS
}

/// The glyph and colour of the noise at `(x, y)` for hash key `key`, or `None` when empty.
pub(super) fn cell(key: u64, x: u16, y: u16, density: f32) -> Option<(char, Color)> {
    let h = mix(key, u64::from(x), u64::from(y), 0x9e37);
    let u = (h >> 40) as f32 / (1u64 << 24) as f32;
    if u >= density {
        return None;
    }
    let ch = char::from(CHARS[(h as usize & 0xffff) % CHARS.len()]);
    Some((ch, COLORS[((h >> 16) as usize & 0xff) % COLORS.len()]))
}

pub(super) fn render(t: f32, seed: u64, density: f32, area: Rect, buf: &mut Buffer) {
    let frame = (t * RATE).floor().max(0.0) as u64;
    let key = mix(seed, frame, 0, 0);
    let clip = area.intersection(buf.area);
    for y in clip.top()..clip.bottom() {
        for x in clip.left()..clip.right() {
            match cell(key, x, y, density) {
                Some((ch, c)) => put(buf, x, y, ch, c),
                None => put(buf, x, y, ' ', Color::DarkGray),
            }
        }
    }
}
