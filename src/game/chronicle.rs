//! The chronicle: one entry per completed level, and the digest the AI director reads.

use serde::{Deserialize, Serialize};

use crate::game::state::RunState;
use crate::pack::model::{ExitDef, Stance, StoryPack};

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

    /// The "story so far" text the AI director reads: the last 10 entries in full, older
    /// entries grouped by act, the flags, the player's last 3 board posts and the registration.
    /// Empty sections are omitted except the registration line.
    pub fn digest(&self, run: &RunState, pack: &StoryPack) -> String {
        const RECENT: usize = 10;
        let split = self.entries.len().saturating_sub(RECENT);
        let (older, recent) = self.entries.split_at(split);
        let mut out = String::new();

        if !recent.is_empty() {
            out.push_str("Recent:\n");
            for e in recent {
                out.push_str(&format!("L{:02} {}:", e.depth, e.title.to_uppercase()));
                if !e.summary.is_empty() {
                    out.push(' ');
                    out.push_str(&e.summary);
                }
                if let Some(c) = &e.choice {
                    out.push_str(&format!(" Chose: \"{}\" ({}).", c.text, c.stance.as_str()));
                }
                if let Some(x) = &e.exit {
                    out.push_str(&format!(" Exit: \"{}\" ({}).", x.text, x.kind.as_str()));
                }
                out.push('\n');
            }
        }

        if !older.is_empty() {
            out.push_str("Earlier:\n");
            let mut groups: Vec<(&str, Vec<&ChronicleEntry>)> = Vec::new();
            for e in older {
                match groups.last_mut() {
                    Some((id, list)) if *id == e.act_id => list.push(e),
                    _ => groups.push((e.act_id.as_str(), vec![e])),
                }
            }
            for (_, list) in groups {
                let name = match list[0].act_name.as_str() {
                    "" => pack.act_for_depth(list[0].depth).map_or("", |a| a.name),
                    n => n,
                };
                let titles: Vec<&str> = list.iter().map(|e| e.title.as_str()).collect();
                out.push_str(&format!("{name}: {}.", titles.join("; ")));
                let choices: Vec<&str> = list
                    .iter()
                    .filter_map(|e| e.choice.as_ref().map(|c| c.text.as_str()))
                    .collect();
                if !choices.is_empty() {
                    out.push_str(&format!(" Choices: {}.", choices.join("; ")));
                }
                out.push('\n');
            }
        }

        if !run.flags.is_empty() {
            let flags: Vec<&str> = run.flags.iter().map(String::as_str).collect();
            out.push_str(&format!("Flags: {}\n", flags.join(", ")));
        }

        if !run.player_posts.is_empty() {
            let start = run.player_posts.len().saturating_sub(3);
            let posts: Vec<String> = run.player_posts[start..]
                .iter()
                .map(|p| format!("\"{}\" (L{:02})", p.body, p.depth))
                .collect();
            out.push_str(&format!("Your recent board posts: {}\n", posts.join(" ")));
        }

        out.push_str(&format!(
            "Registration: real name \"{}\"; heard about ABRAXAS from \"{}\".",
            run.real_name, run.heard_from
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::PlayerPost;
    use crate::pack::model::ExitKind;

    fn entry(depth: u32, act: &str, with_choice: bool) -> ChronicleEntry {
        ChronicleEntry {
            depth,
            act_id: act.to_lowercase(),
            act_name: act.to_string(),
            title: format!("Room {depth}"),
            class: 1,
            omen: 1,
            level_card: String::new(),
            summary: format!("Summary {depth}."),
            choice: with_choice.then(|| ChosenOption {
                text: format!("option {depth}"),
                stance: Stance::Repress,
            }),
            exit: None,
        }
    }

    fn run() -> RunState {
        RunState::new("Tester", "Jo Doe", "a dream", 1, "rollover", None)
    }

    #[test]
    fn digest_with_nothing_has_only_registration() {
        let pack = crate::pack::embedded().unwrap();
        let d = Chronicle::default().digest(&run(), &pack);
        assert_eq!(
            d,
            "Registration: real name \"Jo Doe\"; heard about ABRAXAS from \"a dream\"."
        );
    }

    #[test]
    fn digest_keeps_ten_recent_and_groups_older_by_act() {
        let pack = crate::pack::embedded().unwrap();
        let mut c = Chronicle::default();
        for d in 0..13 {
            let act = if d < 2 { "NIGREDO" } else { "ALBEDO" };
            c.upsert(entry(d, act, d != 1));
        }
        c.set_exit(
            12,
            ExitDef {
                kind: ExitKind::Deep,
                text: "A rope.".into(),
            },
        );
        let mut r = run();
        r.flags.insert("zeta".into());
        r.flags.insert("alpha".into());
        for (i, body) in ["one", "two", "three", "four"].iter().enumerate() {
            r.player_posts.push(PlayerPost {
                depth: i as u32,
                subject: String::new(),
                body: (*body).into(),
            });
        }
        let d = c.digest(&r, &pack);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines[0], "Recent:");
        assert_eq!(
            lines[1],
            "L03 ROOM 3: Summary 3. Chose: \"option 3\" (repress)."
        );
        assert_eq!(
            lines[10],
            "L12 ROOM 12: Summary 12. Chose: \"option 12\" (repress). Exit: \"A rope.\" (deep)."
        );
        assert_eq!(lines[11], "Earlier:");
        assert_eq!(lines[12], "NIGREDO: Room 0; Room 1. Choices: option 0.");
        assert_eq!(lines[13], "ALBEDO: Room 2. Choices: option 2.");
        assert_eq!(lines[14], "Flags: alpha, zeta");
        assert_eq!(
            lines[15],
            "Your recent board posts: \"two\" (L01) \"three\" (L02) \"four\" (L03)"
        );
        assert!(lines[16].starts_with("Registration: real name \"Jo Doe\""));
        assert_eq!(lines.len(), 17);
    }
}
