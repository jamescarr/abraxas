//! Ouroboros: a serpent eating its tail over a dim plasma.

use std::f32::consts::TAU;

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, grad_index, plasma, put};

const SAMPLES: usize = 64;
const BODY_FRACTION: f32 = 0.78;
const BODY: [char; 3] = ['▓', '▒', '░'];
const BG_RAMP: [char; 3] = [' ', '.', ':'];

/// Position (column, row) of body sample `i` (0 = head) at time `t`.
pub(super) fn sample_pos(area: Rect, i: usize, t: f32) -> (f32, f32) {
    let (w, h) = (f32::from(area.width), f32::from(area.height));
    let cx = f32::from(area.x) + (w / 2.0).floor();
    let cy = f32::from(area.y) + (h / 2.0).floor();
    let angle = 1.1 * t - (i as f32 / (SAMPLES - 1) as f32) * BODY_FRACTION * TAU;
    (cx + 0.36 * w * angle.cos(), cy + 0.40 * h * angle.sin())
}

pub(super) fn render(t: f32, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
    let cx = Cx::new(area);
    cx.for_each(buf.area, |x, y, dx, dy_px| {
        let lx = f32::from(x) - f32::from(area.x);
        let ly = f32::from(y) - f32::from(area.y);
        let s = plasma::shade(lx, ly, dx, dy_px, t) * 0.25;
        let ch = BG_RAMP[((s * 4.0 * 3.0) as usize).min(2)];
        put(buf, x, y, ch, gradient[grad_index(s)]);
    });
    // Tail first so the head overwrites on overlap.
    for i in (0..SAMPLES).rev() {
        let (px, py) = sample_pos(area, i, t);
        let (px, py) = (px.round(), py.round());
        if px < 0.0 || py < 0.0 || px > f32::from(u16::MAX) || py > f32::from(u16::MAX) {
            continue;
        }
        let (px, py) = (px as u16, py as u16);
        if !area.contains((px, py).into()) {
            continue;
        }
        let ch = if i == 0 {
            'Θ'
        } else if i == SAMPLES - 1 {
            '·'
        } else {
            BODY[(i / 2) % 3]
        };
        let stop = 5 - (i * 6 / SAMPLES).min(5);
        put(buf, px, py, ch, gradient[stop]);
    }
}
