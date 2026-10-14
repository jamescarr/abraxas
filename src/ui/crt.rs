//! The CRT post-process: odd rows dim to 85% foreground and the whole picture flickers by +-3%
//! for 80 ms every 2-4 seconds. Truecolor only (indexed colours cannot be scaled) and off in calm
//! mode or with `display.crt = false`.

use std::time::Duration;

use rand::{RngExt, SeedableRng};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::config::Config;
use crate::palette::Palette;
use crate::rng::Rng;

/// Foreground factor on odd rows.
pub const SCANLINE: f32 = 0.85;
/// Brightness swing of a flicker.
pub const FLICKER_DELTA: f32 = 0.03;
/// How long one flicker lasts.
pub const FLICKER_LEN: Duration = Duration::from_millis(80);

/// Whether the pass runs at all: truecolor output, `display.crt` on, calm mode off.
pub fn enabled(config: &Config, palette: &Palette) -> bool {
    palette.truecolor && config.display.crt && !config.display.calm
}

/// Deterministic flicker schedule: one 80 ms flicker every 2-4 s, each brighter or dimmer by 3%.
#[derive(Debug, Clone)]
pub struct Flicker {
    rng: Rng,
    start: Duration,
    end: Duration,
    delta: f32,
}

impl Flicker {
    pub fn new(seed: u64) -> Self {
        let mut f = Self {
            rng: Rng::seed_from_u64(seed),
            start: Duration::ZERO,
            end: Duration::ZERO,
            delta: 0.0,
        };
        f.schedule_after(Duration::ZERO);
        f
    }

    fn schedule_after(&mut self, from: Duration) {
        let gap = Duration::from_millis(self.rng.random_range(2000..=4000));
        self.start = from + gap;
        self.end = self.start + FLICKER_LEN;
        self.delta = if self.rng.random_bool(0.5) {
            FLICKER_DELTA
        } else {
            -FLICKER_DELTA
        };
    }

    /// The brightness factor at `elapsed` (1.0 between flickers). `elapsed` must not go
    /// backwards between calls.
    pub fn factor(&mut self, elapsed: Duration) -> f32 {
        while elapsed >= self.end {
            let end = self.end;
            self.schedule_after(end);
        }
        if elapsed >= self.start {
            1.0 + self.delta
        } else {
            1.0
        }
    }
}

fn scale(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let s = |v: u8| (f32::from(v) * f).round().clamp(0.0, 255.0) as u8;
            Color::Rgb(s(r), s(g), s(b))
        }
        other => other,
    }
}

/// Apply the pass to `area` of `buf`: foreground of odd rows (relative to `area`) x 0.85, then
/// foreground and background of every cell x `flicker`. Only `Color::Rgb` cells change.
pub fn apply(buf: &mut Buffer, area: Rect, flicker: f32) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        let odd = (y - area.top()) % 2 == 1;
        let fg_factor = if odd { SCANLINE * flicker } else { flicker };
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.fg = scale(cell.fg, fg_factor);
                if flicker != 1.0 {
                    cell.bg = scale(cell.bg, flicker);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TruecolorMode;

    fn white_buf(h: u16) -> Buffer {
        let area = Rect::new(2, 3, 4, h);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)]
                    .set_fg(Color::Rgb(200, 100, 0))
                    .set_bg(Color::Rgb(0, 0, 100));
            }
        }
        buf
    }

    #[test]
    fn only_odd_rows_dim_and_backgrounds_stay() {
        let mut buf = white_buf(4);
        apply(&mut buf, Rect::new(2, 3, 4, 4), 1.0);
        assert_eq!(buf[(2, 3)].fg, Color::Rgb(200, 100, 0));
        assert_eq!(buf[(2, 4)].fg, Color::Rgb(170, 85, 0));
        assert_eq!(buf[(5, 5)].fg, Color::Rgb(200, 100, 0));
        assert_eq!(buf[(5, 6)].fg, Color::Rgb(170, 85, 0));
        assert_eq!(buf[(2, 4)].bg, Color::Rgb(0, 0, 100));
        // Outside the area nothing changes.
        assert_eq!(buf[(1, 4)].fg, Color::Reset);
        assert_eq!(buf[(2, 7)].fg, Color::Reset);
    }

    #[test]
    fn flicker_scales_fg_and_bg_and_leaves_indexed_colours_alone() {
        let mut buf = white_buf(2);
        buf[(3, 3)]
            .set_fg(Color::Indexed(7))
            .set_bg(Color::Indexed(1));
        apply(&mut buf, Rect::new(2, 3, 4, 2), 1.03);
        assert_eq!(buf[(2, 3)].fg, Color::Rgb(206, 103, 0));
        assert_eq!(buf[(2, 3)].bg, Color::Rgb(0, 0, 103));
        assert_eq!(buf[(3, 3)].fg, Color::Indexed(7));
        assert_eq!(buf[(3, 3)].bg, Color::Indexed(1));
        // Channels clamp at 255.
        buf[(4, 3)].set_fg(Color::Rgb(255, 255, 255));
        apply(&mut buf, Rect::new(2, 3, 4, 2), 1.03);
        assert_eq!(buf[(4, 3)].fg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn enabled_needs_truecolor_crt_and_no_calm() {
        let mut cfg = Config::default();
        let tc = Palette::from_env(TruecolorMode::Always, None);
        let idx = Palette::from_env(TruecolorMode::Never, None);
        assert!(enabled(&cfg, &tc));
        assert!(!enabled(&cfg, &idx));
        cfg.display.calm = true;
        assert!(!enabled(&cfg, &tc));
        cfg.display.calm = false;
        cfg.display.crt = false;
        assert!(!enabled(&cfg, &tc));
    }

    #[test]
    fn flicker_windows_are_80ms_every_2_to_4_seconds_and_deterministic() {
        let sample = |seed| {
            let mut f = Flicker::new(seed);
            // Step at 10 ms; collect (start_ms, len_ms) of every window in 60 s.
            let mut windows = Vec::new();
            let mut open: Option<(u64, f32)> = None;
            for ms in (0..60_000u64).step_by(10) {
                let v = f.factor(Duration::from_millis(ms));
                match (open, v != 1.0) {
                    (None, true) => open = Some((ms, v)),
                    (Some((s, d)), false) => {
                        windows.push((s, ms - s, d));
                        open = None;
                    }
                    _ => {}
                }
            }
            windows
        };
        let a = sample(42);
        assert_eq!(a, sample(42));
        assert_ne!(a, sample(43));
        assert!(a.len() >= 14 && a.len() <= 30, "{}", a.len());
        let mut prev_end = 0;
        for (start, len, delta) in &a {
            assert_eq!(*len, 80);
            assert!((delta - 1.0).abs() > 0.029 && (delta - 1.0).abs() < 0.031);
            let gap = start - prev_end;
            assert!((2000..=4010).contains(&gap), "gap {gap}");
            prev_end = start + len;
        }
    }
}
