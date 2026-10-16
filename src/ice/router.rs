//! ROUTER: rotate pipe tiles to route the signal from the left edge to the right edge (`bounce`).

use std::time::Duration;

use rand::RngExt;
use rand::seq::SliceRandom;

use super::{IceGame, IceInput, IceOutcome};
use crate::game::danger::DangerParams;
use crate::rng::Rng;

/// Trace change when solved (negative).
pub const TRACE_ON_SUCCESS: f32 = -30.0;
/// Trace added on timeout.
pub const TRACE_ON_TIMEOUT: f32 = 10.0;
/// Time limit in seconds.
pub const TIME_LIMIT_SECS: u64 = 45;

const N: u8 = 1;
const E: u8 = 2;
const S: u8 = 4;
const W: u8 = 8;

/// The shape of a tile before rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    /// East-west pipe at rotation 0.
    Straight,
    /// North-east elbow at rotation 0.
    Corner,
    /// Tee open to east, south, west at rotation 0.
    Tee,
    /// Four-way crossing.
    Cross,
}

impl TileKind {
    fn base(self) -> u8 {
        match self {
            TileKind::Straight => E | W,
            TileKind::Corner => N | E,
            TileKind::Tee => E | S | W,
            TileKind::Cross => N | E | S | W,
        }
    }
}

fn rotate(mask: u8, times: u8) -> u8 {
    (0..times % 4).fold(mask, |m, _| ((m << 1) & 15) | (m >> 3))
}

/// The 3x3 glyph rows of a tile with the given open sides (N=1, E=2, S=4, W=8).
fn glyphs(mask: u8) -> [[char; 3]; 3] {
    let center = match mask {
        15 => '\u{256c}',
        7 => '\u{2560}',
        13 => '\u{2563}',
        14 => '\u{2566}',
        11 => '\u{2569}',
        10 => '\u{2550}',
        5 => '\u{2551}',
        6 => '\u{2554}',
        12 => '\u{2557}',
        3 => '\u{255a}',
        9 => '\u{255d}',
        _ => '\u{2219}',
    };
    let mut g = [[' '; 3]; 3];
    g[1][1] = center;
    if mask & N != 0 {
        g[0][1] = '\u{2551}';
    }
    if mask & S != 0 {
        g[2][1] = '\u{2551}';
    }
    if mask & W != 0 {
        g[1][0] = '\u{2550}';
    }
    if mask & E != 0 {
        g[1][2] = '\u{2550}';
    }
    g
}

/// The ROUTER game state.
#[derive(Debug, Clone)]
pub struct Router {
    /// Tiles per row.
    pub width: usize,
    /// Tile rows.
    pub height: usize,
    /// Row of the entry on the left edge.
    pub entry_row: usize,
    /// Row of the exit on the right edge.
    pub exit_row: usize,
    /// Cursor as `(x, y)`.
    pub cursor: (usize, usize),
    /// Tile kinds, row-major.
    pub kinds: Vec<TileKind>,
    /// Clockwise rotations applied to each tile (0..4), row-major.
    pub rot: Vec<u8>,
    /// Indices of the tiles on the generated path.
    pub path: Vec<usize>,
    solution: Vec<u8>,
    flood: Vec<bool>,
    remaining: Duration,
    outcome: Option<IceOutcome>,
}

struct Search<'a> {
    rng: &'a mut Rng,
    w: usize,
    h: usize,
    goal: (usize, usize),
    seen: Vec<bool>,
    path: Vec<(usize, usize)>,
    budget: u32,
}

impl Search<'_> {
    fn run(&mut self, cur: (usize, usize)) -> bool {
        if cur == self.goal {
            return true;
        }
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let mut dirs = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)];
        dirs.shuffle(self.rng);
        for (dx, dy) in dirs {
            let nx = cur.0 as i32 + dx;
            let ny = cur.1 as i32 + dy;
            if nx < 0 || ny < 0 || nx >= self.w as i32 || ny >= self.h as i32 {
                continue;
            }
            let next = (nx as usize, ny as usize);
            let idx = next.1 * self.w + next.0;
            if self.seen[idx] {
                continue;
            }
            self.seen[idx] = true;
            self.path.push(next);
            if self.run(next) {
                return true;
            }
            self.path.pop();
            self.seen[idx] = false;
        }
        false
    }
}

impl Router {
    /// Build a scrambled puzzle with a guaranteed solution.
    pub fn generate(rng: &mut Rng, params: &DangerParams) -> Self {
        let (w, h) = (params.router_w, params.router_h);
        let entry_row = rng.random_range(0..h);
        let exit_row = rng.random_range(0..h);
        let goal = (w - 1, exit_row);
        let mut seen = vec![false; w * h];
        seen[entry_row * w] = true;
        let mut search = Search {
            rng,
            w,
            h,
            goal,
            seen,
            path: vec![(0usize, entry_row)],
            budget: 20_000,
        };
        let found = search.run((0, entry_row));
        let mut cells = search.path;
        if !found {
            cells = (0..w).map(|x| (x, entry_row)).collect();
            let step = if exit_row >= entry_row { 1 } else { -1 };
            let mut y = entry_row as i32;
            while y != exit_row as i32 {
                y += step;
                cells.push((w - 1, y as usize));
            }
        }
        let path: Vec<usize> = cells.iter().map(|&(x, y)| y * w + x).collect();

        let mut kinds: Vec<TileKind> = (0..w * h)
            .map(|_| match rng.random_range(0..4u8) {
                0 => TileKind::Straight,
                1 => TileKind::Corner,
                2 => TileKind::Tee,
                _ => TileKind::Cross,
            })
            .collect();
        let mut solution = vec![0u8; w * h];
        let mut needed = vec![0u8; w * h];
        for (i, &(x, y)) in cells.iter().enumerate() {
            let enter = if i == 0 {
                W
            } else {
                rotate(dir_to(cells[i - 1], (x, y)), 2)
            };
            let leave = if i + 1 == cells.len() {
                E
            } else {
                dir_to((x, y), cells[i + 1])
            };
            let mask = enter | leave;
            let kind = if mask == (E | W) || mask == (N | S) {
                TileKind::Straight
            } else {
                TileKind::Corner
            };
            let idx = y * w + x;
            kinds[idx] = kind;
            needed[idx] = mask;
            solution[idx] = (0..4)
                .find(|&r| rotate(kind.base(), r) == mask)
                .unwrap_or(0);
        }

        let mut rot = vec![0u8; w * h];
        let mut g = Self {
            width: w,
            height: h,
            entry_row,
            exit_row,
            cursor: (0, entry_row),
            kinds,
            rot: Vec::new(),
            path,
            solution: Vec::new(),
            flood: vec![false; w * h],
            remaining: Duration::from_secs(TIME_LIMIT_SECS),
            outcome: None,
        };
        for attempt in 0..200 {
            for r in &mut rot {
                *r = rng.random_range(0..4u8);
            }
            for (i, r) in rot.iter().enumerate() {
                if !g.path.contains(&i) {
                    solution[i] = *r;
                }
            }
            if attempt == 199 {
                // Guarantee a wrong path tile as a last resort.
                let t = g.path[g.path.len() / 2];
                rot[t] = (solution[t] + 1) % 4;
            }
            g.rot = rot.clone();
            let wrong = g
                .path
                .iter()
                .any(|&i| rotate(g.kinds[i].base(), g.rot[i]) != needed[i]);
            g.refresh();
            if wrong && !g.solved() {
                break;
            }
        }
        g.solution = solution;
        g
    }

    fn mask(&self, i: usize) -> u8 {
        rotate(self.kinds[i].base(), self.rot[i])
    }

    fn refresh(&mut self) {
        let w = self.width;
        self.flood.iter_mut().for_each(|f| *f = false);
        let start = self.entry_row * w;
        if self.mask(start) & W == 0 {
            return;
        }
        self.flood[start] = true;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let m = self.mask(i);
            let mut next: [Option<(u8, usize)>; 4] = [None; 4];
            if y > 0 {
                next[0] = Some((N, i - w));
            }
            if y + 1 < self.height {
                next[1] = Some((S, i + w));
            }
            if x > 0 {
                next[2] = Some((W, i - 1));
            }
            if x + 1 < w {
                next[3] = Some((E, i + 1));
            }
            for (bit, j) in next.into_iter().flatten() {
                let opp = rotate(bit, 2);
                if m & bit != 0 && !self.flood[j] && self.mask(j) & opp != 0 {
                    self.flood[j] = true;
                    stack.push(j);
                }
            }
        }
    }

    /// True when the exit tile is reached and opens east.
    pub fn solved(&self) -> bool {
        let i = self.exit_row * self.width + self.width - 1;
        self.flood[i] && self.mask(i) & E != 0
    }

    /// True if tile `(x, y)` is connected to the entry.
    pub fn connected(&self, x: usize, y: usize) -> bool {
        self.flood[y * self.width + x]
    }

    /// Rotation counts per tile (row-major) that solve the puzzle.
    pub fn solution(&self) -> &[u8] {
        &self.solution
    }

    /// Time left on the clock.
    pub fn time_left(&self) -> Duration {
        self.remaining
    }

    /// The CP437 double-line glyphs: `height * 3` strings of `width * 3` characters.
    pub fn render_rows(&self) -> Vec<String> {
        let mut rows = Vec::with_capacity(self.height * 3);
        for y in 0..self.height {
            for sub in 0..3 {
                let mut line = String::with_capacity(self.width * 3 * 3);
                for x in 0..self.width {
                    line.extend(glyphs(self.mask(y * self.width + x))[sub]);
                }
                rows.push(line);
            }
        }
        rows
    }

    fn rotate_cursor(&mut self) {
        let i = self.cursor.1 * self.width + self.cursor.0;
        self.rot[i] = (self.rot[i] + 1) % 4;
        self.refresh();
        if self.solved() {
            self.outcome = Some(IceOutcome::Cleared);
        }
    }
}

fn dir_to(from: (usize, usize), to: (usize, usize)) -> u8 {
    if to.0 > from.0 {
        E
    } else if to.0 < from.0 {
        W
    } else if to.1 > from.1 {
        S
    } else {
        N
    }
}

impl IceGame for Router {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        let (x, y) = &mut self.cursor;
        match input {
            IceInput::Left => *x = x.saturating_sub(1),
            IceInput::Right => *x = (*x + 1).min(self.width - 1),
            IceInput::Up => *y = y.saturating_sub(1),
            IceInput::Down => *y = (*y + 1).min(self.height - 1),
            IceInput::Space | IceInput::Enter => self.rotate_cursor(),
            IceInput::Esc => self.outcome = Some(IceOutcome::Retreated),
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.outcome.is_some() {
            return;
        }
        self.remaining = self.remaining.saturating_sub(dt);
        if self.remaining.is_zero() {
            self.outcome = Some(IceOutcome::Failed {
                trace: TRACE_ON_TIMEOUT,
                ego: 0,
            });
        }
    }

    fn outcome(&self) -> Option<IceOutcome> {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::params;
    use crate::rng::substream;

    fn game(seed: u64, d: u8) -> Router {
        let mut rng = substream(seed, 0, "router-test", 0);
        Router::generate(&mut rng, &params(d))
    }

    #[test]
    fn scrambled_start_is_unsolved_with_a_wrong_path_tile() {
        for d in 1..=5u8 {
            for seed in 0..500 {
                let g = game(seed, d);
                assert!(!g.solved(), "d{d} seed{seed}");
                assert!(g.outcome().is_none());
                assert!(g.path.iter().any(|&i| g.rot[i] != g.solution[i]
                    && rotate(g.kinds[i].base(), g.rot[i])
                        != rotate(g.kinds[i].base(), g.solution[i])));
            }
        }
    }

    #[test]
    fn recorded_solution_connects_the_exit() {
        for d in 1..=5u8 {
            for seed in 0..500 {
                let mut g = game(seed, d);
                g.rot = g.solution().to_vec();
                g.refresh();
                assert!(g.solved(), "d{d} seed{seed}");
                for &i in &g.path {
                    assert!(g.connected(i % g.width, i / g.width));
                }
            }
        }
    }

    #[test]
    fn path_is_self_avoiding_and_spans_entry_to_exit() {
        for d in 1..=5u8 {
            for seed in 0..200 {
                let g = game(seed, d);
                let mut p = g.path.clone();
                assert_eq!(p[0], g.entry_row * g.width);
                assert_eq!(*p.last().unwrap(), g.exit_row * g.width + g.width - 1);
                for pair in p.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    let dx = (a % g.width).abs_diff(b % g.width);
                    let dy = (a / g.width).abs_diff(b / g.width);
                    assert_eq!(dx + dy, 1);
                }
                p.sort_unstable();
                p.dedup();
                assert_eq!(p.len(), g.path.len());
            }
        }
    }

    #[test]
    fn rotating_cycles_and_solving_by_input_clears() {
        let mut g = game(11, 2);
        let i = g.cursor.1 * g.width + g.cursor.0;
        let start = g.rot[i];
        for _ in 0..4 {
            g.handle(IceInput::Space);
            if g.outcome().is_some() {
                break;
            }
        }
        if g.outcome().is_none() {
            assert_eq!(g.rot[i], start);
        }
        let mut g = game(12, 3);
        let target = g.solution().to_vec();
        for y in 0..g.height {
            for x in 0..g.width {
                g.cursor = (x, y);
                let i = y * g.width + x;
                while g.rot[i] != target[i] && g.outcome().is_none() {
                    g.handle(IceInput::Enter);
                }
            }
        }
        assert_eq!(g.outcome(), Some(IceOutcome::Cleared));
    }

    #[test]
    fn timeout_fails_with_trace_and_esc_retreats() {
        let mut g = game(1, 1);
        g.tick(Duration::from_secs(44));
        assert!(g.outcome().is_none());
        g.tick(Duration::from_secs(2));
        assert_eq!(
            g.outcome(),
            Some(IceOutcome::Failed {
                trace: 10.0,
                ego: 0
            })
        );
        let mut g = game(1, 1);
        g.handle(IceInput::Esc);
        assert_eq!(g.outcome(), Some(IceOutcome::Retreated));
    }

    #[test]
    fn render_rows_are_three_by_three_per_tile_in_double_lines() {
        let mut g = game(2, 1);
        g.rot = g.solution().to_vec();
        let rows = g.render_rows();
        assert_eq!(rows.len(), g.height * 3);
        assert!(rows.iter().all(|r| r.chars().count() == g.width * 3));
        // The entry tile's west arm is open.
        let y = g.entry_row * 3 + 1;
        assert_eq!(rows[y].chars().next(), Some('\u{2550}'));
        assert!(
            rows.iter()
                .flat_map(|r| r.chars())
                .all(|c| c == ' ' || ('\u{2550}'..='\u{256c}').contains(&c) || c == '\u{2219}')
        );
    }
}
