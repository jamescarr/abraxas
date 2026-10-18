//! Level seeds: everything the engine decides about a level before any AI call, and the
//! per-level program offers.

use rand::RngExt;
use rand::seq::{IndexedRandom, SliceRandom};

use crate::game::danger;
use crate::game::programs::Program;
use crate::game::state::{Level, LevelSeed, RunState};
use crate::pack::model::{ObjectiveKind, StoryPack};
use crate::rng::substream;

/// Derive the seed for `depth`. Pure function of the run seed, the pack and (for the focus rule)
/// the previous level's seed when it is still in `run.levels`.
pub fn level_seed(run: &RunState, pack: &StoryPack, depth: u32) -> LevelSeed {
    level_seed_for(run.seed, previous_focus(run, depth).as_deref(), pack, depth)
}

fn previous_focus(run: &RunState, depth: u32) -> Option<String> {
    depth
        .checked_sub(1)
        .and_then(|d| run.levels.get(&d))
        .and_then(|l| l.seed.focus.clone())
}

fn level_seed_for(
    run_seed: u64,
    previous_focus: Option<&str>,
    pack: &StoryPack,
    depth: u32,
) -> LevelSeed {
    let anchor = pack.anchor_at(depth);
    let anchor_host = anchor.and_then(|a| a.anchor.host.as_ref());
    let act_id = pack
        .act_for_depth(depth)
        .map(|a| a.id.to_string())
        .unwrap_or_default();

    let base_danger = anchor
        .and_then(|a| a.anchor.danger)
        .unwrap_or_else(|| danger::base_danger(depth))
        .clamp(1, 5);

    let omen = substream(run_seed, depth, "omen", 0).random_range(1..=64u8);

    let motifs: Vec<String> = pack
        .meta
        .motifs
        .sample(&mut substream(run_seed, depth, "motifs", 0), 2)
        .cloned()
        .collect();

    let room = pack
        .meta
        .room_kinds
        .choose(&mut substream(run_seed, depth, "room", 0))
        .cloned()
        .unwrap_or_default();

    let focus = match anchor.and_then(|a| a.anchor.focus.clone()) {
        Some(id) => Some(id),
        None => {
            let previous = previous_focus;
            let mut candidates: Vec<&str> =
                pack.focusable_at(depth).map(|c| c.id.as_str()).collect();
            if candidates.len() >= 2 {
                candidates.retain(|id| Some(*id) != previous);
            }
            candidates
                .choose(&mut substream(run_seed, depth, "focus", 0))
                .map(|id| (*id).to_string())
        }
    };

    let objective_kind = match anchor_host.and_then(|h| h.objective_kind) {
        Some(kind) => kind,
        None => {
            let roll = substream(run_seed, depth, "objective", 0).random_range(0..100u32);
            match roll {
                0..60 => ObjectiveKind::Retrieve,
                60..80 => ObjectiveKind::Plant,
                _ => ObjectiveKind::Crash,
            }
        }
    };

    let target_daemon = (objective_kind == ObjectiveKind::Crash).then(|| {
        anchor_host
            .and_then(|h| h.target_daemon.clone())
            .or_else(|| {
                pack.pool
                    .daemons
                    .choose(&mut substream(run_seed, depth, "daemon", 0))
                    .cloned()
            })
            .unwrap_or_else(|| "daemon".to_string())
    });

    LevelSeed {
        depth,
        act_id,
        anchor: anchor.map(|a| a.file.clone()),
        base_danger,
        omen,
        motifs,
        room,
        focus,
        objective_kind,
        target_daemon,
        wordlock_len: danger::params(base_danger).wordlock_len,
    }
}

/// The two programs a level's file library offers: the anchor's locked list when it has one,
/// otherwise two distinct programs the BBS sells (`Program::sold_on_bbs`) picked by
/// `substream(.., "programs")`. Owned persistent programs are filtered out at display time, not
/// here.
pub fn programs_for_sale(run: &RunState, pack: &StoryPack, depth: u32) -> Vec<Program> {
    programs_for_sale_for(run.seed, pack, depth)
}

fn programs_for_sale_for(run_seed: u64, pack: &StoryPack, depth: u32) -> Vec<Program> {
    if let Some(entry) = pack.anchor_at(depth)
        && !entry.anchor.programs.is_empty()
    {
        return entry.anchor.programs.clone();
    }
    let mut all: Vec<Program> = Program::ALL
        .into_iter()
        .filter(|p| p.sold_on_bbs())
        .collect();
    all.shuffle(&mut substream(run_seed, depth, "programs", 0));
    all.truncate(2);
    all
}

/// The level at `depth`, creating its engine-decided skeleton (seed and programs) when absent.
pub fn ensure_level<'a>(run: &'a mut RunState, pack: &StoryPack, depth: u32) -> &'a mut Level {
    let previous = if run.levels.contains_key(&depth) {
        None
    } else {
        previous_focus(run, depth)
    };
    let run_seed = run.seed;
    run.levels.entry(depth).or_insert_with(|| {
        Level::new(
            level_seed_for(run_seed, previous.as_deref(), pack, depth),
            programs_for_sale_for(run_seed, pack, depth),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack;

    fn run(seed: u64) -> RunState {
        RunState::new("Tester", "T", "a dream", seed, "rollover", None)
    }

    #[test]
    fn seed_is_deterministic_and_in_range() {
        let pack = pack::embedded().unwrap();
        let r = run(42);
        for depth in 0..40 {
            let a = level_seed(&r, &pack, depth);
            assert_eq!(a, level_seed(&r, &pack, depth));
            assert!((1..=64).contains(&a.omen));
            assert!((1..=5).contains(&a.base_danger));
            assert_eq!(a.motifs.len(), 2);
            assert_ne!(a.motifs[0], a.motifs[1]);
            assert_eq!(a.wordlock_len, danger::params(a.base_danger).wordlock_len);
        }
    }

    #[test]
    fn different_run_seeds_dream_different_levels() {
        let pack = pack::embedded().unwrap();
        let omens = |seed| {
            (5..15)
                .map(|d| level_seed(&run(seed), &pack, d).omen)
                .collect::<Vec<_>>()
        };
        assert_ne!(omens(1), omens(2));
    }

    #[test]
    fn anchor_locks_danger_focus_and_objective() {
        let pack = pack::embedded().unwrap();
        let r = run(7);
        let l3 = level_seed(&r, &pack, 3);
        assert_eq!(l3.anchor.as_deref(), Some("anchors/03-black-sun.toml"));
        assert_eq!(l3.base_danger, 2);
        assert_eq!(l3.focus.as_deref(), Some("shadow"));
        assert_eq!(l3.objective_kind, ObjectiveKind::Retrieve);
        let l15 = level_seed(&r, &pack, 15);
        assert_eq!(l15.objective_kind, ObjectiveKind::Crash);
        assert_eq!(l15.target_daemon.as_deref(), Some("pleroma"));
    }

    #[test]
    fn focus_never_repeats_when_two_or_more_qualify() {
        let pack = pack::embedded().unwrap();
        for seed in 0..50u64 {
            let mut r = run(seed);
            let mut previous: Option<String> = None;
            for depth in 16..30 {
                let level = ensure_level(&mut r, &pack, depth).clone();
                assert!(level.seed.focus.is_some());
                assert_ne!(level.seed.focus, previous, "seed {seed} depth {depth}");
                previous = level.seed.focus.clone();
                r.levels.retain(|d, _| *d == depth);
            }
        }
    }

    #[test]
    fn crash_levels_always_name_a_daemon_from_the_pool() {
        let pack = pack::embedded().unwrap();
        let mut seen_crash = false;
        for seed in 0..200u64 {
            let s = level_seed(&run(seed), &pack, 20);
            assert_eq!(
                s.target_daemon.is_some(),
                s.objective_kind == ObjectiveKind::Crash
            );
            if let Some(d) = &s.target_daemon {
                seen_crash = true;
                assert!(pack.pool.daemons.contains(d));
            }
        }
        assert!(seen_crash);
    }

    #[test]
    fn objective_mix_is_roughly_60_20_20() {
        let pack = pack::embedded().unwrap();
        let mut counts = [0u32; 3];
        for seed in 0..2000u64 {
            match level_seed(&run(seed), &pack, 20).objective_kind {
                ObjectiveKind::Retrieve => counts[0] += 1,
                ObjectiveKind::Plant => counts[1] += 1,
                ObjectiveKind::Crash => counts[2] += 1,
            }
        }
        assert!((1100..1300).contains(&counts[0]), "{counts:?}");
        assert!((300..500).contains(&counts[1]), "{counts:?}");
        assert!((300..500).contains(&counts[2]), "{counts:?}");
    }

    #[test]
    fn programs_for_sale_are_two_distinct_unless_the_anchor_locks_them() {
        let pack = pack::embedded().unwrap();
        let r = run(9);
        let locked = programs_for_sale(&r, &pack, 0);
        assert_eq!(locked, vec![Program::Decrypt, Program::Mandala]);
        for depth in 1..30 {
            let p = programs_for_sale(&r, &pack, depth);
            if pack.anchor_at(depth).is_none() {
                assert_eq!(p.len(), 2);
                assert_ne!(p[0], p[1]);
            }
        }
    }

    #[test]
    fn the_bbs_never_offers_irc_only_programs() {
        let pack = pack::embedded().unwrap();
        for depth in 0..40 {
            let offered = programs_for_sale_for(42, &pack, depth);
            assert!(
                offered.iter().all(|p| p.sold_on_bbs()),
                "depth {depth}: {offered:?}"
            );
            if pack.anchor_at(depth).is_none() {
                assert_eq!(offered.len(), 2, "depth {depth}");
            }
        }
    }

    #[test]
    fn ensure_level_is_idempotent() {
        let pack = pack::embedded().unwrap();
        let mut r = run(5);
        let first = ensure_level(&mut r, &pack, 4).seed.clone();
        ensure_level(&mut r, &pack, 4).interlude = Some("kept".into());
        assert_eq!(ensure_level(&mut r, &pack, 4).seed, first);
        assert_eq!(
            ensure_level(&mut r, &pack, 4).interlude.as_deref(),
            Some("kept")
        );
    }
}
