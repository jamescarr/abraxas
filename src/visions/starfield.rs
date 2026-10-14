//! Starfield: 160 stars streaming toward the viewer.

use rand::RngExt;
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{Cx, grad_index, put};
use crate::rng::Rng;

const STARS: usize = 160;
const SPEED: f32 = 0.45;

#[derive(Debug, Clone, Copy)]
pub(super) struct Star {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone)]
pub(super) struct StarState {
    stars: Vec<Star>,
    rng: Rng,
}

impl StarState {
    pub(super) fn new(mut rng: Rng) -> Self {
        let stars = (0..STARS)
            .map(|_| Star {
                x: rng.random_range(-1.0..=1.0f32),
                y: rng.random_range(-1.0..=1.0f32),
                // (0, 1]
                z: 1.0 - rng.random_range(0.0..1.0f32),
            })
            .collect();
        Self { stars, rng }
    }

    pub(super) fn tick(&mut self, dt: f32) {
        for s in &mut self.stars {
            s.z -= SPEED * dt;
            if s.z <= 0.02 {
                s.x = self.rng.random_range(-1.0..=1.0f32);
                s.y = self.rng.random_range(-1.0..=1.0f32);
                s.z = 1.0;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn stars(&self) -> &[Star] {
        &self.stars
    }

    #[cfg(test)]
    pub(super) fn stars_mut(&mut self) -> &mut [Star] {
        &mut self.stars
    }

    pub(super) fn render(&self, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
        let cx = Cx::new(area);
        cx.for_each(buf.area, |x, y, _, _| put(buf, x, y, ' ', gradient[0]));
        let (w, h) = (f32::from(area.width), f32::from(area.height));
        let (ox, oy) = (
            f32::from(area.x) + (w / 2.0).floor(),
            f32::from(area.y) + (h / 2.0).floor(),
        );
        // Far stars first so near ones overwrite them.
        let mut order: [usize; STARS] = [0; STARS];
        for (i, o) in order.iter_mut().enumerate() {
            *o = i;
        }
        order.sort_unstable_by(|&a, &b| self.stars[b].z.total_cmp(&self.stars[a].z));
        for &i in &order {
            let s = self.stars[i];
            let sx = ox + s.x / s.z * w / 4.0;
            let sy = oy + s.y / s.z * h / 4.0;
            if !(sx.is_finite() && sy.is_finite()) || sx < 0.0 || sy < 0.0 {
                continue;
            }
            let (px, py) = (sx.round(), sy.round());
            if px > f32::from(u16::MAX) || py > f32::from(u16::MAX) {
                continue;
            }
            let (px, py) = (px as u16, py as u16);
            if !area.contains((px, py).into()) {
                continue;
            }
            let ch = if s.z > 0.66 {
                '.'
            } else if s.z > 0.33 {
                '∙'
            } else if s.z > 0.15 {
                '•'
            } else {
                '*'
            };
            put(buf, px, py, ch, gradient[grad_index(1.0 - s.z)]);
        }
    }
}
