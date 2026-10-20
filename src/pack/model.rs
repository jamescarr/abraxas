//! Story-pack data model: everything an author can write in a pack, plus the loaded `StoryPack`.
//!
//! These types are shared by the whole engine. They double as the shapes the AI director fills
//! in and the shapes stored in saves, so they derive serde (and JSON schema where the AI sees
//! them).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::game::programs::Program;

// ---------------------------------------------------------------------------------------------
// Enums shared by packs, the director, and the engine
// ---------------------------------------------------------------------------------------------

/// Act palette selector (colours live in `crate::palette`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteId {
    Nigredo,
    Albedo,
    Citrinitas,
    Rubedo,
    Prism,
}

/// A procedural vision. `Noise` is valid in packs (an ending can use it) but the AI is told
/// to pick one of the other seven.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VisionKind {
    Tunnel,
    Plasma,
    Mandala,
    Fire,
    Ouroboros,
    Starfield,
    Mirror,
    Noise,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Unix,
    Vax,
    Mainframe,
    Dos,
}

impl HostOs {
    /// The lowercase name shown in the connect banner and on the hidden line.
    pub fn label(self) -> &'static str {
        match self {
            HostOs::Unix => "unix",
            HostOs::Vax => "vax",
            HostOs::Mainframe => "mainframe",
            HostOs::Dos => "dos",
        }
    }
}

/// How a choice relates to the Shadow: facing it, denying it, or standing aside.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Integrate,
    Repress,
    Neutral,
}

impl Stance {
    pub const ALL: [Stance; 3] = [Stance::Integrate, Stance::Repress, Stance::Neutral];

    /// Integration delta applied when this stance is chosen at a story choice.
    pub fn integration_delta(self) -> i32 {
        match self {
            Stance::Integrate => 1,
            Stance::Repress => -1,
            Stance::Neutral => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Stance::Integrate => "integrate",
            Stance::Repress => "repress",
            Stance::Neutral => "neutral",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    Calm,
    Deep,
    Mirror,
}

impl ExitKind {
    pub const ALL: [ExitKind; 3] = [ExitKind::Calm, ExitKind::Deep, ExitKind::Mirror];

    pub fn as_str(self) -> &'static str {
        match self {
            ExitKind::Calm => "calm",
            ExitKind::Deep => "deep",
            ExitKind::Mirror => "mirror",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveKind {
    Retrieve,
    Plant,
    Crash,
}

/// What guards the objective node (anchor override).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveIce {
    Breach,
    Black,
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Archetype {
    Persona,
    Shadow,
    Syzygy,
    Anima,
    Animus,
    WiseOldMan,
    GreatMother,
    Trickster,
    #[serde(rename = "self")]
    SelfArchetype,
    Child,
    Hero,
}

impl Archetype {
    pub fn as_str(self) -> &'static str {
        match self {
            Archetype::Persona => "persona",
            Archetype::Shadow => "shadow",
            Archetype::Syzygy => "syzygy",
            Archetype::Anima => "anima",
            Archetype::Animus => "animus",
            Archetype::WiseOldMan => "wise_old_man",
            Archetype::GreatMother => "great_mother",
            Archetype::Trickster => "trickster",
            Archetype::SelfArchetype => "self",
            Archetype::Child => "child",
            Archetype::Hero => "hero",
        }
    }
}

/// Whether the AI may touch an anchor level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiMode {
    #[default]
    Adapt,
    Off,
}

// ---------------------------------------------------------------------------------------------
// pack.toml
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMeta {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub motifs: Vec<String>,
    #[serde(default)]
    pub room_kinds: Vec<String>,
    #[serde(default = "default_start_clock")]
    pub start_clock: String,
    #[serde(default = "default_minutes_per_level")]
    pub minutes_per_level: u32,
    #[serde(default)]
    pub act: Vec<ActDef>,
    #[serde(default)]
    pub endless: Option<EndlessDef>,
}

fn default_start_clock() -> String {
    "23:00:00".to_string()
}

fn default_minutes_per_level() -> u32 {
    4
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActDef {
    pub id: String,
    pub name: String,
    /// Inclusive `[from, to]`.
    pub depths: [u32; 2],
    pub palette: PaletteId,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub direction: String,
    pub default_vision: VisionKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndlessDef {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_endless_palette")]
    pub palette: PaletteId,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default = "default_endless_vision")]
    pub default_vision: VisionKind,
}

fn default_endless_palette() -> PaletteId {
    PaletteId::Prism
}

fn default_endless_vision() -> VisionKind {
    VisionKind::Starfield
}

/// Uniform view over an authored act or the endless tail.
#[derive(Debug, Clone, Copy)]
pub struct ActView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub palette: PaletteId,
    pub goal: &'a str,
    pub direction: &'a str,
    pub default_vision: VisionKind,
    /// Index into `meta.act`, or `meta.act.len()` for the endless tail.
    pub index: usize,
    pub endless: bool,
}

/// Id used for the endless tail in chronicle entries and level seeds.
pub const ENDLESS_ACT_ID: &str = "endless";

// ---------------------------------------------------------------------------------------------
// cast.toml
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastFile {
    #[serde(default)]
    pub character: Vec<Character>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Character {
    pub id: String,
    /// `{shadow}` only for the shadow archetype.
    pub handle: String,
    pub archetype: Archetype,
    #[serde(default)]
    pub from_depth: u32,
    #[serde(default)]
    pub focusable: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub voice: String,
    #[serde(default)]
    pub art: Option<String>,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub eliza: Vec<ElizaGroup>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElizaGroup {
    pub keywords: Vec<String>,
    pub replies: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Shared authored/engine content shapes
// ---------------------------------------------------------------------------------------------

/// A message-board post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostDef {
    pub from: String,
    pub subject: String,
    pub body: String,
}

/// A text file on the BBS or on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
}

/// A zine article on the hidden line; reading it teaches the DUEL attack of `attack`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArticleDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub attack: Program,
    pub body: String,
}

/// One option of a story choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionDef {
    pub text: String,
    pub stance: Stance,
    #[serde(default)]
    pub flags: Vec<String>,
}

/// A story choice (`[choice]` / pool `[[choice]]` with `[[choice.option]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceDef {
    pub prompt: String,
    #[serde(default, rename = "option")]
    pub options: Vec<OptionDef>,
}

/// A way down to the next level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitDef {
    pub kind: ExitKind,
    pub text: String,
}

/// An authored ending (anchor `[[ending]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndingDef {
    pub id: String,
    pub title: String,
    pub when_stance: Stance,
    #[serde(default)]
    pub min_integration: Option<i32>,
    #[serde(default)]
    pub max_integration: Option<i32>,
    /// Defaults to `Mandala` when absent.
    #[serde(default)]
    pub vision: Option<VisionKind>,
    pub text: String,
}

impl EndingDef {
    pub fn vision(&self) -> VisionKind {
        self.vision.unwrap_or(VisionKind::Mandala)
    }

    /// True when the choice stance and the player's integration select this ending.
    pub fn matches(&self, stance: Stance, integration: i32) -> bool {
        self.when_stance == stance
            && self.min_integration.is_none_or(|m| integration >= m)
            && self.max_integration.is_none_or(|m| integration <= m)
    }
}

// ---------------------------------------------------------------------------------------------
// anchors/*.toml
// ---------------------------------------------------------------------------------------------

/// An authored level at a fixed depth. Every key except `depth` is optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub depth: u32,
    #[serde(default)]
    pub ai: AiMode,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub bbs_name: Option<String>,
    #[serde(default)]
    pub tagline: Option<String>,
    /// A cast id, or a literal NPC handle.
    #[serde(default)]
    pub sysop: Option<String>,
    #[serde(default)]
    pub danger: Option<u8>,
    /// A cast id.
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub vision: Option<VisionKind>,
    #[serde(default)]
    pub programs: Vec<Program>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub level_card: Option<String>,
    #[serde(default)]
    pub required: Option<String>,
    /// Lines an adapted beat must contain word for word (case and punctuation aside); each is
    /// also in `beat`, so the authored fallback always has them.
    #[serde(default)]
    pub keep: Vec<String>,
    #[serde(default)]
    pub interlude: Option<String>,
    #[serde(default)]
    pub beat: Option<String>,
    #[serde(default)]
    pub captions: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub host: Option<AnchorHost>,
    #[serde(default)]
    pub post: Vec<PostDef>,
    #[serde(default)]
    pub text_file: Vec<FileDef>,
    #[serde(default)]
    pub host_file: Vec<FileDef>,
    #[serde(default)]
    pub choice: Option<ChoiceDef>,
    #[serde(default)]
    pub exit: Vec<ExitDef>,
    #[serde(default)]
    pub ending: Vec<EndingDef>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorHost {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub os: Option<HostOs>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub objective_kind: Option<ObjectiveKind>,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub target_file: Option<String>,
    /// Crash objectives only; lowercase process name.
    #[serde(default)]
    pub target_daemon: Option<String>,
    #[serde(default)]
    pub objective_ice: Option<ObjectiveIce>,
    /// 3..=8; overrides the danger table's node count.
    #[serde(default)]
    pub nodes: Option<u8>,
    #[serde(default)]
    pub node_names: Vec<String>,
    #[serde(default)]
    pub ice_names: Vec<String>,
}

/// An anchor together with the file it was loaded from (for error messages).
#[derive(Debug, Clone)]
pub struct AnchorEntry {
    /// Path relative to the pack root, e.g. `anchors/03-black-sun.toml`.
    pub file: String,
    pub anchor: Anchor,
}

// ---------------------------------------------------------------------------------------------
// pool.toml
// ---------------------------------------------------------------------------------------------

/// The offline generator's word bank. Missing or empty lists inherit from the embedded pool.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pool {
    pub titles: Vec<String>,
    pub bbs_names: Vec<String>,
    pub taglines: Vec<String>,
    pub level_cards: Vec<String>,
    pub sysops: Vec<String>,
    pub host_names: Vec<String>,
    pub node_names: Vec<String>,
    pub ice_names: Vec<String>,
    pub daemons: Vec<String>,
    pub interludes: Vec<String>,
    pub beats: Vec<String>,
    pub captions: Vec<String>,
    pub post: Vec<PostDef>,
    pub text_file: Vec<FileDef>,
    pub host_file: Vec<FileDef>,
    pub choice: Vec<ChoiceDef>,
    pub exit: Vec<ExitDef>,
    pub words_5: Vec<String>,
    pub words_6: Vec<String>,
    pub words_7: Vec<String>,
    pub words_8: Vec<String>,
    pub eliza: Vec<ElizaGroup>,
    pub eliza_fallback: Vec<String>,
    /// IRC server host names (`irc.hecate.net`).
    pub irc_servers: Vec<String>,
    /// Channel names, `#` plus 2..=20 of `[a-z0-9_-]`.
    pub irc_channels: Vec<String>,
    /// Nicks of the chatterers (handle rule, as `sysops`).
    pub irc_nicks: Vec<String>,
    /// Nicks of fserv bots (handle rule).
    pub irc_bots: Vec<String>,
    /// Chatter lines; may use `{handle}`, `{shadow}`, `{real_name}`.
    pub irc_chatter: Vec<String>,
    /// `.NFO` hint files; the engine appends the front-door clue to the body.
    pub irc_nfo: Vec<FileDef>,
    /// Zines about hacking technique (the game's own commands and ICE).
    pub irc_zines: Vec<FileDef>,
    /// Pipe-coded zine art (at most 78x20) shown on the hidden line when the ANS directory has
    /// no files.
    pub zine_art: Vec<FileDef>,
    /// Zine articles on the hidden line; each teaches one DUEL attack.
    pub zine_article: Vec<ArticleDef>,
}

impl Pool {
    /// Fill every missing or empty list from `base`, list by list.
    pub fn inherit_from(&mut self, base: &Pool) {
        macro_rules! inherit {
            ($($field:ident),* $(,)?) => {
                $(if self.$field.is_empty() { self.$field = base.$field.clone(); })*
            };
        }
        inherit!(
            titles,
            bbs_names,
            taglines,
            level_cards,
            sysops,
            host_names,
            node_names,
            ice_names,
            daemons,
            interludes,
            beats,
            captions,
            post,
            text_file,
            host_file,
            choice,
            exit,
            words_5,
            words_6,
            words_7,
            words_8,
            eliza,
            eliza_fallback,
            irc_servers,
            irc_channels,
            irc_nicks,
            irc_bots,
            irc_chatter,
            irc_nfo,
            irc_zines,
            zine_art,
            zine_article,
        );
    }

    /// Candidate words of exactly `len` letters (5..=8); empty for other lengths.
    pub fn words(&self, len: usize) -> &[String] {
        match len {
            5 => &self.words_5,
            6 => &self.words_6,
            7 => &self.words_7,
            8 => &self.words_8,
            _ => &[],
        }
    }

    /// Exits of one kind.
    pub fn exits_of(&self, kind: ExitKind) -> impl Iterator<Item = &ExitDef> {
        self.exit.iter().filter(move |e| e.kind == kind)
    }
}

// ---------------------------------------------------------------------------------------------
// The loaded pack
// ---------------------------------------------------------------------------------------------

/// A fully loaded story pack: pack.toml, canon, cast, anchors, pool (after inheritance) and art.
#[derive(Debug, Clone)]
pub struct StoryPack {
    pub meta: PackMeta,
    pub canon: String,
    pub cast: Vec<Character>,
    /// Sorted by file name.
    pub anchors: Vec<AnchorEntry>,
    pub pool: Pool,
    /// `art/*.txt`, keyed by file name (raw text, pipe codes intact).
    pub art: BTreeMap<String, String>,
}

impl StoryPack {
    pub fn endless_enabled(&self) -> bool {
        self.meta.endless.as_ref().is_some_and(|e| e.enabled)
    }

    /// Last authored depth (the final act's `to`), or 0 with no acts.
    pub fn last_depth(&self) -> u32 {
        self.meta.act.iter().map(|a| a.depths[1]).max().unwrap_or(0)
    }

    /// The act covering `depth`, the endless tail beyond the last act when enabled, else `None`.
    pub fn act_for_depth(&self, depth: u32) -> Option<ActView<'_>> {
        for (index, a) in self.meta.act.iter().enumerate() {
            if depth >= a.depths[0] && depth <= a.depths[1] {
                return Some(ActView {
                    id: &a.id,
                    name: &a.name,
                    palette: a.palette,
                    goal: &a.goal,
                    direction: &a.direction,
                    default_vision: a.default_vision,
                    index,
                    endless: false,
                });
            }
        }
        match &self.meta.endless {
            Some(e) if e.enabled && depth > self.last_depth() => Some(ActView {
                id: ENDLESS_ACT_ID,
                name: &e.name,
                palette: e.palette,
                goal: &e.goal,
                direction: &e.direction,
                default_vision: e.default_vision,
                index: self.meta.act.len(),
                endless: true,
            }),
            _ => None,
        }
    }

    /// True when finishing the choice at `depth` plays an ending: the anchor there has authored
    /// endings, or it is the last authored depth and the endless tail is disabled (the generic
    /// `END OF LINE` ending).
    pub fn ends_run_at(&self, depth: u32) -> bool {
        self.anchor_at(depth)
            .is_some_and(|a| !a.anchor.ending.is_empty())
            || (!self.endless_enabled() && !self.meta.act.is_empty() && depth == self.last_depth())
    }

    /// The first authored ending at `depth` that matches the chosen stance and the player's
    /// integration (`None` when there are none or none match).
    pub fn ending_for(&self, depth: u32, stance: Stance, integration: i32) -> Option<&EndingDef> {
        self.anchor_at(depth)?
            .anchor
            .ending
            .iter()
            .find(|e| e.matches(stance, integration))
    }

    pub fn anchor_at(&self, depth: u32) -> Option<&AnchorEntry> {
        self.anchors.iter().find(|a| a.anchor.depth == depth)
    }

    pub fn character(&self, id: &str) -> Option<&Character> {
        self.cast.iter().find(|c| c.id.eq_ignore_ascii_case(id))
    }

    /// The character with the shadow archetype, if the pack has one.
    pub fn shadow_character(&self) -> Option<&Character> {
        self.cast.iter().find(|c| c.archetype == Archetype::Shadow)
    }

    /// Cast members that may be in focus at `depth`.
    pub fn focusable_at(&self, depth: u32) -> impl Iterator<Item = &Character> {
        self.cast
            .iter()
            .filter(move |c| c.focusable && c.from_depth <= depth)
    }

    /// Art by file name, e.g. `philemon.txt`.
    pub fn art(&self, name: &str) -> Option<&str> {
        self.art.get(name).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ending(stance: Stance, min: Option<i32>, max: Option<i32>) -> EndingDef {
        EndingDef {
            id: "x".into(),
            title: "X".into(),
            when_stance: stance,
            min_integration: min,
            max_integration: max,
            vision: None,
            text: String::new(),
        }
    }

    #[test]
    fn ending_matches_stance_and_integration_bounds() {
        let e = ending(Stance::Integrate, Some(3), None);
        assert!(e.matches(Stance::Integrate, 3));
        assert!(!e.matches(Stance::Integrate, 2));
        assert!(!e.matches(Stance::Repress, 9));
        let low = ending(Stance::Integrate, None, Some(2));
        assert!(low.matches(Stance::Integrate, -10));
        assert!(!low.matches(Stance::Integrate, 3));
    }

    #[test]
    fn pool_inherits_only_empty_lists() {
        let base = Pool {
            titles: vec!["A".into()],
            captions: vec!["C".into()],
            ..Pool::default()
        };
        let mut custom = Pool {
            titles: vec!["MINE".into()],
            ..Pool::default()
        };
        custom.inherit_from(&base);
        assert_eq!(custom.titles, vec!["MINE".to_string()]);
        assert_eq!(custom.captions, vec!["C".to_string()]);
    }

    #[test]
    fn archetype_serializes_self_keyword() {
        let a: Archetype = serde_json::from_str("\"self\"").unwrap();
        assert_eq!(a, Archetype::SelfArchetype);
        assert_eq!(a.as_str(), "self");
    }
}
