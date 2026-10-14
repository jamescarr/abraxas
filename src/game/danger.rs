//! Danger classes and the per-class parameter table.

use crate::pack::model::ExitKind;

/// Per-danger-class parameters for hosts and mini-games.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DangerParams {
    pub danger: u8,
    /// Nodes in a host graph (an anchor's `host.nodes` overrides this).
    pub nodes: u8,
    pub wordlock_len: usize,
    pub wordlock_words: usize,
    pub breach_grid: usize,
    pub breach_buffer: usize,
    pub breach_daemons: usize,
    pub breach_secs: u32,
    pub phreak_start: usize,
    pub phreak_rounds: usize,
    pub phreak_tone_ms: u32,
    pub router_w: usize,
    pub router_h: usize,
    /// Trace percent per 10 seconds, shell time.
    pub trace_per_10s: f32,
    pub ice_strength: i32,
    pub ice_attack: i32,
}

const TABLE: [DangerParams; 5] = [
    DangerParams {
        danger: 1,
        nodes: 4,
        wordlock_len: 5,
        wordlock_words: 8,
        breach_grid: 5,
        breach_buffer: 6,
        breach_daemons: 1,
        breach_secs: 40,
        phreak_start: 3,
        phreak_rounds: 3,
        phreak_tone_ms: 450,
        router_w: 7,
        router_h: 5,
        trace_per_10s: 2.0,
        ice_strength: 30,
        ice_attack: 6,
    },
    DangerParams {
        danger: 2,
        nodes: 5,
        wordlock_len: 6,
        wordlock_words: 10,
        breach_grid: 5,
        breach_buffer: 6,
        breach_daemons: 2,
        breach_secs: 35,
        phreak_start: 3,
        phreak_rounds: 4,
        phreak_tone_ms: 400,
        router_w: 7,
        router_h: 5,
        trace_per_10s: 3.0,
        ice_strength: 40,
        ice_attack: 8,
    },
    DangerParams {
        danger: 3,
        nodes: 6,
        wordlock_len: 6,
        wordlock_words: 11,
        breach_grid: 6,
        breach_buffer: 7,
        breach_daemons: 2,
        breach_secs: 30,
        phreak_start: 4,
        phreak_rounds: 4,
        phreak_tone_ms: 350,
        router_w: 8,
        router_h: 5,
        trace_per_10s: 4.0,
        ice_strength: 50,
        ice_attack: 10,
    },
    DangerParams {
        danger: 4,
        nodes: 7,
        wordlock_len: 7,
        wordlock_words: 12,
        breach_grid: 6,
        breach_buffer: 7,
        breach_daemons: 3,
        breach_secs: 25,
        phreak_start: 4,
        phreak_rounds: 5,
        phreak_tone_ms: 325,
        router_w: 9,
        router_h: 6,
        trace_per_10s: 5.0,
        ice_strength: 60,
        ice_attack: 12,
    },
    DangerParams {
        danger: 5,
        nodes: 8,
        wordlock_len: 8,
        wordlock_words: 14,
        breach_grid: 6,
        breach_buffer: 8,
        breach_daemons: 3,
        breach_secs: 20,
        phreak_start: 5,
        phreak_rounds: 5,
        phreak_tone_ms: 300,
        router_w: 9,
        router_h: 6,
        trace_per_10s: 6.0,
        ice_strength: 70,
        ice_attack: 14,
    },
];

/// Parameters for a danger class; the class is clamped to 1..=5.
pub fn params(danger: u8) -> DangerParams {
    TABLE[usize::from(danger.clamp(1, 5)) - 1]
}

/// `clamp(1 + depth / 4, 1, 5)`.
pub fn base_danger(depth: u32) -> u8 {
    (1 + depth / 4).clamp(1, 5) as u8
}

/// The danger of a run: the level's base danger shifted by the exit that led here.
/// Calm exits lower it, deep exits raise it, mirror and none leave it.
pub fn run_danger(base: u8, exit: Option<ExitKind>) -> u8 {
    let modifier: i8 = match exit {
        Some(ExitKind::Calm) => -1,
        Some(ExitKind::Deep) => 1,
        Some(ExitKind::Mirror) | None => 0,
    };
    (i16::from(base) + i16::from(modifier)).clamp(1, 5) as u8
}

/// KB awarded for completing a door: `round(32 x run_danger x mult)`.
pub fn door_reward(run_danger: u8, exit: Option<ExitKind>) -> u32 {
    let mult = match exit {
        Some(ExitKind::Calm) => 0.75,
        Some(ExitKind::Deep) => 1.5,
        Some(ExitKind::Mirror) | None => 1.0,
    };
    (32.0 * f64::from(run_danger.clamp(1, 5)) * mult).round() as u32
}

/// The level-card descriptor line for a danger class.
pub fn descriptor(danger: u8, shadow_presence: bool) -> String {
    let base = match danger.clamp(1, 5) {
        1 => "Safe | Secure | Minimal entity count",
        2 => "Safe | Unstable | Low entity count",
        3 => "Unsafe | Unstable | Medium entity count",
        4 => "Unsafe | Hostile | High entity count",
        _ => "Deadzone | Hostile | Entity-dense",
    };
    let base = base.replace('|', "\u{2502}");
    if shadow_presence {
        format!("{base} \u{2502} Shadow presence")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_danger_follows_depth_and_clamps() {
        assert_eq!(base_danger(0), 1);
        assert_eq!(base_danger(3), 1);
        assert_eq!(base_danger(4), 2);
        assert_eq!(base_danger(15), 4);
        assert_eq!(base_danger(16), 5);
        assert_eq!(base_danger(500), 5);
    }

    #[test]
    fn run_danger_applies_exit_modifier_inside_bounds() {
        assert_eq!(run_danger(1, Some(ExitKind::Calm)), 1);
        assert_eq!(run_danger(5, Some(ExitKind::Deep)), 5);
        assert_eq!(run_danger(3, Some(ExitKind::Calm)), 2);
        assert_eq!(run_danger(3, Some(ExitKind::Deep)), 4);
        assert_eq!(run_danger(3, Some(ExitKind::Mirror)), 3);
        assert_eq!(run_danger(3, None), 3);
    }

    #[test]
    fn door_reward_matches_formula() {
        assert_eq!(door_reward(1, None), 32);
        assert_eq!(door_reward(2, Some(ExitKind::Calm)), 48);
        assert_eq!(door_reward(3, Some(ExitKind::Deep)), 144);
        assert_eq!(door_reward(5, Some(ExitKind::Mirror)), 160);
        assert_eq!(door_reward(1, Some(ExitKind::Calm)), 24);
    }

    #[test]
    fn table_is_indexed_by_class() {
        for d in 1..=5u8 {
            assert_eq!(params(d).danger, d);
        }
        assert_eq!(params(0).danger, 1);
        assert_eq!(params(9).danger, 5);
        assert_eq!(params(4).wordlock_len, 7);
        assert_eq!(params(2).wordlock_words, 10);
    }
}
