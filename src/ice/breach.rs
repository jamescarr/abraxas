//! BREACH: pick codes from a grid into a buffer to complete daemons (guards GATE nodes).

use std::time::Duration;

use rand::RngExt;

use super::{IceGame, IceInput, IceOutcome, Loadout};
use crate::game::danger::DangerParams;
use crate::rng::Rng;

/// The six cell codes; grid cells hold indices into this table.
pub const CODES: [&str; 6] = ["1C", "55", "BD", "E9", "7A", "FF"];
/// Trace added when time runs out.
pub const TRACE_ON_TIMEOUT: f32 = 15.0;
/// EGO lost when time runs out (negative).
pub const EGO_ON_TIMEOUT: i32 = -5;
/// Trace added when the buffer fills without the required daemon.
pub const TRACE_ON_FULL: f32 = 10.0;
/// Trace added by Esc.
pub const TRACE_ON_ABORT: f32 = 5.0;
/// KB granted by DATAMINE.
pub const DATAMINE_KB: u32 = 16;
/// Trace change granted by TRACE WIPE (negative).
pub const TRACE_WIPE_DELTA: f32 = -10.0;

/// What a daemon does when completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonKind {
    /// Completing it clears the ICE.
    Required,
    /// +16 KB.
    DataMine,
    /// Trace -10.
    TraceWipe,
}

/// A target code sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daemon {
    /// Display name.
    pub name: &'static str,
    /// Effect kind.
    pub kind: DaemonKind,
    /// Required code indices, contiguous in the buffer.
    pub seq: Vec<u8>,
}

/// The line the next pick must come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// A row (cursor moves left/right).
    Row(usize),
    /// A column (cursor moves up/down).
    Column(usize),
}

/// The BREACH game state.
#[derive(Debug, Clone)]
pub struct Breach {
    /// `grid[row][col]` is an index into [`CODES`].
    pub grid: Vec<Vec<u8>>,
    /// Grid side length.
    pub size: usize,
    /// Buffer capacity (includes BUFFER.EXE's +1).
    pub buffer: usize,
    /// Daemons; the required one is first.
    pub daemons: Vec<Daemon>,
    /// Picks so far as `(row, col)`.
    pub picks: Vec<(usize, usize)>,
    /// Cursor as `(row, col)`.
    pub cursor: (usize, usize),
    /// Line the next pick must come from.
    pub axis: Axis,
    /// True once the first pick started the timer.
    pub started: bool,
    remaining: Duration,
    loadout: Loadout,
    outcome: Option<IceOutcome>,
}

fn staircase(len: usize) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 0)];
    while out.len() < len {
        let (r, c) = out[out.len() - 1];
        out.push(if out.len() % 2 == 1 {
            (r + 1, c)
        } else {
            (r, c + 1)
        });
    }
    out
}

/// A random legal pick path of `len` distinct cells.
fn random_path(rng: &mut Rng, n: usize, len: usize) -> Vec<(usize, usize)> {
    'tries: for _ in 0..200 {
        let mut path = vec![(0, rng.random_range(0..n))];
        while path.len() < len {
            let (r, c) = path[path.len() - 1];
            let col_move = path.len() % 2 == 1;
            let options: Vec<(usize, usize)> = (0..n)
                .map(|i| if col_move { (i, c) } else { (r, i) })
                .filter(|p| !path.contains(p))
                .collect();
            if options.is_empty() {
                continue 'tries;
            }
            path.push(options[rng.random_range(0..options.len())]);
        }
        return path;
    }
    staircase(len)
}

impl Breach {
    /// Build a grid with the daemons planted along a random legal path.
    pub fn generate(rng: &mut Rng, params: &DangerParams, loadout: Loadout) -> Self {
        let n = params.breach_grid;
        let buffer = params.breach_buffer + usize::from(loadout.buffer_plus);
        let path_len = buffer.min(2 * n - 1);
        let path = random_path(rng, n, path_len);
        let mut grid: Vec<Vec<u8>> = (0..n)
            .map(|_| (0..n).map(|_| rng.random_range(0..6u8)).collect())
            .collect();

        let d = params.danger;
        let mut specs: Vec<(&'static str, DaemonKind, usize)> =
            vec![("BACKDOOR", DaemonKind::Required, if d >= 3 { 3 } else { 2 })];
        if d >= 2 {
            specs.push(("DATAMINE", DaemonKind::DataMine, 2));
        }
        if d >= 4 {
            specs.push(("TRACE WIPE", DaemonKind::TraceWipe, 3));
        }
        specs.truncate(params.breach_daemons.max(1));

        let mut taken = vec![false; path.len()];
        let mut daemons = Vec::new();
        for (i, (name, kind, k)) in specs.into_iter().enumerate() {
            let windows: Vec<usize> = (0..=path.len().saturating_sub(k))
                .filter(|&s| path.len() >= k && taken[s..s + k].iter().all(|t| !t))
                .collect();
            let Some(&start) = windows.get(rng.random_range(0..windows.len().max(1))) else {
                debug_assert!(i > 0, "required daemon must fit");
                continue;
            };
            let seq: Vec<u8> = (0..k).map(|_| rng.random_range(0..6u8)).collect();
            for (j, &code) in seq.iter().enumerate() {
                let (r, c) = path[start + j];
                grid[r][c] = code;
                taken[start + j] = true;
            }
            daemons.push(Daemon { name, kind, seq });
        }

        let first = (0usize, 0usize);
        let mut g = Self {
            grid,
            size: n,
            buffer,
            daemons,
            picks: Vec::new(),
            cursor: first,
            axis: Axis::Row(0),
            started: false,
            remaining: Duration::from_secs(u64::from(params.breach_secs)),
            loadout,
            outcome: None,
        };
        g.cursor = (0, n / 2);
        g
    }

    /// Programs after play (unchanged; BREACH consumes none).
    pub fn loadout(&self) -> Loadout {
        self.loadout
    }

    /// Time left on the clock.
    pub fn time_left(&self) -> Duration {
        self.remaining
    }

    /// The text of a cell.
    pub fn code(&self, row: usize, col: usize) -> &'static str {
        CODES[usize::from(self.grid[row][col])]
    }

    /// True if the cell was already picked.
    pub fn is_used(&self, row: usize, col: usize) -> bool {
        self.picks.contains(&(row, col))
    }

    /// The code indices in the buffer.
    pub fn buffer_codes(&self) -> Vec<u8> {
        self.picks.iter().map(|&(r, c)| self.grid[r][c]).collect()
    }

    /// True if the daemon's sequence appears contiguously in the buffer.
    pub fn completed(&self, daemon: usize) -> bool {
        let seq = &self.daemons[daemon].seq;
        self.buffer_codes()
            .windows(seq.len())
            .any(|w| w == seq.as_slice())
    }

    /// `(KB, trace delta)` earned from completed optional daemons.
    pub fn rewards(&self) -> (u32, f32) {
        let mut kb = 0;
        let mut trace = 0.0;
        for (i, d) in self.daemons.iter().enumerate() {
            if !self.completed(i) {
                continue;
            }
            match d.kind {
                DaemonKind::Required => {}
                DaemonKind::DataMine => kb += DATAMINE_KB,
                DaemonKind::TraceWipe => trace += TRACE_WIPE_DELTA,
            }
        }
        (kb, trace)
    }

    fn line_cells(&self) -> Vec<(usize, usize)> {
        (0..self.size)
            .map(|i| match self.axis {
                Axis::Row(r) => (r, i),
                Axis::Column(c) => (i, c),
            })
            .collect()
    }

    fn pick(&mut self) {
        let (r, c) = self.cursor;
        let in_line = match self.axis {
            Axis::Row(row) => r == row,
            Axis::Column(col) => c == col,
        };
        if !in_line || self.is_used(r, c) {
            return;
        }
        self.picks.push((r, c));
        self.started = true;
        if self.completed(0) {
            self.outcome = Some(IceOutcome::Cleared);
            return;
        }
        self.axis = if self.picks.len() % 2 == 1 {
            Axis::Column(c)
        } else {
            Axis::Row(r)
        };
        let next = self
            .line_cells()
            .into_iter()
            .find(|&(a, b)| !self.is_used(a, b));
        match next {
            Some(cell) if self.picks.len() < self.buffer => self.cursor = cell,
            _ => {
                self.outcome = Some(IceOutcome::Failed {
                    trace: TRACE_ON_FULL,
                    ego: 0,
                });
            }
        }
    }
}

impl IceGame for Breach {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        let n = self.size;
        let (r, c) = &mut self.cursor;
        match (input, self.axis) {
            (IceInput::Esc, _) => {
                self.outcome = Some(IceOutcome::Aborted {
                    trace: TRACE_ON_ABORT,
                });
            }
            (IceInput::Left, Axis::Row(_)) => *c = c.saturating_sub(1),
            (IceInput::Right, Axis::Row(_)) => *c = (*c + 1).min(n - 1),
            (IceInput::Up, Axis::Column(_)) => *r = r.saturating_sub(1),
            (IceInput::Down, Axis::Column(_)) => *r = (*r + 1).min(n - 1),
            (IceInput::Enter | IceInput::Space, _) => self.pick(),
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.outcome.is_some() || !self.started {
            return;
        }
        self.remaining = self.remaining.saturating_sub(dt);
        if self.remaining.is_zero() {
            self.outcome = Some(IceOutcome::Failed {
                trace: TRACE_ON_TIMEOUT,
                ego: EGO_ON_TIMEOUT,
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

    fn game(seed: u64, d: u8, loadout: Loadout) -> Breach {
        let mut rng = substream(seed, 0, "breach-test", 0);
        Breach::generate(&mut rng, &params(d), loadout)
    }

    /// Exhaustive search over legal pick sequences; true if the required daemon can complete.
    fn solvable(g: &Breach) -> bool {
        fn dfs(g: &Breach, picks: &mut Vec<(usize, usize)>, codes: &mut Vec<u8>) -> bool {
            let seq = &g.daemons[0].seq;
            if codes.windows(seq.len()).any(|w| w == seq.as_slice()) {
                return true;
            }
            if picks.len() >= g.buffer {
                return false;
            }
            for i in 0..g.size {
                let cell = match picks.last() {
                    None => (0, i),
                    Some(&(r, c)) => {
                        if picks.len() % 2 == 1 {
                            (i, c)
                        } else {
                            (r, i)
                        }
                    }
                };
                if picks.contains(&cell) {
                    continue;
                }
                picks.push(cell);
                codes.push(g.grid[cell.0][cell.1]);
                let ok = dfs(g, picks, codes);
                picks.pop();
                codes.pop();
                if ok {
                    return true;
                }
            }
            false
        }
        dfs(g, &mut Vec::new(), &mut Vec::new())
    }

    #[test]
    fn required_daemon_is_always_completable_within_buffer() {
        for d in 1..=5u8 {
            for seed in 0..60 {
                let g = game(seed, d, Loadout::default());
                assert_eq!(g.daemons[0].kind, DaemonKind::Required);
                assert_eq!(g.daemons[0].seq.len(), if d >= 3 { 3 } else { 2 });
                assert!(solvable(&g), "d{d} seed{seed}");
            }
        }
    }

    #[test]
    fn daemon_roster_follows_danger() {
        for d in 1..=5u8 {
            for seed in 0..100 {
                let g = game(seed, d, Loadout::default());
                let has = |k| g.daemons.iter().any(|x| x.kind == k);
                assert_eq!(has(DaemonKind::DataMine), d >= 2);
                assert!(!has(DaemonKind::TraceWipe) || d >= 4);
            }
        }
    }

    #[test]
    fn buffer_plus_adds_one_slot() {
        let a = game(1, 3, Loadout::default());
        let b = game(
            1,
            3,
            Loadout {
                buffer_plus: true,
                ..Loadout::default()
            },
        );
        assert_eq!(b.buffer, a.buffer + 1);
    }

    /// Plays the planted solution greedily by searching the DFS and feeding real inputs.
    fn play_solution(g: &mut Breach) -> bool {
        fn find(g: &Breach, picks: &mut Vec<(usize, usize)>) -> Option<Vec<(usize, usize)>> {
            let seq = &g.daemons[0].seq;
            let codes: Vec<u8> = picks.iter().map(|&(r, c)| g.grid[r][c]).collect();
            if codes.windows(seq.len()).any(|w| w == seq.as_slice()) {
                return Some(picks.clone());
            }
            if picks.len() >= g.buffer {
                return None;
            }
            for i in 0..g.size {
                let cell = match picks.last() {
                    None => (0, i),
                    Some(&(_, c)) if picks.len() % 2 == 1 => (i, c),
                    Some(&(r, _)) => (r, i),
                };
                if picks.contains(&cell) {
                    continue;
                }
                picks.push(cell);
                let f = find(g, picks);
                picks.pop();
                if f.is_some() {
                    return f;
                }
            }
            None
        }
        let Some(path) = find(g, &mut Vec::new()) else {
            return false;
        };
        for (r, c) in path {
            while g.cursor.1 < c {
                g.handle(IceInput::Right);
            }
            while g.cursor.1 > c {
                g.handle(IceInput::Left);
            }
            while g.cursor.0 < r {
                g.handle(IceInput::Down);
            }
            while g.cursor.0 > r {
                g.handle(IceInput::Up);
            }
            g.handle(IceInput::Enter);
        }
        true
    }

    #[test]
    fn real_inputs_clear_the_game() {
        for d in 1..=5u8 {
            for seed in 0..20 {
                let mut g = game(seed, d, Loadout::default());
                assert!(play_solution(&mut g));
                assert_eq!(g.outcome(), Some(IceOutcome::Cleared), "d{d} seed{seed}");
            }
        }
    }

    #[test]
    fn timer_runs_only_after_first_pick_and_expiry_fails() {
        let mut g = game(3, 1, Loadout::default());
        g.tick(Duration::from_secs(100));
        assert!(g.outcome().is_none());
        assert_eq!(g.time_left(), Duration::from_secs(40));
        g.handle(IceInput::Enter);
        g.tick(Duration::from_secs(39));
        assert!(g.outcome().is_none());
        g.tick(Duration::from_secs(2));
        assert_eq!(
            g.outcome(),
            Some(IceOutcome::Failed {
                trace: 15.0,
                ego: -5
            })
        );
    }

    #[test]
    fn picks_alternate_and_never_reuse_cells() {
        let mut g = game(5, 2, Loadout::default());
        g.cursor = (0, 2);
        g.handle(IceInput::Enter);
        assert_eq!(g.axis, Axis::Column(2));
        // Left/Right ignored on a column; reuse of the used cell is refused.
        g.handle(IceInput::Left);
        assert_eq!(g.cursor.1, 2);
        g.cursor = (0, 2);
        g.handle(IceInput::Enter);
        assert_eq!(g.picks.len(), 1);
        g.cursor = (3, 2);
        g.handle(IceInput::Enter);
        assert_eq!(g.axis, Axis::Row(3));
        // A cell off the active line is rejected.
        g.cursor = (1, 1);
        g.handle(IceInput::Enter);
        assert_eq!(g.picks.len(), 2);
    }

    #[test]
    fn full_buffer_without_required_fails_and_rewards_count_optionals() {
        let mut g = game(9, 4, Loadout::default());
        // Force a known grid: no required sequence can ever appear.
        for row in &mut g.grid {
            row.iter_mut().for_each(|c| *c = 0);
        }
        let mk = |kind, seq: Vec<u8>| Daemon {
            name: "T",
            kind,
            seq,
        };
        g.daemons = vec![
            mk(DaemonKind::Required, vec![1, 1, 1]),
            mk(DaemonKind::DataMine, vec![0, 0]),
            mk(DaemonKind::TraceWipe, vec![0, 0, 0]),
        ];
        let mut guard = 0;
        while g.outcome().is_none() && guard < 20 {
            g.handle(IceInput::Enter);
            guard += 1;
        }
        assert_eq!(
            g.outcome(),
            Some(IceOutcome::Failed {
                trace: 10.0,
                ego: 0
            })
        );
        assert_eq!(g.picks.len(), g.buffer);
        assert_eq!(g.rewards(), (16, -10.0));
    }
}
