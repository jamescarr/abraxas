//! The chronicle: one entry per completed level, and the digest the AI director reads.

use serde::{Deserialize, Serialize};

use crate::pack::model::{ExitDef, Stance};

/// The option the player picked at a level's story choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChosenOption {
    pub text: String,
    pub stance: Stance,
}

/// What happened at one level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChronicleEntry {
    pub depth: u32,
    pub act_id: String,
    pub act_name: String,
    pub title: String,
    /// Base danger class 1..=5.
    pub class: u8,
    /// I Ching hexagram 1..=64.
    pub omen: u8,
    pub level_card: String,
    pub summary: String,
    pub choice: Option<ChosenOption>,
    /// Added when the exit is picked.
    pub exit: Option<ExitDef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chronicle {
    /// Ordered by depth.
    pub entries: Vec<ChronicleEntry>,
}

impl Chronicle {
    pub fn entry(&self, depth: u32) -> Option<&ChronicleEntry> {
        self.entries.iter().find(|e| e.depth == depth)
    }

    /// Insert an entry, replacing any entry at the same depth, keeping depth order. An existing
    /// exit is kept when the replacement has none.
    pub fn upsert(&mut self, mut entry: ChronicleEntry) {
        if let Some(pos) = self.entries.iter().position(|e| e.depth == entry.depth) {
            if entry.exit.is_none() {
                entry.exit = self.entries[pos].exit.take();
            }
            self.entries[pos] = entry;
        } else {
            let at = self.entries.partition_point(|e| e.depth < entry.depth);
            self.entries.insert(at, entry);
        }
    }

    /// Record the exit taken from `depth`. No-op when there is no entry yet.
    pub fn set_exit(&mut self, depth: u32, exit: ExitDef) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.depth == depth) {
            e.exit = Some(exit);
        }
    }
}
