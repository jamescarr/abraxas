//! Procedural visions rendered into a ratatui buffer.
//!
//! Every vision draws from time `t` (seconds) and the act gradient: six resolved colours, dark to
//! bright, used as discrete stops with no blending. Glyphs stay inside CP437 and are one cell wide.

pub mod fire;
pub mod hexagram;
pub mod mandala;
pub mod mirror;
pub mod noise;
pub mod ouroboros;
pub mod plasma;
pub mod starfield;
pub mod tunnel;

use std::time::Duration;

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::pack::model::VisionKind;
use crate::rng::{substream, substream_u64};

pub use hexagram::{HexagramCast, hexagram_title};

/// The shade ramp, dark to bright.
pub const RAMP: &str = " .:-=+*%#@";

/// Vision time while calm: frozen here.
const CALM_T: f32 = 1.0;

/// Gradient stop for a shade in 0..=1.
pub(crate) fn grad_index(shade: f32) -> usize {
    ((shade.max(0.0) * 6.0) as usize).min(5)
}

/// Ramp glyph for a shade in 0..=1.
pub(crate) fn ramp_char(shade: f32) -> char {
    let i = ((shade.max(0.0) * 10.0) as usize).min(9);
    char::from(RAMP.as_bytes()[i])
}

/// Writes one cell if `(x, y)` lies inside `buf`.
pub(crate) fn put(buf: &mut Buffer, x: u16, y: u16, ch: char, fg: Color) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch).set_fg(fg).set_bg(Color::Black);
    }
}

/// Writes the shade as ramp glyph + discrete gradient colour.
pub(crate) fn put_shade(buf: &mut Buffer, x: u16, y: u16, shade: f32, gradient: &[Color; 6]) {
    put(buf, x, y, ramp_char(shade), gradient[grad_index(shade)]);
}

/// Cheap deterministic 64-bit hash of four values.
pub(crate) fn mix(a: u64, b: u64, c: u64, d: u64) -> u64 {
    let mut z = a
        .wrapping_add(0x9E37_79B9_7F4A_7C15)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ b.wrapping_mul(0x94D0_49BB_1331_11EB)
        ^ c.wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ d.wrapping_mul(0xA076_1D64_78BD_642F);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Iterates the cells of `area` that lie inside the buffer, with offsets from the centre in
/// "pixel" units (`dy_px` counts two per row because cells are twice as tall as wide).
pub(crate) struct Cx {
    area: Rect,
    cx: f32,
    cy: f32,
}

impl Cx {
    pub(crate) fn new(area: Rect) -> Self {
        Self {
            area,
            cx: f32::from(area.x) + (f32::from(area.width) / 2.0).floor(),
            cy: f32::from(area.y) + (f32::from(area.height) / 2.0).floor(),
        }
    }

    pub(crate) fn for_each(&self, bounds: Rect, mut f: impl FnMut(u16, u16, f32, f32)) {
        let clip = self.area.intersection(bounds);
        for y in clip.top()..clip.bottom() {
            let dy_px = (f32::from(y) - self.cy) * 2.0;
            for x in clip.left()..clip.right() {
                f(x, y, f32::from(x) - self.cx, dy_px);
            }
        }
    }
}

enum State {
    None,
    Fire(fire::FireState),
    Stars(starfield::StarState),
}

/// One running vision.
pub struct Vision {
    kind: VisionKind,
    gradient: [Color; 6],
    calm: bool,
    t: f32,
    key: u64,
    density: f32,
    snapshot: Option<Buffer>,
    state: State,
}

impl Vision {
    /// `seed`/`depth` seed stateful visions via `substream(seed, depth, "vision-<name>", 0)`.
    pub fn new(kind: VisionKind, seed: u64, depth: u32, gradient: [Color; 6], calm: bool) -> Self {
        let name = match kind {
            VisionKind::Tunnel => "vision-tunnel",
            VisionKind::Plasma => "vision-plasma",
            VisionKind::Mandala => "vision-mandala",
            VisionKind::Fire => "vision-fire",
            VisionKind::Ouroboros => "vision-ouroboros",
            VisionKind::Starfield => "vision-starfield",
            VisionKind::Mirror => "vision-mirror",
            VisionKind::Noise => "vision-noise",
        };
        let state = match kind {
            VisionKind::Fire => {
                State::Fire(fire::FireState::new(substream(seed, depth, name, 0), calm))
            }
            VisionKind::Starfield => {
                State::Stars(starfield::StarState::new(substream(seed, depth, name, 0)))
            }
            _ => State::None,
        };
        Self {
            kind,
            gradient,
            calm,
            t: if calm { CALM_T } else { 0.0 },
            key: substream_u64(seed, depth, name, 0),
            density: 0.35,
            snapshot: None,
            state,
        }
    }

    pub fn kind(&self) -> VisionKind {
        self.kind
    }

    /// Advance time. Calm mode: `t` stays at 1.0 and simulations do not evolve.
    pub fn tick(&mut self, dt: Duration) {
        if self.calm {
            return;
        }
        let secs = dt.as_secs_f32();
        self.t += secs;
        match &mut self.state {
            State::Fire(f) => f.tick(secs),
            State::Stars(s) => s.tick(secs),
            State::None => {}
        }
    }

    /// Seconds of vision time (1.0 when calm).
    pub fn elapsed(&self) -> f32 {
        self.t
    }

    /// Draw into `area` of `buf`, clipped to the buffer.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let (t, g) = (self.t, &self.gradient);
        match (&self.state, self.kind) {
            (State::Fire(f), _) => f.render(g, area, buf),
            (State::Stars(s), _) => s.render(g, area, buf),
            (_, VisionKind::Tunnel) => tunnel::render(t, g, area, buf),
            (_, VisionKind::Plasma) => plasma::render(t, g, area, buf),
            (_, VisionKind::Mandala) => mandala::render(t, g, area, buf),
            (_, VisionKind::Ouroboros) => ouroboros::render(t, g, area, buf),
            (_, VisionKind::Mirror) => {
                mirror::render(t, self.key, self.calm, g, self.snapshot.as_ref(), area, buf);
            }
            (_, VisionKind::Noise) => noise::render(t, self.key, self.density, area, buf),
            // Fire and Starfield always carry their state.
            (State::None, VisionKind::Fire | VisionKind::Starfield) => {}
        }
    }

    /// Mirror vision only: snapshot of the last canvas frame.
    pub fn set_snapshot(&mut self, snapshot: Buffer) {
        if self.kind == VisionKind::Mirror {
            self.snapshot = Some(snapshot);
        }
    }

    /// Noise vision density 0.0..=1.0 (default 0.35); ignored by others.
    pub fn set_density(&mut self, d: f32) {
        self.density = d.clamp(0.0, 1.0);
    }
}

/// Glyphs only, rows joined by newlines (for tests).
pub fn buffer_to_string(buf: &Buffer, area: Rect) -> String {
    let clip = area.intersection(buf.area);
    let mut out = String::with_capacity(usize::from(clip.width + 1) * usize::from(clip.height));
    for y in clip.top()..clip.bottom() {
        if y > clip.top() {
            out.push('\n');
        }
        for x in clip.left()..clip.right() {
            out.push_str(buf.cell((x, y)).map_or(" ", |c| c.symbol()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color::*;

    const GRAD: [Color; 6] = [Black, Blue, DarkGray, Gray, LightCyan, White];
    const ALL: [VisionKind; 8] = [
        VisionKind::Tunnel,
        VisionKind::Plasma,
        VisionKind::Mandala,
        VisionKind::Fire,
        VisionKind::Ouroboros,
        VisionKind::Starfield,
        VisionKind::Mirror,
        VisionKind::Noise,
    ];
    const CANVAS: Rect = Rect::new(0, 0, 80, 25);

    fn run(kind: VisionKind, calm: bool, ticks: u32) -> Buffer {
        let mut v = Vision::new(kind, 42, 3, GRAD, calm);
        for _ in 0..ticks {
            v.tick(Duration::from_millis(33));
        }
        let mut buf = Buffer::empty(CANVAS);
        v.render(CANVAS, &mut buf);
        buf
    }

    fn allowed(c: char) -> bool {
        c.is_ascii() || "█▓▒░·☼Θ∙•".contains(c)
    }

    #[test]
    fn deterministic_for_same_inputs() {
        for kind in ALL {
            assert_eq!(run(kind, false, 20), run(kind, false, 20), "{kind:?}");
        }
    }

    #[test]
    fn calm_is_frozen() {
        for kind in ALL {
            assert_eq!(run(kind, true, 0), run(kind, true, 50), "{kind:?}");
        }
        let mut v = Vision::new(VisionKind::Plasma, 1, 0, GRAD, true);
        v.tick(Duration::from_secs(3));
        assert_eq!(v.elapsed(), 1.0);
    }

    #[test]
    fn time_evolves_when_not_calm() {
        for kind in ALL {
            assert_ne!(run(kind, false, 3), run(kind, false, 40), "{kind:?}");
        }
        let mut v = Vision::new(VisionKind::Tunnel, 1, 0, GRAD, false);
        v.tick(Duration::from_millis(500));
        assert!((v.elapsed() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn kinds_look_different() {
        let pics: Vec<String> = ALL
            .iter()
            .map(|&k| buffer_to_string(&run(k, false, 30), CANVAS))
            .collect();
        for i in 0..pics.len() {
            for j in i + 1..pics.len() {
                assert_ne!(pics[i], pics[j], "{:?} vs {:?}", ALL[i], ALL[j]);
            }
        }
    }

    #[test]
    fn never_panics_on_odd_areas() {
        let areas = [
            Rect::new(0, 0, 0, 0),
            Rect::new(0, 0, 1, 1),
            Rect::new(3, 2, 80, 25),
            Rect::new(70, 20, 40, 40),
            Rect::new(200, 200, 10, 10),
            CANVAS,
        ];
        for kind in ALL {
            let mut v = Vision::new(kind, 9, 1, GRAD, false);
            v.tick(Duration::from_millis(700));
            v.set_snapshot(Buffer::empty(Rect::new(0, 0, 5, 3)));
            for area in areas {
                let mut buf = Buffer::empty(CANVAS);
                v.render(area, &mut buf);
            }
        }
    }

    #[test]
    fn fills_whole_area_with_cp437_glyphs_and_known_colours() {
        let noise_colors = noise::colors();
        for kind in ALL {
            let buf = run(kind, false, 25);
            for y in 0..25 {
                for x in 0..80 {
                    let c = buf.cell((x, y)).expect("in range");
                    let mut chars = c.symbol().chars();
                    let ch = chars.next().expect("glyph written");
                    assert!(chars.next().is_none(), "{kind:?} wrote multi-char symbol");
                    assert!(allowed(ch), "{kind:?} wrote {ch:?}");
                    let ok = GRAD.contains(&c.fg)
                        || (kind == VisionKind::Noise && noise_colors.contains(&c.fg));
                    assert!(ok, "{kind:?} at {x},{y} used {:?}", c.fg);
                    assert_eq!(c.bg, Black);
                }
            }
        }
    }

    #[test]
    fn mirror_with_noise_cells_uses_noise_colours_only() {
        // Only the glitch row may leave the gradient; verify it with a snapshot in play.
        let mut snap = Buffer::empty(CANVAS);
        snap.set_string(0, 0, "hello", ratatui::style::Style::new().fg(Red));
        let mut v = Vision::new(VisionKind::Mirror, 5, 2, GRAD, false);
        v.set_snapshot(snap);
        v.tick(Duration::from_millis(10));
        let mut buf = Buffer::empty(CANVAS);
        v.render(CANVAS, &mut buf);
        let row = mirror::glitch_row(v.elapsed(), v.key, 25).expect("window active at 10ms");
        for y in 0..25u16 {
            for x in 0..80u16 {
                let c = buf.cell((x, y)).expect("cell");
                if y == row {
                    assert!(noise::colors().contains(&c.fg) || c.fg == DarkGray);
                } else {
                    assert!(c.fg == Red || c.fg == Reset || GRAD.contains(&c.fg));
                }
            }
        }
    }

    #[test]
    fn mirror_reflects_snapshot_with_row_shifts() {
        let mut snap = Buffer::empty(CANVAS);
        for y in 0..25u16 {
            for x in 0..80u16 {
                let ch = char::from(b'a' + ((u32::from(x) * 7 + u32::from(y) * 3) % 26) as u8);
                snap.set_string(x, y, ch.to_string(), ratatui::style::Style::new().fg(Red));
            }
        }
        let mut v = Vision::new(VisionKind::Mirror, 5, 2, GRAD, false);
        v.set_snapshot(snap.clone());
        v.tick(Duration::from_millis(200)); // t = 0.2: no glitch window (window is 0..0.1 of each 0.5)
        assert_eq!(mirror::glitch_row(v.elapsed(), v.key, 25), None);
        let mut buf = Buffer::empty(CANVAS);
        v.render(CANVAS, &mut buf);
        for y in 0..25u16 {
            let shift = mirror::row_shift(y, v.elapsed());
            for x in 0..80u16 {
                let sx = (79 - i32::from(x) - shift).rem_euclid(80) as u16;
                assert_eq!(buf.cell((x, y)), snap.cell((sx, y)), "at {x},{y}");
            }
        }
    }

    #[test]
    fn mirror_glitch_window_is_100ms_every_500ms() {
        assert!(mirror::glitch_row(0.05, 1, 25).is_some());
        assert!(mirror::glitch_row(0.10, 1, 25).is_none());
        assert!(mirror::glitch_row(0.45, 1, 25).is_none());
        assert!(mirror::glitch_row(0.52, 1, 25).is_some());
        assert!(mirror::glitch_row(0.52, 1, 0).is_none());
    }

    #[test]
    fn mirror_without_snapshot_is_not_blank() {
        let buf = run(VisionKind::Mirror, true, 0);
        let s = buffer_to_string(&buf, CANVAS);
        assert!(s.chars().filter(|c| *c != ' ' && *c != '\n').count() > 200);
    }

    #[test]
    fn fire_heat_stays_bounded_and_bottom_row_is_hot() {
        let mut f = fire::FireState::new(substream(1, 0, "vision-fire", 0), false);
        for _ in 0..200 {
            f.tick(1.0 / 30.0);
            let heat = f.heat();
            assert!(heat.iter().all(|&h| h <= 36));
            assert!(heat[fire::H * fire::W..].iter().all(|&h| h == 36));
        }
        let hot = f.heat()[..fire::W].iter().filter(|&&h| h > 0).count();
        assert!(hot > 0, "flames reach the top row");
    }

    #[test]
    fn fire_steps_at_30_hz_from_accumulated_dt() {
        let mk = || fire::FireState::new(substream(1, 0, "vision-fire", 0), false);
        let (mut a, mut b, mut c) = (mk(), mk(), mk());
        a.tick(1.0 / 30.0);
        for _ in 0..4 {
            b.tick(0.01); // 4 x 10 ms = 40 ms: only one step total
        }
        c.tick(0.034);
        assert_eq!(a.heat(), c.heat());
        assert_eq!(a.heat(), b.heat());
        let mut d = mk();
        d.tick(0.02);
        assert_ne!(d.heat(), a.heat(), "less than a step does nothing");
    }

    #[test]
    fn starfield_respawns_at_z1_and_keeps_z_in_range() {
        let mut s = starfield::StarState::new(substream(1, 0, "vision-starfield", 0));
        assert_eq!(s.stars().len(), 160);
        s.stars_mut()[0].z = 0.03;
        s.tick(0.1); // 0.03 - 0.045 <= 0.02 -> respawn
        assert_eq!(s.stars()[0].z, 1.0);
        for _ in 0..500 {
            s.tick(0.033);
            assert!(s.stars().iter().all(|st| st.z > 0.0 && st.z <= 1.0));
        }
    }

    #[test]
    fn starfield_glyphs_follow_depth() {
        let mut s = starfield::StarState::new(substream(1, 0, "vision-starfield", 0));
        for (i, st) in s.stars_mut().iter_mut().enumerate() {
            st.x = 0.0;
            st.y = 0.0;
            st.z = 1.0;
            if i == 0 {
                st.z = 0.1;
            }
        }
        let mut buf = Buffer::empty(CANVAS);
        s.render(&GRAD, CANVAS, &mut buf);
        // z = 1 stars sit on the centre; the nearest star at z = .1 also projects to the centre
        // (x = y = 0) and, being drawn last, wins with '*'.
        assert_eq!(buf.cell((40, 12)).expect("cell").symbol(), "*");
    }

    #[test]
    fn ouroboros_head_follows_1_1_t() {
        let (px, py) = ouroboros::sample_pos(CANVAS, 0, 2.0);
        let a = 1.1f32 * 2.0;
        assert!((px - (40.0 + 0.36 * 80.0 * a.cos())).abs() < 1e-3);
        assert!((py - (12.0 + 0.40 * 25.0 * a.sin())).abs() < 1e-3);
        let mut v = Vision::new(VisionKind::Ouroboros, 1, 0, GRAD, false);
        v.tick(Duration::from_secs(2));
        let mut buf = Buffer::empty(CANVAS);
        v.render(CANVAS, &mut buf);
        let (hx, hy) = (px.round() as u16, py.round() as u16);
        assert_eq!(buf.cell((hx, hy)).expect("cell").symbol(), "Θ");
        assert_eq!(buf.cell((hx, hy)).expect("cell").fg, White);
    }

    #[test]
    fn noise_density_controls_fill() {
        let count = |d: f32| {
            let mut v = Vision::new(VisionKind::Noise, 1, 0, GRAD, false);
            v.set_density(d);
            let mut buf = Buffer::empty(CANVAS);
            v.render(CANVAS, &mut buf);
            buffer_to_string(&buf, CANVAS)
                .chars()
                .filter(|c| !" \n".contains(*c))
                .count()
        };
        assert_eq!(count(0.0), 0);
        assert_eq!(count(1.0), 2000);
        let mid = count(0.35);
        assert!((500..900).contains(&mid), "{mid}");
    }

    #[test]
    fn mandala_has_sun_at_centre_and_tunnel_dark_centre() {
        let m = run(VisionKind::Mandala, false, 5);
        assert_eq!(m.cell((40, 12)).expect("cell").symbol(), "☼");
        let t = run(VisionKind::Tunnel, false, 5);
        assert_eq!(t.cell((40, 12)).expect("cell").fg, Black);
    }

    #[test]
    fn grad_index_is_discrete_and_clamped() {
        assert_eq!(grad_index(-1.0), 0);
        assert_eq!(grad_index(0.0), 0);
        assert_eq!(grad_index(0.5), 3);
        assert_eq!(grad_index(1.0), 5);
        assert_eq!(grad_index(7.0), 5);
        assert_eq!(ramp_char(0.0), ' ');
        assert_eq!(ramp_char(1.0), '@');
    }
}
