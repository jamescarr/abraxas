//! PHREAK: repeat a growing DTMF tone sequence (guards RELAY nodes).

use std::time::Duration;

use rand::RngExt;
use rand::SeedableRng;

use super::{IceGame, IceInput, IceOutcome, Loadout};
use crate::game::danger::DangerParams;
use crate::rng::Rng;

/// Keypad keys in reading order.
pub const KEYS: [char; 12] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '*', '0', '#'];
/// Gap between flashed tones.
pub const GAP_MS: u64 = 150;
/// Trace added by a miss.
pub const TRACE_ON_MISS: f32 = 10.0;
/// Trace added by Esc.
pub const TRACE_ON_ABORT: f32 = 5.0;

/// The DTMF `(row Hz, column Hz)` pair for a keypad key.
pub fn dtmf(key: char) -> Option<(u16, u16)> {
    let i = KEYS.iter().position(|&k| k == key)?;
    const ROWS: [u16; 4] = [697, 770, 852, 941];
    const COLS: [u16; 3] = [1209, 1336, 1477];
    Some((ROWS[i / 3], COLS[i % 3]))
}

/// Whether the game is playing the sequence or waiting for the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The sequence is flashing.
    Flashing,
    /// The player repeats it.
    Input,
}

/// The PHREAK game state.
#[derive(Debug, Clone)]
pub struct Phreak {
    /// The full sequence (length `start + rounds - 1`); round `r` plays its first `start + r - 1` keys.
    pub seq: Vec<char>,
    /// Current round, 1-based.
    pub round: usize,
    /// Total rounds.
    pub rounds: usize,
    /// Keys entered so far this round.
    pub entered: Vec<char>,
    /// Current phase.
    pub phase: Phase,
    /// Restart counter (increments on every miss).
    pub attempt: u32,
    start: usize,
    tone_ms: u64,
    elapsed: Duration,
    replayed: bool,
    rng: Rng,
    loadout: Loadout,
    outcome: Option<IceOutcome>,
    pending_trace: f32,
}

impl Phreak {
    /// Build a game and begin flashing round 1.
    pub fn generate(rng: &mut Rng, params: &DangerParams, loadout: Loadout) -> Self {
        let mut own = Rng::seed_from_u64(rng.random());
        let seq = Self::new_sequence(&mut own, params.phreak_start + params.phreak_rounds - 1);
        Self {
            seq,
            round: 1,
            rounds: params.phreak_rounds,
            entered: Vec::new(),
            phase: Phase::Flashing,
            attempt: 0,
            start: params.phreak_start,
            tone_ms: u64::from(params.phreak_tone_ms),
            elapsed: Duration::ZERO,
            replayed: false,
            rng: own,
            loadout,
            outcome: None,
            pending_trace: 0.0,
        }
    }

    fn new_sequence(rng: &mut Rng, len: usize) -> Vec<char> {
        (0..len)
            .map(|_| KEYS[rng.random_range(0..KEYS.len())])
            .collect()
    }

    /// Tones in the given round: `start + round - 1`.
    pub fn round_len(&self, round: usize) -> usize {
        self.start + round - 1
    }

    /// The keys the current round expects.
    pub fn expected(&self) -> &[char] {
        &self.seq[..self.round_len(self.round)]
    }

    /// The key currently lit, while flashing.
    pub fn lit(&self) -> Option<char> {
        if self.phase != Phase::Flashing {
            return None;
        }
        let period = self.tone_ms + GAP_MS;
        let ms = self.elapsed.as_millis() as u64;
        let step = (ms / period) as usize;
        (ms % period < self.tone_ms)
            .then(|| self.expected().get(step).copied())
            .flatten()
    }

    /// True if METRONOM.EXE can still replay this round.
    pub fn can_replay(&self) -> bool {
        self.loadout.metronom && !self.replayed && self.phase == Phase::Input
    }

    /// Programs after play (unchanged; PHREAK consumes none).
    pub fn loadout(&self) -> Loadout {
        self.loadout
    }

    /// Drain trace penalties incurred since the last call.
    pub fn take_trace(&mut self) -> f32 {
        std::mem::take(&mut self.pending_trace)
    }

    fn start_flash(&mut self) {
        self.phase = Phase::Flashing;
        self.elapsed = Duration::ZERO;
        self.entered.clear();
    }

    fn press(&mut self, key: char) {
        let idx = self.entered.len();
        if self.expected()[idx] != key {
            self.pending_trace += TRACE_ON_MISS;
            self.attempt += 1;
            let len = self.seq.len();
            self.seq = Self::new_sequence(&mut self.rng, len);
            self.round = 1;
            self.replayed = false;
            self.start_flash();
            return;
        }
        self.entered.push(key);
        if self.entered.len() == self.expected().len() {
            if self.round == self.rounds {
                self.outcome = Some(IceOutcome::Cleared);
            } else {
                self.round += 1;
                self.replayed = false;
                self.start_flash();
            }
        }
    }
}

impl IceGame for Phreak {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        match input {
            IceInput::Esc => {
                self.outcome = Some(IceOutcome::Aborted {
                    trace: TRACE_ON_ABORT,
                });
            }
            IceInput::Char(c) if self.phase == Phase::Input => {
                let c = c.to_ascii_uppercase();
                if c == 'R' {
                    if self.can_replay() {
                        self.replayed = true;
                        self.start_flash();
                    }
                } else if KEYS.contains(&c) {
                    self.press(c);
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.outcome.is_some() || self.phase != Phase::Flashing {
            return;
        }
        self.elapsed += dt;
        let total = (self.tone_ms + GAP_MS) * self.expected().len() as u64;
        if self.elapsed.as_millis() as u64 >= total {
            self.phase = Phase::Input;
            self.entered.clear();
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

    fn game(seed: u64, d: u8, loadout: Loadout) -> Phreak {
        let mut rng = substream(seed, 0, "phreak-test", 0);
        Phreak::generate(&mut rng, &params(d), loadout)
    }

    fn finish_flash(g: &mut Phreak) {
        g.tick(Duration::from_secs(60));
        assert_eq!(g.phase, Phase::Input);
    }

    fn type_round(g: &mut Phreak) {
        for k in g.expected().to_vec() {
            g.handle(IceInput::Char(k));
        }
    }

    #[test]
    fn rounds_grow_by_one_tone_and_finish_cleared() {
        for d in 1..=5u8 {
            let p = params(d);
            let mut g = game(7, d, Loadout::default());
            for r in 1..=p.phreak_rounds {
                assert_eq!(g.round, r);
                assert_eq!(g.expected().len(), p.phreak_start + r - 1);
                finish_flash(&mut g);
                type_round(&mut g);
            }
            assert_eq!(g.outcome(), Some(IceOutcome::Cleared));
            assert_eq!(g.take_trace(), 0.0);
        }
    }

    #[test]
    fn flashing_lights_each_tone_then_gap() {
        let mut g = game(1, 1, Loadout::default());
        let first = g.expected()[0];
        assert_eq!(g.lit(), Some(first));
        g.tick(Duration::from_millis(449));
        assert_eq!(g.lit(), Some(first));
        g.tick(Duration::from_millis(2));
        assert_eq!(g.lit(), None);
        g.tick(Duration::from_millis(150));
        assert_eq!(g.lit(), Some(g.expected()[1]));
    }

    #[test]
    fn input_ignored_while_flashing() {
        let mut g = game(2, 1, Loadout::default());
        g.handle(IceInput::Char(g.expected()[0]));
        assert!(g.entered.is_empty());
    }

    #[test]
    fn miss_costs_trace_and_restarts_with_new_sequence() {
        let mut g = game(3, 2, Loadout::default());
        finish_flash(&mut g);
        type_round(&mut g);
        assert_eq!(g.round, 2);
        finish_flash(&mut g);
        let old = g.seq.clone();
        let wrong = KEYS
            .iter()
            .copied()
            .find(|k| *k != g.expected()[0])
            .unwrap();
        g.handle(IceInput::Char(wrong));
        assert_eq!(g.take_trace(), 10.0);
        assert_eq!(g.attempt, 1);
        assert_eq!(g.round, 1);
        assert_eq!(g.phase, Phase::Flashing);
        assert_ne!(g.seq, old);
        assert!(g.outcome().is_none());
    }

    #[test]
    fn metronom_replays_once_per_round() {
        let mut g = game(
            4,
            1,
            Loadout {
                metronom: true,
                ..Loadout::default()
            },
        );
        finish_flash(&mut g);
        g.handle(IceInput::Char('r'));
        assert_eq!(g.phase, Phase::Flashing);
        finish_flash(&mut g);
        g.handle(IceInput::Char('R'));
        assert_eq!(g.phase, Phase::Input);
        type_round(&mut g);
        finish_flash(&mut g);
        assert!(g.can_replay());
        // Without METRONOM, R is ignored.
        let mut h = game(4, 1, Loadout::default());
        finish_flash(&mut h);
        h.handle(IceInput::Char('R'));
        assert_eq!(h.phase, Phase::Input);
        assert_eq!(h.take_trace(), 0.0);
    }

    #[test]
    fn esc_aborts() {
        let mut g = game(5, 1, Loadout::default());
        g.handle(IceInput::Esc);
        assert_eq!(g.outcome(), Some(IceOutcome::Aborted { trace: 5.0 }));
    }

    #[test]
    fn dtmf_table() {
        assert_eq!(dtmf('1'), Some((697, 1209)));
        assert_eq!(dtmf('5'), Some((770, 1336)));
        assert_eq!(dtmf('9'), Some((852, 1477)));
        assert_eq!(dtmf('*'), Some((941, 1209)));
        assert_eq!(dtmf('0'), Some((941, 1336)));
        assert_eq!(dtmf('#'), Some((941, 1477)));
        assert_eq!(dtmf('A'), None);
    }
}
