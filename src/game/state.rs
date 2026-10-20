//! The run state: everything that is saved, plus the engine-side shapes of generated content.
//!
//! `Place` and `Passage` are what the director (AI or offline) produces after validation; the
//! rest of the game only ever sees these types, never raw AI output.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::game::chronicle::{ChosenOption, Chronicle};
use crate::game::programs::{self, Deck, Program};
use crate::pack::model::{
    ChoiceDef, ExitDef, FileDef, HostOs, ObjectiveIce, ObjectiveKind, OptionDef, PostDef,
    StoryPack, VisionKind,
};
use crate::text::shadow_handle;

/// Save file schema version.
pub const SAVE_FORMAT: u32 = 1;
pub const EGO_MAX: i32 = 100;
pub const EGO_START: i32 = 100;
pub const RATIO_START_KB: u32 = 96;
pub const INTEGRATION_MIN: i32 = -10;
pub const INTEGRATION_MAX: i32 = 10;
/// EGO restored on every level arrival.
pub const EGO_ON_ARRIVAL: i32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFile {
    pub format: u32,
    pub run: RunState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub ego: i32,
    pub ratio_kb: u32,
    pub integration: i32,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            ego: EGO_START,
            ratio_kb: RATIO_START_KB,
            integration: 0,
        }
    }
}

impl Stats {
    /// Add (or subtract) EGO, clamped to `0..=EGO_MAX`.
    pub fn add_ego(&mut self, delta: i32) {
        self.ego = self.ego.saturating_add(delta).clamp(0, EGO_MAX);
    }

    /// Add to INTEGRATION, clamped to `-10..=10`.
    pub fn add_integration(&mut self, delta: i32) {
        self.integration = self
            .integration
            .saturating_add(delta)
            .clamp(INTEGRATION_MIN, INTEGRATION_MAX);
    }

    pub fn is_dead(&self) -> bool {
        self.ego <= 0
    }
}

/// Where in the level loop the run is. Used to resume after a quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Just arrived; the level card is (or will be) showing.
    Arrival,
    /// On the BBS menu. `door_done` once the objective succeeded; `choice` is the option index
    /// picked at the choice screen.
    Hub {
        door_done: bool,
        choice: Option<usize>,
    },
    /// Falling to the next level through `exit` (index into the passage's exits).
    Descending { exit: usize },
    /// An anchor ending is being played.
    Ending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPost {
    pub depth: u32,
    pub subject: String,
    pub body: String,
}

/// What the player took from fservs: cracked bots (`"{depth}:{bot}"`) and pulled files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrcProgress {
    pub cracked: BTreeSet<String>,
    pub files: Vec<IrcFile>,
}

impl IrcProgress {
    /// True when `file` from fserv `bot` at `depth` was already pulled.
    pub fn has_file(&self, depth: u32, bot: &str, file: &str) -> bool {
        self.files
            .iter()
            .any(|f| f.depth == depth && f.bot == bot && f.file.name == file)
    }
}

/// A file pulled from an fserv, kept for `/files` and `/read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrcFile {
    pub depth: u32,
    pub bot: String,
    pub file: FileDef,
}

/// The hidden line's backdoor on one board. `Open` counts TAP misses (`BACKDOOR_MISSES` lock it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backdoor {
    Open { misses: u8 },
    Planted,
    Locked,
}

impl Default for Backdoor {
    fn default() -> Self {
        Backdoor::Open { misses: 0 }
    }
}

/// TAP misses that lock the backdoor on a board.
pub const BACKDOOR_MISSES: u8 = 3;

/// Where a generated part came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenSource {
    Authored,
    Ai { model: String, millis: u64 },
    Offline { reason: String },
}

impl GenSource {
    /// Short tag for the `--dev` footer and `pack dream` output, e.g. `ai qwen3.8 41.2s`.
    pub fn tag(&self) -> String {
        match self {
            GenSource::Authored => "authored".to_string(),
            GenSource::Ai { model, millis } => {
                format!("ai {model} {:.1}s", *millis as f64 / 1000.0)
            }
            GenSource::Offline { reason } if reason.is_empty() => "offline".to_string(),
            GenSource::Offline { reason } => format!("offline: {reason}"),
        }
    }
}

/// Everything the engine decides about a level before any AI call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelSeed {
    pub depth: u32,
    /// Act id, or `ENDLESS_ACT_ID` beyond the authored acts.
    pub act_id: String,
    /// File name of the anchor at this depth, if any.
    pub anchor: Option<String>,
    pub base_danger: u8,
    /// I Ching hexagram 1..=64.
    pub omen: u8,
    pub motifs: Vec<String>,
    pub room: String,
    /// Cast id of the archetype in focus.
    pub focus: Option<String>,
    pub objective_kind: ObjectiveKind,
    /// Crash objectives only.
    pub target_daemon: Option<String>,
    pub wordlock_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sysop {
    pub handle: String,
    /// Set when the sysop is a cast member.
    pub cast_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub name: String,
    pub os: HostOs,
    pub banner: String,
    /// One sentence naming the job.
    pub objective: String,
    /// Retrieve: the file to fetch. Plant: `PAYLOAD.PIF`. Crash: empty.
    pub target_file: String,
    /// Anchor override of what guards the objective node.
    pub objective_ice: Option<ObjectiveIce>,
    /// Anchor override of the node count (3..=8).
    pub nodes: Option<u8>,
    /// Display names only; command ids are generated by the host graph. Always 8.
    pub node_names: Vec<String>,
    /// Always 3.
    pub ice_names: Vec<String>,
}

/// A validated level: its board, files, host dressing and WORDLOCK words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub title: String,
    pub bbs_name: String,
    pub tagline: String,
    pub level_card: String,
    pub sysop: Sysop,
    pub host: HostInfo,
    /// Includes the engine's `re: front door` password-clue post as the last entry.
    pub posts: Vec<PostDef>,
    pub text_files: Vec<FileDef>,
    pub host_files: Vec<FileDef>,
    /// Uppercase candidates of exactly `LevelSeed::wordlock_len` letters; always contains the
    /// password.
    pub words: Vec<String>,
    pub password: String,
}

impl Place {
    /// Keys recorded in `RunState::used_pool` when this place is adopted, so the offline
    /// generator avoids repeating titles, BBS names and posts within a run. The same key format
    /// is what `director::offline` checks when it picks from the pool.
    pub fn pool_keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(2 + self.posts.len());
        keys.push(format!("title:{}", self.title));
        keys.push(format!("bbs:{}", self.bbs_name));
        keys.extend(
            self.posts
                .iter()
                .map(|p| format!("post:{}|{}", p.from, p.subject)),
        );
        keys
    }
}

/// A validated passage: the payload beat, the choice, the exits and the descent dressing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    pub beat: String,
    /// Exactly 3 options, one per stance.
    pub choice: ChoiceDef,
    /// Exactly 3 exits: calm, deep, mirror.
    pub exits: Vec<ExitDef>,
    pub vision: VisionKind,
    /// Exactly 3 lines.
    pub captions: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub seed: LevelSeed,
    pub place: Option<Place>,
    pub place_source: Option<GenSource>,
    pub passage: Option<Passage>,
    pub passage_source: Option<GenSource>,
    pub interlude: Option<String>,
    pub interlude_source: Option<GenSource>,
    /// The two programs offered in this level's file library (anchors may lock them).
    pub programs_for_sale: Vec<Program>,
}

impl Level {
    pub fn new(seed: LevelSeed, programs_for_sale: Vec<Program>) -> Self {
        Self {
            seed,
            place: None,
            place_source: None,
            passage: None,
            passage_source: None,
            interlude: None,
            interlude_source: None,
            programs_for_sale,
        }
    }
}

/// The in-world clock: time of day and the date the status bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockReading {
    /// Seconds since midnight.
    pub seconds: u32,
    /// `12/31/99`, `01/01/00`, or `01/01/19100` in the endless levels.
    pub date: String,
}

impl ClockReading {
    pub fn hms(&self) -> String {
        format!(
            "{:02}:{:02}:{:02}",
            self.seconds / 3600,
            (self.seconds / 60) % 60,
            self.seconds % 60
        )
    }
}

/// Parse `HH:MM:SS` into seconds since midnight; falls back to 23:00:00.
pub fn parse_clock(text: &str) -> u32 {
    let mut parts = text.split(':').map(|p| p.trim().parse::<u32>().ok());
    match (
        parts.next().flatten(),
        parts.next().flatten(),
        parts.next().flatten(),
    ) {
        (Some(h), Some(m), s) if h < 24 && m < 60 => h * 3600 + m * 60 + s.unwrap_or(0).min(59),
        _ => 23 * 3600,
    }
}

/// Everything saved about one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunState {
    pub handle: String,
    pub real_name: String,
    pub heard_from: String,
    pub seed: u64,
    pub pack_id: String,
    /// `None` = the embedded pack.
    pub pack_dir: Option<PathBuf>,
    pub depth: u32,
    pub stats: Stats,
    pub deck: Deck,
    pub chronicle: Chronicle,
    /// Only the current depth and depth + 1 are kept; older levels live on as chronicle entries.
    pub levels: BTreeMap<u32, Level>,
    pub phase: Phase,
    /// The exit used to arrive at the current level (drives `run_danger`).
    pub exit_taken: Option<ExitDef>,
    /// Incremented on every dump shock; host generation uses it as the attempt counter.
    pub door_attempts: u32,
    pub flags: BTreeSet<String>,
    pub player_posts: Vec<PlayerPost>,
    /// Pool entries already used, so the offline generator avoids repeats.
    pub used_pool: BTreeSet<String>,
    /// What the player took from IRC fservs.
    #[serde(default)]
    pub irc: IrcProgress,
    /// Hidden-line backdoors by depth.
    #[serde(default)]
    pub backdoors: BTreeMap<u32, Backdoor>,
    /// Id of the ending reached, once there is one.
    pub ending: Option<String>,
    pub completed: bool,
}

impl RunState {
    pub fn new(
        handle: impl Into<String>,
        real_name: impl Into<String>,
        heard_from: impl Into<String>,
        seed: u64,
        pack_id: impl Into<String>,
        pack_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            handle: handle.into(),
            real_name: real_name.into(),
            heard_from: heard_from.into(),
            seed,
            pack_id: pack_id.into(),
            pack_dir,
            depth: 0,
            stats: Stats::default(),
            deck: Deck::new(),
            chronicle: Chronicle::default(),
            levels: BTreeMap::new(),
            phase: Phase::Arrival,
            exit_taken: None,
            door_attempts: 0,
            flags: BTreeSet::new(),
            player_posts: Vec::new(),
            used_pool: BTreeSet::new(),
            irc: IrcProgress::default(),
            backdoors: BTreeMap::new(),
            ending: None,
            completed: false,
        }
    }

    /// The Shadow's handle for this player.
    pub fn shadow(&self) -> String {
        shadow_handle(&self.handle)
    }

    pub fn level(&self) -> Option<&Level> {
        self.levels.get(&self.depth)
    }

    pub fn level_mut(&mut self) -> Option<&mut Level> {
        self.levels.get_mut(&self.depth)
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }

    pub fn deck_owns(&self, program: Program) -> bool {
        programs::owns(&self.deck, program)
    }

    /// Buy a program for its cost. Returns false (nothing changes) when the ratio is too low or a
    /// persistent program is already owned.
    pub fn buy(&mut self, program: Program) -> bool {
        if self.stats.ratio_kb < program.cost_kb()
            || (program.is_persistent() && self.deck_owns(program))
        {
            return false;
        }
        if programs::add(&mut self.deck, program) {
            self.stats.ratio_kb -= program.cost_kb();
            true
        } else {
            false
        }
    }

    /// Move to `depth`: +10 EGO (capped), reset door attempts, open the hub. `exit_taken` is kept
    /// because it set this level's danger. Older levels are dropped.
    pub fn arrive(&mut self, depth: u32) {
        self.depth = depth;
        self.stats.add_ego(EGO_ON_ARRIVAL);
        self.door_attempts = 0;
        self.phase = Phase::Arrival;
        self.prune_levels();
    }

    /// Keep only the current depth and depth + 1.
    pub fn prune_levels(&mut self) {
        let keep = self.depth;
        self.levels.retain(|d, _| *d == keep || *d == keep + 1);
        self.backdoors.retain(|d, _| *d >= keep);
    }

    /// Apply the player's story choice at the current depth: INTEGRATION by stance, flags, a
    /// chronicle entry, and the hub phase. Returns false when the level, its place/passage, or the
    /// option does not exist.
    pub fn apply_choice(&mut self, pack: &StoryPack, option_index: usize) -> bool {
        let depth = self.depth;
        let Some(level) = self.levels.get(&depth) else {
            return false;
        };
        let (Some(place), Some(passage)) = (&level.place, &level.passage) else {
            return false;
        };
        let Some(option) = passage.choice.options.get(option_index) else {
            return false;
        };
        let option: OptionDef = option.clone();
        let act = pack.act_for_depth(depth);
        let entry = crate::game::chronicle::ChronicleEntry {
            depth,
            act_id: level.seed.act_id.clone(),
            act_name: act.map(|a| a.name.to_string()).unwrap_or_default(),
            title: place.title.clone(),
            class: level.seed.base_danger,
            omen: level.seed.omen,
            level_card: place.level_card.clone(),
            summary: passage.summary.clone(),
            choice: Some(ChosenOption {
                text: option.text.clone(),
                stance: option.stance,
            }),
            exit: None,
        };
        self.stats
            .add_integration(option.stance.integration_delta());
        self.flags.extend(option.flags.iter().cloned());
        self.chronicle.upsert(entry);
        self.phase = Phase::Hub {
            door_done: true,
            choice: Some(option_index),
        };
        true
    }

    /// Take exit `exit_index` of the current passage: records it and starts the descent.
    pub fn apply_exit(&mut self, exit_index: usize) -> bool {
        let Some(exit) = self
            .levels
            .get(&self.depth)
            .and_then(|l| l.passage.as_ref())
            .and_then(|p| p.exits.get(exit_index))
            .cloned()
        else {
            return false;
        };
        self.chronicle.set_exit(self.depth, exit.clone());
        self.exit_taken = Some(exit);
        self.phase = Phase::Descending { exit: exit_index };
        true
    }

    /// The in-world clock at the current depth. `level_elapsed_secs` is real time spent in the
    /// level; it is capped at one second short of the next level's base time.
    pub fn clock(&self, pack: &StoryPack, level_elapsed_secs: f32) -> ClockReading {
        let per_level = pack.meta.minutes_per_level.max(1) * 60;
        let elapsed = (level_elapsed_secs.max(0.0) as u32).min(per_level - 1);
        let total = parse_clock(&pack.meta.start_clock)
            .saturating_add(per_level.saturating_mul(self.depth))
            .saturating_add(elapsed);
        let days = total / 86_400;
        let endless = pack.act_for_depth(self.depth).is_some_and(|a| a.endless);
        let date = if endless {
            "01/01/19100"
        } else if days == 0 {
            "12/31/99"
        } else {
            "01/01/00"
        };
        ClockReading {
            seconds: total % 86_400,
            date: date.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> RunState {
        RunState::new("Tester", "Test Person", "a dream", 1, "rollover", None)
    }

    #[test]
    fn ego_clamps_to_zero_and_max() {
        let mut s = Stats::default();
        s.add_ego(50);
        assert_eq!(s.ego, EGO_MAX);
        s.add_ego(-500);
        assert_eq!(s.ego, 0);
        assert!(s.is_dead());
    }

    #[test]
    fn integration_clamps_both_ways() {
        let mut s = Stats::default();
        s.add_integration(99);
        assert_eq!(s.integration, INTEGRATION_MAX);
        s.add_integration(-99);
        assert_eq!(s.integration, INTEGRATION_MIN);
    }

    #[test]
    fn arrival_heals_ten_and_prunes_old_levels() {
        let mut r = run();
        r.stats.ego = 50;
        for d in 0..4 {
            let seed = LevelSeed {
                depth: d,
                act_id: "nigredo".into(),
                anchor: None,
                base_danger: 1,
                omen: 1,
                motifs: vec![],
                room: String::new(),
                focus: None,
                objective_kind: ObjectiveKind::Retrieve,
                target_daemon: None,
                wordlock_len: 5,
            };
            r.levels.insert(d, Level::new(seed, vec![]));
        }
        r.arrive(2);
        assert_eq!(r.stats.ego, 60);
        assert_eq!(r.levels.keys().copied().collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(r.phase, Phase::Arrival);
    }

    #[test]
    fn buy_spends_ratio_and_refuses_when_poor_or_owned() {
        let mut r = run();
        assert!(r.buy(Program::IcePick));
        assert_eq!(r.stats.ratio_kb, RATIO_START_KB - 80);
        assert!(!r.buy(Program::IcePick));
        assert!(!r.buy(Program::Ghost));
        assert_eq!(r.stats.ratio_kb, 16);
    }

    #[test]
    fn irc_progress_round_trips_and_old_saves_load_without_it() {
        let mut r = run();
        r.irc.cracked.insert("3:warezbot".into());
        r.irc.files.push(IrcFile {
            depth: 3,
            bot: "warezbot".into(),
            file: FileDef {
                name: "PHRK_48.TXT".into(),
                description: "zine".into(),
                body: "text".into(),
            },
        });
        assert!(r.irc.has_file(3, "warezbot", "PHRK_48.TXT"));
        assert!(!r.irc.has_file(4, "warezbot", "PHRK_48.TXT"));
        let mut value = serde_json::to_value(&r).unwrap();
        let back: RunState = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back.irc, r.irc);
        // A save written before IRC existed has no `irc` key.
        value.as_object_mut().unwrap().remove("irc");
        let old: RunState = serde_json::from_value(value).unwrap();
        assert_eq!(old.irc, IrcProgress::default());
    }

    #[test]
    fn backdoors_start_open_and_old_saves_load_without_them() {
        let mut r = run();
        assert_eq!(Backdoor::default(), Backdoor::Open { misses: 0 });
        r.backdoors.insert(2, Backdoor::Planted);
        let mut value = serde_json::to_value(&r).unwrap();
        let back: RunState = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back.backdoors.get(&2), Some(&Backdoor::Planted));
        // A save written before the hidden line existed has no `backdoors` key.
        value.as_object_mut().unwrap().remove("backdoors");
        let old: RunState = serde_json::from_value(value).unwrap();
        assert!(old.backdoors.is_empty());
    }

    #[test]
    fn arriving_drops_backdoors_of_shallower_boards() {
        let mut r = run();
        r.backdoors.insert(1, Backdoor::Planted);
        r.backdoors.insert(2, Backdoor::Locked);
        r.arrive(2);
        assert_eq!(r.backdoors.keys().copied().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn clock_parses_and_formats() {
        assert_eq!(parse_clock("23:00:00"), 82_800);
        assert_eq!(parse_clock("garbage"), 82_800);
        let c = ClockReading {
            seconds: 3_661,
            date: String::new(),
        };
        assert_eq!(c.hms(), "01:01:01");
    }

    #[test]
    fn gen_source_tags() {
        assert_eq!(GenSource::Authored.tag(), "authored");
        assert_eq!(
            GenSource::Ai {
                model: "m".into(),
                millis: 41_200
            }
            .tag(),
            "ai m 41.2s"
        );
        assert_eq!(
            GenSource::Offline {
                reason: "timeout".into()
            }
            .tag(),
            "offline: timeout"
        );
    }
}
