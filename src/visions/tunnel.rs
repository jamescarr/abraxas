//! Tunnel: a checkerboard receding down a polar well.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, put_shade};

pub(super) fn render(t: f32, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
    let cx = Cx::new(area);
    let half_w = (f32::from(area.width) / 2.0).max(1.0);
    cx.for_each(buf.area, |x, y, dx, dy_px| {
        let r = dx.hypot(dy_px) / half_w;
        let a = dy_px.atan2(dx);
        let u = 6.0 / r.max(0.05) + 3.0 * t;
        let v = 6.0 * a / std::f32::consts::PI + 0.5 * t;
        let checker = ((u.floor() as i64 + v.floor() as i64) & 1) as f32;
        let shade = r.min(1.0) * (0.35 + 0.65 * checker);
        put_shade(buf, x, y, shade, gradient);
    });
}
