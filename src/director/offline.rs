//! The offline generator: builds levels, interludes, epilogues and ELIZA replies from the
//! authored anchors and the pack's pool, with no AI. Every pick is deterministic from
//! `substream(run.seed, depth, "offline-<part>", 0)`, and the structural guarantees are the same
//! ones AI output is repaired into (it shares the builders in `director::validate`).

use rand::seq::IndexedRandom;

use crate::director::validate::{
    Filler, GenContext, PassageCand, PlaceCand, build_passage, build_place,
};
use crate::game::state::{Passage, Place, RunState};
use crate::pack::model::{Character, EndingDef, ExitDef, StoryPack};
use crate::rng::substream;

/// A level without the AI: anchor content first (locked fields, authored posts and files), the
/// pool for everything else. Titles, BBS names and posts already used in the run are avoided;
/// when the title list is exhausted, titles are reused with ` II` appended.
pub fn place(ctx: &GenContext) -> Place {
    build_place(ctx, PlaceCand::default(), "offline-place")
}

/// The passage for `place`: the anchor's authored beat, choice, exits, captions, summary and
/// vision, else the pool. The default summary is `{handle} passed through {title}.`
pub fn passage(ctx: &GenContext, place: &Place) -> Passage {
    build_passage(ctx, place, PassageCand::default(), "offline-passage")
}

/// The descent interlude into this level: the anchor's authored interlude, else a pool
/// interlude with `{exit}` replaced by the text of the exit taken.
pub fn interlude(ctx: &GenContext, _place: &Place, exit: Option<&ExitDef>) -> String {
    let f = Filler::new(ctx);
    if let Some(authored) = ctx.anchor().and_then(|a| a.interlude.as_deref()) {
        return f.fill(authored);
    }
    let exit_text = exit.map_or("the way down", |e| e.text.trim_end_matches('.'));
    let mut rng = substream(ctx.run.seed, ctx.depth(), "offline-interlude", 0);
    match ctx.pack.pool.interludes.choose(&mut rng) {
        Some(text) => f.fill_with(text, None, Some(exit_text)),
        None => format!("You fall through {exit_text}. The line hums. The carpet is damp."),
    }
}

/// The authored ending text with placeholders filled.
pub fn epilogue(ending: &EndingDef, run: &RunState) -> String {
    Filler::for_run(run, None).fill(&ending.text)
}

/// A chat reply without the AI. The character's ELIZA groups are tried in order and the first
/// one with a keyword that appears (case-insensitively) in `text` wins; its replies cycle with
/// `turn`. Otherwise the fallback lines cycle with `turn`. `None` is the generic NPC sysop and
/// uses the pool's groups and fallback; a character without groups or fallback lines of its own
/// borrows the pool's.
pub fn eliza_reply(
    pack: &StoryPack,
    character: Option<&Character>,
    text: &str,
    turn: usize,
) -> String {
    let groups = match character {
        Some(c) if !c.eliza.is_empty() => &c.eliza,
        _ => &pack.pool.eliza,
    };
    let fallback = match character {
        Some(c) if !c.fallback.is_empty() => &c.fallback,
        _ => &pack.pool.eliza_fallback,
    };
    // A trailing space lets keywords like "hi " match a final word.
    let haystack = format!("{} ", text.to_lowercase());
    let hit = groups.iter().find(|g| {
        !g.replies.is_empty()
            && g.keywords
                .iter()
                .any(|k| !k.is_empty() && haystack.contains(&k.to_lowercase()))
    });
    if let Some(g) = hit {
        return g.replies[turn % g.replies.len()].clone();
    }
    if fallback.is_empty() {
        return "...".to_string();
    }
    fallback[turn % fallback.len()].clone()
}

pub use crate::director::validate::sysop_for;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::validate::tests::{run, seed_at};
    use crate::game::danger;
    use crate::game::state::LevelSeed;
    use crate::pack::{
        self,
        model::{ExitKind, ObjectiveKind, Stance, VisionKind},
    };
    use crate::text::{is_83_name, is_cp437};

    fn level(run: &RunState, pack: &StoryPack, depth: u32) -> (LevelSeed, Place, Passage) {
        let seed = seed_at(run, pack, depth);
        let ctx = GenContext {
            run,
            pack,
            seed: &seed,
        };
        let place = place(&ctx);
        let passage = passage(&ctx, &place);
        (seed, place, passage)
    }

    fn all_text(place: &Place, passage: &Passage) -> Vec<String> {
        let mut t = vec![
            place.title.clone(),
            place.bbs_name.clone(),
            place.tagline.clone(),
            place.level_card.clone(),
            place.sysop.handle.clone(),
            place.host.name.clone(),
            place.host.banner.clone(),
            place.host.objective.clone(),
            passage.beat.clone(),
            passage.choice.prompt.clone(),
            passage.summary.clone(),
        ];
        t.extend(
            place
                .posts
                .iter()
                .flat_map(|p| [p.from.clone(), p.subject.clone(), p.body.clone()]),
        );
        t.extend(
            place
                .text_files
                .iter()
                .chain(&place.host_files)
                .flat_map(|f| [f.name.clone(), f.description.clone(), f.body.clone()]),
        );
        t.extend(
            place
                .host
                .node_names
                .iter()
                .chain(&place.host.ice_names)
                .cloned(),
        );
        t.extend(passage.choice.options.iter().map(|o| o.text.clone()));
        t.extend(passage.exits.iter().map(|e| e.text.clone()));
        t.extend(passage.captions.iter().cloned());
        t
    }

    #[test]
    fn offline_levels_satisfy_every_structural_invariant() {
        let pack = pack::embedded().unwrap();
        for seed in 0..100u64 {
            let r = run(seed);
            for depth in 0..=20u32 {
                let (s, place, passage) = level(&r, &pack, depth);
                let ctx = format!("seed {seed} depth {depth}");
                assert_eq!(place.posts.len(), 5, "{ctx}");
                assert_eq!(place.posts[4].subject, "re: front door", "{ctx}");
                assert_eq!(place.text_files.len(), 2, "{ctx}");
                assert_eq!(place.host_files.len(), 3, "{ctx}");
                for list in [&place.text_files, &place.host_files] {
                    let mut names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
                    assert!(names.iter().all(|n| is_83_name(n)), "{ctx}: {names:?}");
                    names.sort();
                    names.dedup();
                    assert_eq!(names.len(), list.len(), "{ctx}: duplicate file names");
                }
                let n = danger::params(s.base_danger).wordlock_words;
                assert!(place.words.len() >= 2 * n, "{ctx}");
                assert!(
                    place
                        .words
                        .iter()
                        .all(|w| w.len() == s.wordlock_len
                            && w.bytes().all(|b| b.is_ascii_uppercase())),
                    "{ctx}"
                );
                assert!(place.words.contains(&place.password), "{ctx}");
                assert_eq!(place.host.node_names.len(), 8, "{ctx}");
                assert_eq!(place.host.ice_names.len(), 3, "{ctx}");
                assert!((2..=16).contains(&place.sysop.handle.len()), "{ctx}");
                let shadow = r.shadow();
                let shadow_posts = place.posts[..4].iter().filter(|p| p.from == shadow).count();
                assert_eq!(shadow_posts >= 1, depth >= 1, "{ctx}");
                match s.objective_kind {
                    ObjectiveKind::Retrieve => {
                        assert!(is_83_name(&place.host.target_file), "{ctx}");
                        assert!(
                            place
                                .host_files
                                .iter()
                                .any(|f| f.name == place.host.target_file),
                            "{ctx}"
                        );
                    }
                    ObjectiveKind::Plant => {
                        assert_eq!(place.host.target_file, "PAYLOAD.PIF", "{ctx}")
                    }
                    ObjectiveKind::Crash => {
                        assert!(place.host.target_file.is_empty(), "{ctx}");
                        let d = s.target_daemon.as_deref().unwrap();
                        assert!(place.host.objective.to_lowercase().contains(d), "{ctx}");
                    }
                }
                let mut stances: Vec<Stance> =
                    passage.choice.options.iter().map(|o| o.stance).collect();
                stances.sort();
                assert_eq!(
                    stances,
                    [Stance::Integrate, Stance::Repress, Stance::Neutral],
                    "{ctx}"
                );
                let kinds: Vec<ExitKind> = passage.exits.iter().map(|e| e.kind).collect();
                assert_eq!(kinds, ExitKind::ALL, "{ctx}");
                assert_eq!(passage.captions.len(), 3, "{ctx}");
                assert_ne!(passage.vision, VisionKind::Noise, "{ctx}");
                assert!(
                    !passage.summary.is_empty() && !passage.beat.is_empty(),
                    "{ctx}"
                );
                for t in all_text(&place, &passage) {
                    assert!(t.chars().all(is_cp437), "{ctx}: non-CP437 in {t:?}");
                    assert!(
                        !t.contains('{') && !t.contains('}'),
                        "{ctx}: placeholder in {t:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn offline_generation_is_deterministic_and_seed_sensitive() {
        let pack = pack::embedded().unwrap();
        for depth in [0, 3, 5, 12, 18] {
            let a = level(&run(42), &pack, depth);
            let b = level(&run(42), &pack, depth);
            assert_eq!((&a.1, &a.2), (&b.1, &b.2));
        }
        assert_ne!(level(&run(1), &pack, 5).1, level(&run(2), &pack, 5).1);
    }

    #[test]
    fn anchor_zero_is_exactly_the_authored_level() {
        let pack = pack::embedded().unwrap();
        let r = run(42);
        let (_, place, passage) = level(&r, &pack, 0);
        let a = &pack.anchor_at(0).unwrap().anchor;
        assert_eq!(place.title, "THE LOBBY");
        assert_eq!(place.bbs_name, "ABRAXAS");
        assert_eq!(place.sysop.cast_id.as_deref(), Some("philemon"));
        assert_eq!(place.password, "OWLET");
        assert_eq!(&place.words[..a.words.len()], a.words.as_slice());
        for (got, want) in place.posts[..4].iter().zip(&a.post) {
            assert_eq!(got.subject, want.subject);
            assert_eq!(got.from, want.from);
        }
        assert_eq!(place.posts[4].subject, "re: front door");
        assert_eq!(
            place
                .text_files
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["HOWTO.TXT", "ABRAXAS.NFO"]
        );
        assert_eq!(
            place
                .host_files
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["SHIFTLOG.TXT", "CALLERS.LST", "WREN-00.LOG"]
        );
        assert_eq!(place.host.target_file, "WREN-00.LOG");
        assert_eq!(place.host.nodes, Some(3));
        assert_eq!(place.host.node_names[0], "FRONT DESK");
        assert!(
            passage
                .beat
                .starts_with("WREN-00.LOG. 12/10/99, 02:14. Jo Doe, I dialed")
        );
        assert_eq!(
            passage.captions,
            [
                "THE LINE IS NOT DEAD",
                "IT IS LISTENING",
                "DOWN IS THE ONLY WAY OUT"
            ]
        );
        assert_eq!(
            passage.summary,
            "Tester dialed Wren's board, crossed onto ABRAXAS, and read her first log, which said not to hang up."
        );
        assert_eq!(passage.choice.options.len(), 3);
        assert_eq!(
            passage.choice.options[2].text,
            "Ask Philemon who he really is."
        );
        assert_eq!(passage.vision, VisionKind::Fire);
    }

    #[test]
    fn anchor_three_keeps_its_locked_fields_offline() {
        let pack = pack::embedded().unwrap();
        let r = run(7);
        let (_, place, passage) = level(&r, &pack, 3);
        assert_eq!(place.title, "THE BLACK SUN");
        assert_eq!(place.bbs_name, "SOL NIGER");
        assert_eq!(place.sysop.cast_id.as_deref(), Some("shadow"));
        assert_eq!(place.sysop.handle, r.shadow());
        assert!(place.host_files.iter().any(|f| f.name == "WREN-01.LOG"));
        assert!(passage.beat.contains("Abraxas is not a place"));
        assert!(place.posts[0].from == r.shadow());
    }

    #[test]
    fn used_titles_bbs_names_and_posts_are_avoided() {
        let pack = pack::embedded().unwrap();
        let mut r = run(5);
        let (_, first, _) = level(&r, &pack, 5);
        r.used_pool.extend(first.pool_keys());
        let (_, second, _) = level(&r, &pack, 5);
        assert_ne!(first.title, second.title);
        assert_ne!(first.bbs_name, second.bbs_name);
        for p in &second.posts[..4] {
            assert!(
                !first.posts[..4]
                    .iter()
                    .any(|q| q.from == p.from && q.subject == p.subject),
                "reused post {p:?}"
            );
        }
    }

    #[test]
    fn titles_in_the_chronicle_are_avoided_and_exhaustion_appends_ii() {
        let pack = pack::embedded().unwrap();
        let mut r = run(5);
        // All but one title used: that one must be picked.
        let keep = pack.pool.titles[7].clone();
        for t in pack.pool.titles.iter().filter(|t| **t != keep) {
            r.used_pool.insert(format!("title:{t}"));
        }
        assert_eq!(level(&r, &pack, 5).1.title, keep);
        // Every title used: a title appears again with " II".
        r.used_pool.insert(format!("title:{keep}"));
        let title = level(&r, &pack, 5).1.title;
        let base = title.strip_suffix(" II").expect("II suffix");
        assert!(pack.pool.titles.iter().any(|t| t == base));
        // Chronicle titles count as used too.
        let mut r2 = run(5);
        let first = level(&r2, &pack, 5).1.title;
        r2.chronicle.upsert(crate::game::chronicle::ChronicleEntry {
            depth: 4,
            act_id: "albedo".into(),
            act_name: "ALBEDO".into(),
            title: first.clone(),
            class: 1,
            omen: 1,
            level_card: String::new(),
            summary: String::new(),
            choice: None,
            exit: None,
        });
        assert_ne!(level(&r2, &pack, 5).1.title, first);
    }

    #[test]
    fn interludes_use_the_authored_text_or_the_exit() {
        let pack = pack::embedded().unwrap();
        let r = run(9);
        let exit = ExitDef {
            kind: ExitKind::Deep,
            text: "The carrier tone, followed down like a rope.".into(),
        };
        let seed0 = seed_at(&r, &pack, 0);
        let ctx0 = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed0,
        };
        let place0 = place(&ctx0);
        assert!(interlude(&ctx0, &place0, Some(&exit)).starts_with("The line crosses."));
        let seed5 = seed_at(&r, &pack, 5);
        let ctx5 = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed5,
        };
        let place5 = place(&ctx5);
        let text = interlude(&ctx5, &place5, Some(&exit));
        assert!(
            text.contains("The carrier tone, followed down like a rope"),
            "{text}"
        );
        assert!(!text.contains("{exit}"));
        assert!(!text.contains("rope.."));
        assert_eq!(text, interlude(&ctx5, &place5, Some(&exit)));
        assert!(!interlude(&ctx5, &place5, None).contains('{'));
    }

    #[test]
    fn epilogue_fills_placeholders() {
        let pack = pack::embedded().unwrap();
        let ending = pack
            .anchor_at(15)
            .unwrap()
            .anchor
            .ending
            .iter()
            .find(|e| e.id == "individuation")
            .unwrap();
        let r = run(1);
        let text = epilogue(ending, &r);
        assert!(!text.contains('{'));
        assert!(text.chars().all(is_cp437));
    }

    #[test]
    fn eliza_matches_first_group_cycles_and_falls_back() {
        let pack = pack::embedded().unwrap();
        let philemon = pack.character("philemon");
        let g = &philemon.unwrap().eliza[0];
        let kw = g.keywords[0].clone();
        let a = eliza_reply(
            &pack,
            philemon,
            &format!("tell me about {}", kw.to_uppercase()),
            0,
        );
        let b = eliza_reply(&pack, philemon, &kw, 1);
        assert_eq!(a, g.replies[0]);
        assert_eq!(b, g.replies[1 % g.replies.len()]);
        assert_eq!(
            eliza_reply(&pack, philemon, &kw, g.replies.len()),
            g.replies[0]
        );

        let fb = &philemon.unwrap().fallback;
        assert_eq!(eliza_reply(&pack, philemon, "zzzz qqqq", 0), fb[0]);
        assert_eq!(
            eliza_reply(&pack, philemon, "zzzz qqqq", fb.len() + 1),
            fb[1]
        );

        // Generic NPC sysop: pool groups; "hi " keyword matches a trailing "hi".
        let hello = &pack.pool.eliza[0];
        assert_eq!(eliza_reply(&pack, None, "hi", 0), hello.replies[0]);
        assert_eq!(
            eliza_reply(&pack, None, "zzzz", 1),
            pack.pool.eliza_fallback[1]
        );
    }

    #[test]
    fn eliza_first_matching_group_wins() {
        let mut pack = pack::embedded().unwrap();
        pack.pool.eliza = vec![
            crate::pack::model::ElizaGroup {
                keywords: vec!["red".into()],
                replies: vec!["R1".into(), "R2".into()],
            },
            crate::pack::model::ElizaGroup {
                keywords: vec!["blue".into(), "red".into()],
                replies: vec!["B".into()],
            },
        ];
        assert_eq!(eliza_reply(&pack, None, "red and blue", 3), "R2");
        assert_eq!(eliza_reply(&pack, None, "only blue", 0), "B");
    }
}
