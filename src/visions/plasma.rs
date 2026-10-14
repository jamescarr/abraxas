//! Plasma: four summed sine waves.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, put_shade};

/// Plasma shade in 0..=1 at local cell `(x, y)` with offsets from the centre.
pub(super) fn shade(x: f32, y: f32, dx: f32, dy_px: f32, t: f32) -> f32 {
    let v = (0.16 * x + t).sin()
        + (0.32 * y + 1.3 * t).sin()
        + (0.09 * (x + 2.0 * y) + 0.7 * t).sin()
        + (0.15 * dx.hypot(dy_px) - 1.6 * t).sin();
    ((v + 4.0) / 8.0).clamp(0.0, 1.0)
}

pub(super) fn render(t: f32, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
    let cx = Cx::new(area);
    cx.for_each(buf.area, |x, y, dx, dy_px| {
        let lx = f32::from(x) - f32::from(area.x);
        let ly = f32::from(y) - f32::from(area.y);
        put_shade(buf, x, y, shade(lx, ly, dx, dy_px, t), gradient);
    });
}
