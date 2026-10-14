//! Mirror: the last canvas frame, reflected, shivering row by row.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, mix, noise, plasma, put_shade};

/// Seconds between glitch rows and how long each lasts.
const GLITCH_PERIOD: f32 = 0.5;
const GLITCH_LEN: f32 = 0.1;

/// Horizontal shift of row `y` (local row index) at time `t`.
pub(super) fn row_shift(y: u16, t: f32) -> i32 {
    (2.0 * (0.4 * f32::from(y) + 3.0 * t).sin()).round() as i32
}

/// The local row replaced by noise at `t`, if a glitch window is active.
pub(super) fn glitch_row(t: f32, seed: u64, height: u16) -> Option<u16> {
    if height == 0 {
        return None;
    }
    let k = (t / GLITCH_PERIOD).floor().max(0.0) as u64;
    if t - k as f32 * GLITCH_PERIOD < GLITCH_LEN {
        Some((mix(seed, k, 0, 0x6c17) % u64::from(height)) as u16)
    } else {
        None
    }
}

pub(super) fn render(
    t: f32,
    seed: u64,
    calm: bool,
    gradient: &[Color; 6],
    snapshot: Option<&Buffer>,
    area: Rect,
    buf: &mut Buffer,
) {
    let glitch = if calm {
        None
    } else {
        glitch_row(t, seed, area.height)
    };
    let cx = Cx::new(area);
    let nkey = mix(seed, (t * 20.0) as u64, 1, 0);
    cx.for_each(buf.area, |x, y, dx, dy_px| {
        let (lx, ly) = (x - area.x, y - area.y);
        if glitch == Some(ly) {
            match noise::cell(nkey, x, y, 0.6) {
                Some((ch, c)) => super::put(buf, x, y, ch, c),
                None => super::put(buf, x, y, ' ', Color::DarkGray),
            }
            return;
        }
        let shift = row_shift(ly, t);
        if let Some(snap) = snapshot {
            let sw = i32::from(snap.area.width);
            let src = if sw > 0 && i32::from(ly) < i32::from(snap.area.height) {
                let sx = (sw - 1 - i32::from(lx) - shift).rem_euclid(sw);
                snap.cell((snap.area.x + sx as u16, snap.area.y + ly))
                    .cloned()
            } else {
                None
            };
            if let Some(cell) = src {
                if let Some(dst) = buf.cell_mut((x, y)) {
                    *dst = cell;
                }
                return;
            }
            super::put(buf, x, y, ' ', gradient[0]);
        } else {
            // Mirrored plasma-like fallback.
            let w = i32::from(area.width);
            let mx = (w - 1 - i32::from(lx) - shift).rem_euclid(w.max(1)) as f32;
            let s = plasma::shade(mx, f32::from(ly), dx, dy_px, t);
            put_shade(buf, x, y, s, gradient);
        }
    });
}
