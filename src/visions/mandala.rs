//! Mandala: eight-fold polar petals with rings and a sun at the centre.

use std::f32::consts::{PI, TAU};

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, grad_index, put, put_shade};

pub(super) fn render(t: f32, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
    let cx = Cx::new(area);
    let h = f32::from(area.height).max(1.0);
    cx.for_each(buf.area, |x, y, dx, dy_px| {
        let r = dx.hypot(dy_px) / h;
        let theta = dy_px.atan2(dx);
        let sector = TAU / 8.0;
        let tf = ((theta + 0.25 * t).rem_euclid(sector) - PI / 8.0).abs();
        let petals = (10.0 * r - 2.0 * t).sin() * (16.0 * tf).cos();
        let shade = (0.5 + 0.5 * petals) * (1.0 - 0.8 * r).max(0.15);
        if r < 0.06 {
            put(buf, x, y, '☼', gradient[5]);
        } else if (24.0 * r).sin().abs() < 0.12 {
            put(buf, x, y, '·', gradient[grad_index(shade).max(2)]);
        } else {
            put_shade(buf, x, y, shade, gradient);
        }
    });
}
