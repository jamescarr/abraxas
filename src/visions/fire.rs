//! Doom fire: a heat grid cooled and shuffled upward at 30 steps per second.

use rand::RngExt;
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use super::{RAMP, grad_index, put};
use crate::rng::Rng;

/// Grid width in cells.
pub(super) const W: usize = 80;
/// Visible grid height; the grid has one more (always hot) row below.
pub(super) const H: usize = 25;
const MAX_HEAT: u8 = 36;
const STEP: f32 = 1.0 / 30.0;
/// Steps taken at construction for calm mode, so the frozen picture is a flame.
const PREWARM: usize = 40;

#[derive(Debug, Clone)]
pub(super) struct FireState {
    heat: Vec<u8>,
    rng: Rng,
    acc: f32,
}

impl FireState {
    pub(super) fn new(rng: Rng, calm: bool) -> Self {
        let mut heat = vec![0u8; W * (H + 1)];
        heat[H * W..].fill(MAX_HEAT);
        let mut s = Self {
            heat,
            rng,
            acc: 0.0,
        };
        if calm {
            for _ in 0..PREWARM {
                s.step();
            }
        }
        s
    }

    fn step(&mut self) {
        for y in 1..=H {
            for x in 0..W {
                let rnd = self.rng.random_range(0..4usize);
                let dst = (x + 1).saturating_sub(rnd).min(W - 1);
                let src = self.heat[y * W + x];
                self.heat[(y - 1) * W + dst] = src.saturating_sub((rnd & 1) as u8);
            }
        }
    }

    pub(super) fn tick(&mut self, dt: f32) {
        self.acc += dt;
        // A long stall should not trigger an unbounded burst of simulation.
        self.acc = self.acc.min(STEP * 30.0);
        while self.acc >= STEP {
            self.acc -= STEP;
            self.step();
        }
    }

    #[cfg(test)]
    pub(super) fn heat(&self) -> &[u8] {
        &self.heat
    }

    pub(super) fn render(&self, gradient: &[Color; 6], area: Rect, buf: &mut Buffer) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        let clip = area.intersection(buf.area);
        for y in clip.top()..clip.bottom() {
            for x in clip.left()..clip.right() {
                let gx = usize::from(x - area.x) * W / w.max(1);
                let gy = usize::from(y - area.y) * H / h.max(1);
                let heat = self.heat[gy * W + gx];
                let ch = char::from(RAMP.as_bytes()[usize::from(heat) * 9 / 36]);
                let shade = f32::from(heat) / f32::from(MAX_HEAT);
                put(buf, x, y, ch, gradient[grad_index(shade)]);
            }
        }
    }
}
