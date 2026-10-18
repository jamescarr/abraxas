//! DUEL: turn-based fight against black ICE, or the Shadow (where acceptance wins).

use std::time::Duration;

use rand::RngExt;
use rand::SeedableRng;

use super::{IceGame, IceInput, IceOutcome, Loadout};
use crate::game::danger::DangerParams;
use crate::rng::Rng;

/// Trace added by retreating.
pub const TRACE_ON_RETREAT: f32 = 10.0;
/// Base attack damage range (inclusive).
pub const ATTACK_RANGE: (i32, i32) = (8, 12);
/// Bonus damage from ICEPICK.EXE.
pub const ICEPICK_BONUS: i32 = 6;
/// Bonus damage on the turn after DEFEND.
pub const DEFEND_BONUS: i32 = 4;
/// Damage used for MIRROR before the player has attacked.
pub const DEFAULT_LAST_ATTACK: i32 = 10;
/// Percentage of max strength a Hydra REGROW heals.
pub const REGEN_PCT: i32 = 10;
const MAX_LOG: usize = 40;

/// What the opponent is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuelKind {
    /// Black ICE guarding a VAULT or CORE.
    Black,
    /// The player's Shadow.
    Shadow,
}

/// Which black ICE creature this is. The class is assigned per node by the host generator and
/// drives art, the intent cycle and strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackClass {
    /// `assets/ice-wolf.txt`.
    Wolf,
    /// `assets/ice-eye.txt`.
    Eye,
    /// `assets/ice-hydra.txt`.
    Hydra,
}

impl BlackClass {
    pub const ALL: [BlackClass; 3] = [BlackClass::Wolf, BlackClass::Eye, BlackClass::Hydra];

    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            BlackClass::Wolf => "WOLF",
            BlackClass::Eye => "EYE",
            BlackClass::Hydra => "HYDRA",
        }
    }

    /// The intent cycle this creature runs.
    pub fn cycle(self) -> [Intent; 4] {
        match self {
            BlackClass::Wolf => [Intent::Stalk, Intent::Lunge, Intent::Lunge, Intent::Strike],
            BlackClass::Eye => [Intent::Probe, Intent::Gaze, Intent::Strike, Intent::Charge],
            BlackClass::Hydra => [Intent::Bite, Intent::Bite, Intent::Regrow, Intent::Crush],
        }
    }

    /// Strength as a percentage of the danger class's ICE strength.
    pub fn strength_pct(self) -> i32 {
        match self {
            BlackClass::Wolf => 75,
            BlackClass::Eye => 90,
            BlackClass::Hydra => 90,
        }
    }
}

/// The kind of action the opponent has announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Black ICE: half attack.
    Probe,
    /// Black ICE or Shadow: a full hit.
    Strike,
    /// Black ICE: gathering power for CRUSH.
    Charge,
    /// Black ICE: double attack.
    Crush,
    /// Wolf: circles, waiting to lunge.
    Stalk,
    /// Wolf: one and a half times attack; DEFEND quarters instead of halving.
    Lunge,
    /// Eye: a full attack that DEFEND cannot blunt; a MIRROR reflects it for double.
    Gaze,
    /// Hydra: three quarters attack, three quick bites.
    Bite,
    /// Hydra: heals 15% of its max strength.
    Regrow,
    /// Shadow: hits for your last attack damage.
    Mirror,
    /// Shadow: reaches for you; ACCEPT now to integrate.
    Reach,
}

impl Intent {
    /// Display label.
    pub fn label(self) -> &'static str {
        match self {
            Intent::Probe => "PROBE",
            Intent::Strike => "STRIKE",
            Intent::Charge => "CHARGE",
            Intent::Crush => "CRUSH",
            Intent::Stalk => "STALK",
            Intent::Lunge => "LUNGE",
            Intent::Gaze => "GAZE",
            Intent::Bite => "BITE",
            Intent::Regrow => "REGROW",
            Intent::Mirror => "MIRROR",
            Intent::Reach => "REACH",
        }
    }
}

/// One executed opponent action, drained by the view to drive the attack animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub intent: Intent,
    /// Damage dealt to EGO (after defense), the amount healed for REGROW, else 0.
    pub amount: i32,
    /// True when a MIRROR charge consumed the hit.
    pub reflected: bool,
}

/// The DUEL game state.
#[derive(Debug, Clone)]
pub struct Duel {
    /// Opponent type.
    pub kind: DuelKind,
    /// Opponent strength remaining.
    pub strength: i32,
    /// Opponent starting strength.
    pub max_strength: i32,
    /// The player's EGO, reduced as damage lands.
    pub ego: i32,
    /// True while a MIRROR charge is armed.
    pub mirror_armed: bool,
    cycle: [Intent; 4],
    idx: usize,
    attack: i32,
    danger: u8,
    class: Option<BlackClass>,
    last_attack: Option<i32>,
    defended_last: bool,
    ego_delta: i32,
    hits: Vec<Hit>,
    log: Vec<String>,
    rng: Rng,
    loadout: Loadout,
    outcome: Option<IceOutcome>,
    pending_trace: f32,
}

impl Duel {
    /// Start a duel. `ego` is the player's current EGO. `class` names the black ICE creature
    /// (ignored for the Shadow, whose art comes from the pack).
    pub fn generate(
        rng: &mut Rng,
        params: &DangerParams,
        kind: DuelKind,
        loadout: Loadout,
        ego: i32,
        class: BlackClass,
    ) -> Self {
        let mut own = Rng::seed_from_u64(rng.random());
        let (cycle, strength, class) = match kind {
            DuelKind::Black => (
                class.cycle(),
                params.ice_strength * class.strength_pct() / 100,
                Some(class),
            ),
            DuelKind::Shadow => (
                [Intent::Mirror, Intent::Reach, Intent::Strike, Intent::Reach],
                30 + 10 * i32::from(params.danger),
                None,
            ),
        };
        let idx = own.random_range(0..4usize);
        let mut d = Self {
            kind,
            strength,
            max_strength: strength,
            ego,
            mirror_armed: false,
            cycle,
            idx,
            attack: params.ice_attack,
            danger: params.danger,
            class,
            last_attack: None,
            defended_last: false,
            ego_delta: 0,
            hits: Vec::new(),
            log: Vec::new(),
            rng: own,
            loadout,
            outcome: None,
            pending_trace: 0.0,
        };
        let first = d.current_intent();
        d.push(format!("It intends: {}.", d.describe(first)));
        d
    }

    /// The announced intent the opponent will execute after the player's next action.
    pub fn current_intent(&self) -> Intent {
        self.cycle[self.idx]
    }

    /// The intent after the current one.
    pub fn next_intent(&self) -> Intent {
        self.cycle[(self.idx + 1) % 4]
    }

    /// Damage `intent` would deal right now.
    pub fn intent_damage(&self, intent: Intent) -> i32 {
        match intent {
            Intent::Probe => self.attack / 2,
            Intent::Strike => match self.kind {
                DuelKind::Black => self.attack,
                DuelKind::Shadow => self.base_attack(),
            },
            Intent::Crush => self.attack * 2,
            Intent::Lunge => self.attack * 3 / 2,
            Intent::Gaze => self.attack,
            Intent::Bite => self.attack / 2,
            Intent::Mirror => self.last_attack.unwrap_or(DEFAULT_LAST_ATTACK),
            Intent::Charge | Intent::Reach | Intent::Stalk | Intent::Regrow => 0,
        }
    }

    /// The player's attack without the post-DEFEND bonus: 10, plus ICEPICK.
    fn base_attack(&self) -> i32 {
        DEFAULT_LAST_ATTACK
            + if self.loadout.icepick {
                ICEPICK_BONUS
            } else {
                0
            }
    }

    /// The black ICE creature; `None` for the Shadow (the UI uses the pack art mirrored).
    pub fn class(&self) -> Option<BlackClass> {
        self.class
    }

    /// Total EGO change from this duel so far (zero or negative).
    pub fn ego_delta(&self) -> i32 {
        self.ego_delta
    }

    /// The combat log, oldest first.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Programs after play (a spent MIRROR charge is gone).
    pub fn loadout(&self) -> Loadout {
        self.loadout
    }

    /// Drain trace penalties incurred since the last call.
    pub fn take_trace(&mut self) -> f32 {
        std::mem::take(&mut self.pending_trace)
    }

    /// Executed opponent actions since the last call.
    pub fn take_hits(&mut self) -> Vec<Hit> {
        std::mem::take(&mut self.hits)
    }

    /// Force the announced intent (tests and the view's animation tests).
    #[cfg(test)]
    pub(crate) fn set_intent(&mut self, intent: Intent) {
        self.cycle[self.idx] = intent;
    }

    /// SLEAZE success chance for this danger class.
    pub fn sleaze_chance(&self) -> f64 {
        0.55 - 0.08 * f64::from(self.danger)
    }

    fn describe(&self, intent: Intent) -> String {
        match intent {
            Intent::Charge => "CHARGE (gathering power)".into(),
            Intent::Stalk => "STALK (circling)".into(),
            Intent::Regrow => format!("REGROW (heals {})", self.max_strength * REGEN_PCT / 100),
            Intent::Reach => "REACH (it holds out a hand)".into(),
            other => format!("{} ({})", other.label(), self.intent_damage(other)),
        }
    }

    fn push(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn enemy_down(&mut self) -> bool {
        if self.strength > 0 {
            return false;
        }
        self.strength = 0;
        match self.kind {
            DuelKind::Black => {
                self.push("The ICE collapses.".into());
                self.outcome = Some(IceOutcome::Cleared);
            }
            DuelKind::Shadow => {
                self.push("you cut it away. it grows back.".into());
                self.outcome = Some(IceOutcome::Destroyed);
            }
        }
        true
    }

    fn attack(&mut self) {
        let mut dmg = self.rng.random_range(ATTACK_RANGE.0..=ATTACK_RANGE.1);
        if self.loadout.icepick {
            dmg += ICEPICK_BONUS;
        }
        if self.defended_last {
            dmg += DEFEND_BONUS;
        }
        self.last_attack = Some(dmg);
        self.strength -= dmg;
        self.push(format!("You strike for {dmg}."));
    }

    /// The opponent executes its displayed intent. `multiplier` doubles damage for a refused
    /// ACCEPT; `defending` halves it (quarters a LUNGE, and cannot blunt a GAZE).
    fn enemy_turn(&mut self, multiplier: i32, defending: bool) {
        let intent = self.current_intent();
        let dmg = match (intent, defending) {
            (Intent::Gaze, _) => self.intent_damage(intent) * multiplier,
            (Intent::Lunge, true) => self.intent_damage(intent) * multiplier / 4,
            (_, true) => self.intent_damage(intent) * multiplier / 2,
            _ => self.intent_damage(intent) * multiplier,
        };
        match intent {
            Intent::Charge => self.push("It gathers power.".into()),
            Intent::Stalk => self.push("It circles you.".into()),
            Intent::Reach => self.push("It reaches for you.".into()),
            Intent::Regrow => {
                let heal =
                    (self.max_strength * REGEN_PCT / 100).min(self.max_strength - self.strength);
                self.strength = (self.strength + heal).min(self.max_strength);
                self.push(format!("It regrows a head (+{heal})."));
                self.hits.push(Hit {
                    intent,
                    amount: heal,
                    reflected: false,
                });
            }
            _ => {
                if self.mirror_armed {
                    self.mirror_armed = false;
                    let bounced = if intent == Intent::Gaze { dmg * 2 } else { dmg };
                    self.strength -= bounced;
                    self.push(format!(
                        "{} reflected: {bounced} damage to it.",
                        intent.label()
                    ));
                    self.hits.push(Hit {
                        intent,
                        amount: bounced,
                        reflected: true,
                    });
                } else {
                    self.ego -= dmg;
                    self.ego_delta -= dmg;
                    self.push(format!("{} hits you for {dmg}.", intent.label()));
                    self.hits.push(Hit {
                        intent,
                        amount: dmg,
                        reflected: false,
                    });
                }
            }
        }
        self.idx = (self.idx + 1) % 4;
        let next = self.current_intent();
        self.push(format!("It intends: {}.", self.describe(next)));
    }

    fn finish_turn(&mut self, defended: bool) {
        self.defended_last = defended;
        if self.enemy_down() {
            return;
        }
        if self.ego <= 0 {
            self.outcome = Some(IceOutcome::Failed { trace: 0.0, ego: 0 });
        }
    }

    fn act(&mut self, input: IceInput) {
        let IceInput::Char(c) = input else {
            return;
        };
        {
            match c.to_ascii_uppercase() {
                'A' => {
                    self.attack();
                    if self.enemy_down() {
                        return;
                    }
                    self.enemy_turn(1, false);
                    self.finish_turn(false);
                }
                'S' => match self.kind {
                    DuelKind::Shadow => self.push("you cannot sneak past yourself.".into()),
                    DuelKind::Black => {
                        if self.rng.random_bool(self.sleaze_chance()) {
                            self.push("You slip past the ICE.".into());
                            self.outcome = Some(IceOutcome::Cleared);
                        } else {
                            self.push("The sleaze fails.".into());
                            self.enemy_turn(1, false);
                            self.finish_turn(false);
                        }
                    }
                },
                'D' => {
                    self.push("You brace.".into());
                    self.enemy_turn(1, true);
                    self.finish_turn(true);
                }
                'P' => {
                    if self.loadout.mirror == 0 || self.mirror_armed {
                        self.push("No program ready.".into());
                        return;
                    }
                    self.loadout.mirror -= 1;
                    self.mirror_armed = true;
                    self.push("MIRROR.EXE armed.".into());
                    self.enemy_turn(1, false);
                    self.finish_turn(false);
                }
                'R' => {
                    self.pending_trace += TRACE_ON_RETREAT;
                    self.push("You retreat.".into());
                    self.outcome = Some(IceOutcome::Retreated);
                }
                'X' => match self.kind {
                    DuelKind::Black => self.push("There is nothing to accept.".into()),
                    DuelKind::Shadow if self.current_intent() == Intent::Reach => {
                        self.push("You take its hand.".into());
                        self.outcome = Some(IceOutcome::Integrated);
                    }
                    DuelKind::Shadow => {
                        self.push("You open your arms too soon.".into());
                        self.enemy_turn(2, false);
                        self.finish_turn(false);
                    }
                },
                _ => {}
            }
        }
    }
}

impl IceGame for Duel {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        self.act(input);
    }

    fn tick(&mut self, _dt: Duration) {}

    fn outcome(&self) -> Option<IceOutcome> {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::danger::{IceGrade, params};
    use crate::rng::substream;

    fn duel(seed: u64, d: u8, kind: DuelKind, loadout: Loadout) -> Duel {
        let mut rng = substream(seed, 0, "duel-test", 0);
        // The generic mechanics tests keep the original cycle; class behaviour is tested
        // separately against the real cycles.
        let mut g = Duel::generate(&mut rng, &params(d), kind, loadout, 100, BlackClass::Hydra);
        if kind == DuelKind::Black {
            g.cycle = [Intent::Probe, Intent::Strike, Intent::Charge, Intent::Crush];
        }
        g
    }

    fn at(d: &mut Duel, intent: Intent) {
        d.idx = d.cycle.iter().position(|i| *i == intent).unwrap();
    }

    #[test]
    fn classes_exist_and_label() {
        assert_eq!(BlackClass::Wolf.label(), "WOLF");
        assert_eq!(BlackClass::Eye.label(), "EYE");
        assert_eq!(BlackClass::Hydra.label(), "HYDRA");
        assert_eq!(BlackClass::ALL.len(), 3);
    }

    fn class_duel(seed: u64, d: u8, grade: IceGrade, class: BlackClass, ego: i32) -> Duel {
        let mut rng = substream(seed, 0, "duel-class", 0);
        Duel::generate(
            &mut rng,
            &params(d).graded(grade),
            DuelKind::Black,
            Loadout::default(),
            ego,
            class,
        )
    }

    #[test]
    fn each_class_has_its_own_cycle() {
        assert_eq!(
            BlackClass::Wolf.cycle(),
            [Intent::Stalk, Intent::Lunge, Intent::Lunge, Intent::Strike]
        );
        assert_eq!(
            BlackClass::Eye.cycle(),
            [Intent::Probe, Intent::Gaze, Intent::Strike, Intent::Charge]
        );
        assert_eq!(
            BlackClass::Hydra.cycle(),
            [Intent::Bite, Intent::Bite, Intent::Regrow, Intent::Crush]
        );
        for class in BlackClass::ALL {
            let g = class_duel(1, 3, IceGrade::Standard, class, 100);
            assert_eq!(g.max_strength, 50 * class.strength_pct() / 100);
        }
    }

    #[test]
    fn wolf_lunge_defended_takes_a_quarter() {
        let mut g = class_duel(1, 3, IceGrade::Standard, BlackClass::Wolf, 100);
        assert_eq!(g.intent_damage(Intent::Lunge), 15);
        at(&mut g, Intent::Lunge);
        g.handle(IceInput::Char('d'));
        assert_eq!(
            g.ego,
            97,
            "defended lunge deals a quarter: {}",
            g.log().last().unwrap()
        );
        let hits = g.take_hits();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].intent, Intent::Lunge);
        assert_eq!(hits[0].amount, 3);
        assert!(!hits[0].reflected);
    }

    #[test]
    fn eye_gaze_ignores_defend_but_mirror_reflects_it_double() {
        let mut g = class_duel(1, 3, IceGrade::Standard, BlackClass::Eye, 100);
        at(&mut g, Intent::Gaze);
        g.handle(IceInput::Char('d'));
        assert_eq!(g.ego, 90, "gaze ignores defend");
        // Arm a mirror during a harmless CHARGE so it survives until the next gaze.
        g.loadout.mirror = 1;
        at(&mut g, Intent::Charge);
        g.handle(IceInput::Char('p'));
        assert!(g.mirror_armed);
        at(&mut g, Intent::Gaze);
        let before = g.strength;
        g.handle(IceInput::Char('d'));
        assert_eq!(g.strength, before - 20, "gaze reflected for double");
        assert_eq!(g.ego, 90);
        let hits = g.take_hits();
        assert!(hits.last().unwrap().reflected);
    }

    #[test]
    fn hydra_regrow_heals_and_caps_at_max() {
        let mut g = class_duel(1, 3, IceGrade::Standard, BlackClass::Hydra, 100);
        g.strength = 20;
        at(&mut g, Intent::Regrow);
        g.handle(IceInput::Char('d'));
        assert_eq!(g.strength, 20 + 45 * REGEN_PCT / 100);
        // Near max: capped.
        g.strength = g.max_strength - 1;
        at(&mut g, Intent::Regrow);
        g.handle(IceInput::Char('d'));
        assert_eq!(g.strength, g.max_strength);
        let hits = g.take_hits();
        assert!(
            hits.iter()
                .any(|h| h.intent == Intent::Regrow && h.amount > 0)
        );
    }

    /// The referee: a fixed policy (DEFEND when a LUNGE or CRUSH is announced, else ATTACK)
    /// must beat every class at every grade and danger with no programs and 100 EGO.
    /// This pins class and grade multipliers to a winnable range; tune those constants,
    /// not this test.
    #[test]
    fn fixed_policy_beats_every_class_grade_and_danger() {
        let mut worst_ego = 100;
        for class in BlackClass::ALL {
            for grade in [IceGrade::Soft, IceGrade::Standard, IceGrade::Hardened] {
                for d in 1..=5u8 {
                    for seed in 0..200u64 {
                        let mut g = class_duel(seed, d, grade, class, 100);
                        for _ in 0..1000 {
                            if g.outcome().is_some() {
                                break;
                            }
                            let input = match g.current_intent() {
                                Intent::Crush | Intent::Lunge => IceInput::Char('d'),
                                _ => IceInput::Char('a'),
                            };
                            g.handle(input);
                        }
                        assert_eq!(
                            g.outcome(),
                            Some(IceOutcome::Cleared),
                            "lost: {class:?} {grade:?} D{d} seed {seed} ego {}",
                            g.ego
                        );
                        worst_ego = worst_ego.min(g.ego);
                    }
                }
            }
        }
        assert!(worst_ego > 0, "worst EGO {worst_ego}");
    }

    #[test]
    fn charge_always_precedes_crush_and_cycle_is_complete() {
        for d in 1..=5u8 {
            for seed in 0..500 {
                let mut g = duel(seed, d, DuelKind::Black, Loadout::default());
                let mut seen = Vec::new();
                for _ in 0..8 {
                    seen.push(g.current_intent());
                    g.idx = (g.idx + 1) % 4;
                }
                for w in seen.windows(2) {
                    if w[1] == Intent::Crush {
                        assert_eq!(w[0], Intent::Charge);
                    }
                }
                assert_eq!(
                    g.max_strength,
                    params(d).ice_strength * BlackClass::Hydra.strength_pct() / 100
                );
                assert!(g.class().is_some());
            }
        }
    }

    #[test]
    fn black_ice_damage_values_follow_attack() {
        let g = duel(1, 3, DuelKind::Black, Loadout::default());
        assert_eq!(g.intent_damage(Intent::Probe), 5);
        assert_eq!(g.intent_damage(Intent::Strike), 10);
        assert_eq!(g.intent_damage(Intent::Charge), 0);
        assert_eq!(g.intent_damage(Intent::Crush), 20);
    }

    #[test]
    fn accept_on_reach_integrates() {
        for seed in 0..50 {
            let mut g = duel(seed, 2, DuelKind::Shadow, Loadout::default());
            assert_eq!(g.max_strength, 50);
            assert!(g.class().is_none());
            at(&mut g, Intent::Reach);
            g.handle(IceInput::Char('x'));
            assert_eq!(g.outcome(), Some(IceOutcome::Integrated));
            assert_eq!(g.ego_delta(), 0);
        }
    }

    #[test]
    fn accept_on_strike_takes_double_damage() {
        let mut g = duel(2, 2, DuelKind::Shadow, Loadout::default());
        at(&mut g, Intent::Strike);
        g.handle(IceInput::Char('X'));
        assert!(g.outcome().is_none());
        assert_eq!(g.ego_delta(), -20);
        assert_eq!(g.ego, 80);
        // Next intent after STRIKE is REACH.
        assert_eq!(g.current_intent(), Intent::Reach);
    }

    #[test]
    fn mirror_intent_uses_last_attack_and_defaults_to_ten() {
        let mut g = duel(3, 1, DuelKind::Shadow, Loadout::default());
        at(&mut g, Intent::Mirror);
        assert_eq!(g.intent_damage(Intent::Mirror), 10);
        g.handle(IceInput::Char('a'));
        let last = g.last_attack.unwrap();
        assert!((8..=12).contains(&last));
        assert_eq!(g.ego_delta(), -last);
    }

    #[test]
    fn shadow_at_zero_strength_is_destroyed() {
        let mut g = duel(4, 1, DuelKind::Shadow, Loadout::default());
        g.strength = 1;
        g.handle(IceInput::Char('A'));
        assert_eq!(g.outcome(), Some(IceOutcome::Destroyed));
        assert_eq!(g.ego_delta(), 0, "dead opponents do not strike back");
    }

    #[test]
    fn sleaze_never_works_on_the_shadow_and_costs_nothing() {
        for seed in 0..500 {
            let mut g = duel(seed, 1, DuelKind::Shadow, Loadout::default());
            let before = g.current_intent();
            for _ in 0..10 {
                g.handle(IceInput::Char('S'));
            }
            assert!(g.outcome().is_none());
            assert_eq!(g.current_intent(), before);
            assert_eq!(g.log().last().unwrap(), "you cannot sneak past yourself.");
        }
    }

    #[test]
    fn sleaze_against_black_ice_succeeds_at_roughly_the_stated_chance() {
        let mut wins = 0;
        for seed in 0..2000 {
            let mut g = duel(seed, 1, DuelKind::Black, Loadout::default());
            g.handle(IceInput::Char('s'));
            if g.outcome() == Some(IceOutcome::Cleared) {
                wins += 1;
            }
        }
        // 0.55 - 0.08 = 0.47
        assert!((850..1030).contains(&wins), "{wins}");
    }

    #[test]
    fn defend_halves_damage_and_boosts_next_attack() {
        let mut g = duel(5, 3, DuelKind::Black, Loadout::default());
        at(&mut g, Intent::Strike);
        g.handle(IceInput::Char('D'));
        assert_eq!(g.ego_delta(), -5);
        g.handle(IceInput::Char('A'));
        let last = g.last_attack.unwrap();
        assert!((12..=16).contains(&last));
        // The bonus does not carry to a second attack.
        g.handle(IceInput::Char('A'));
        assert!((8..=12).contains(&g.last_attack.unwrap()));
    }

    #[test]
    fn icepick_adds_six() {
        let mut g = duel(
            6,
            1,
            DuelKind::Black,
            Loadout {
                icepick: true,
                ..Loadout::default()
            },
        );
        g.handle(IceInput::Char('A'));
        assert!((14..=18).contains(&g.last_attack.unwrap()));
    }

    #[test]
    fn mirror_reflects_the_next_hit_and_spends_a_charge() {
        let mut g = duel(
            7,
            2,
            DuelKind::Black,
            Loadout {
                mirror: 1,
                ..Loadout::default()
            },
        );
        at(&mut g, Intent::Probe);
        // P arms mirror, then the ICE's PROBE (4) is reflected.
        g.handle(IceInput::Char('P'));
        assert_eq!(g.loadout().mirror, 0);
        assert_eq!(g.ego_delta(), 0);
        assert_eq!(g.strength, g.max_strength - 4);
        assert!(!g.mirror_armed);
        // No charge left: nothing happens, no turn consumed.
        let intent = g.current_intent();
        g.handle(IceInput::Char('P'));
        assert_eq!(g.current_intent(), intent);
    }

    #[test]
    fn mirror_waits_through_non_damaging_intents() {
        let mut g = duel(
            8,
            2,
            DuelKind::Black,
            Loadout {
                mirror: 1,
                ..Loadout::default()
            },
        );
        at(&mut g, Intent::Charge);
        g.handle(IceInput::Char('P'));
        assert!(g.mirror_armed);
        assert_eq!(g.current_intent(), Intent::Crush);
        g.handle(IceInput::Char('D'));
        assert!(!g.mirror_armed);
        assert_eq!(g.ego_delta(), 0);
        assert_eq!(g.strength, g.max_strength - 8);
    }

    #[test]
    fn ego_zero_fails_the_duel() {
        let mut rng = substream(1, 0, "duel-test", 0);
        let mut g = Duel::generate(
            &mut rng,
            &params(5),
            DuelKind::Black,
            Loadout::default(),
            5,
            BlackClass::Hydra,
        );
        g.cycle = [Intent::Probe, Intent::Strike, Intent::Charge, Intent::Crush];
        at(&mut g, Intent::Crush);
        g.handle(IceInput::Char('D'));
        assert_eq!(g.outcome(), Some(IceOutcome::Failed { trace: 0.0, ego: 0 }));
        assert_eq!(g.ego_delta(), -14);
    }

    #[test]
    fn retreat_reports_trace_once() {
        let mut g = duel(9, 2, DuelKind::Black, Loadout::default());
        g.handle(IceInput::Char('R'));
        assert_eq!(g.outcome(), Some(IceOutcome::Retreated));
        assert_eq!(g.take_trace(), 10.0);
        assert_eq!(g.take_trace(), 0.0);
    }

    #[test]
    fn killing_black_ice_clears() {
        let mut g = duel(10, 1, DuelKind::Black, Loadout::default());
        g.strength = 5;
        g.handle(IceInput::Char('A'));
        assert_eq!(g.outcome(), Some(IceOutcome::Cleared));
    }

    #[test]
    fn accept_does_nothing_against_black_ice() {
        let mut g = duel(11, 1, DuelKind::Black, Loadout::default());
        let i = g.current_intent();
        g.handle(IceInput::Char('X'));
        assert_eq!(g.current_intent(), i);
        assert!(g.outcome().is_none());
    }
}
