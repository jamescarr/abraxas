//! Prompt text and request parameters for every AI call.
//!
//! The system message is identical for every call in a run (it depends only on the player's
//! names and the pack), so Ollama can reuse its cached prefix. Everything that varies goes in the
//! user message.

use crate::config::AiConfig;
use crate::director::validate::{Filler, GenContext};
use crate::director::{ChatLine, ChatPartner};
use crate::game::state::{Place, RunState};
use crate::llm::{LlmRequest, Message};
use crate::oracle;
use crate::pack::model::{EndingDef, ObjectiveKind, StoryPack};
use crate::rng::substream_u64;

pub const MAX_TOKENS_PLACE: u32 = 2600;
pub const MAX_TOKENS_PASSAGE: u32 = 1000;
pub const MAX_TOKENS_INTERLUDE: u32 = 200;
pub const MAX_TOKENS_EPILOGUE: u32 = 500;
pub const MAX_TOKENS_CHAT: u32 = 180;

/// Ollama (and OpenAI) parse the request seed as a signed 64-bit integer, so only 63 bits of the
/// derived seed are sent.
pub const API_SEED_MASK: u64 = i64::MAX as u64;

/// How many chat turns are sent to the model.
pub const CHAT_TURNS: usize = 12;

/// Text calls run slightly hotter than JSON calls.
const TEXT_TEMPERATURE_BONUS: f32 = 0.1;

const NPC_SYSOP_PERSONA: &str =
    "a weary, helpful, slightly strange 1999 sysop who knows this board's lore";

fn objective_word(kind: ObjectiveKind) -> &'static str {
    match kind {
        ObjectiveKind::Retrieve => "retrieve",
        ObjectiveKind::Plant => "plant",
        ObjectiveKind::Crash => "crash",
    }
}

fn schema_text(schema: &serde_json::Value) -> String {
    serde_json::to_string(schema).unwrap_or_default()
}

/// The system message shared by every call of a run.
pub fn system_prompt(run: &RunState, pack: &StoryPack) -> String {
    let f = Filler::for_run(run, None);
    let mut out = String::new();
    out.push_str(
        "You are the Director of ABRAXAS, a text game set on 12/31/1999 in a fictional world where the net is a shared dream reached by modem. You write one piece of the game at a time.\n",
    );
    out.push_str(
        "Rules: stay inside the fiction; no real brands, real people, or technology after 1999. Narration is second person, present tense, short sentences, concrete sensory detail. Use Jungian symbolism only through images; never name Jung or explain psychology. Plain text only: no markdown, no emoji. Respect every length limit.\n",
    );
    out.push_str(&format!(
        "The player is {} (real name {}). Their Shadow's handle is {}.\n",
        run.handle,
        run.real_name,
        run.shadow()
    ));
    out.push_str(&format!("STORY PACK: {}\n", pack.meta.title));
    out.push_str(f.fill(pack.canon.trim()).as_str());
    out.push('\n');
    out.push_str(&format!("STYLE: {}\n", f.fill(pack.meta.style.trim())));
    out.push_str("CAST:");
    for c in &pack.cast {
        out.push_str(&format!(
            "\n- {}: {}, {}. {} Voice: {}",
            c.id,
            f.fill(&c.handle),
            c.archetype.as_str(),
            f.fill(c.description.trim()),
            f.fill(c.voice.trim())
        ));
    }
    out
}

/// The PLACE user message.
pub fn place_prompt(ctx: &GenContext, digest: &str) -> String {
    let seed = ctx.seed;
    let depth = ctx.depth();
    let f = Filler::new(ctx);
    let mut out = format!("Write the PLACE for LEVEL {depth}.\n");
    if let Some(act) = ctx.pack.act_for_depth(depth) {
        out.push_str(&format!(
            "Act: {}. {} {}\n",
            act.name,
            act.goal.trim(),
            act.direction.trim()
        ));
    }
    let h = oracle::hexagram(seed.omen);
    out.push_str(&format!(
        "Omen: I Ching hexagram {}, \"{}\". Let its image color this place.\n",
        h.number, h.name
    ));
    let focus = seed
        .focus
        .as_deref()
        .and_then(|id| ctx.pack.character(id))
        .map_or_else(
            || "\"none\"".to_string(),
            |c| format!("{} ({})", c.id, c.archetype.as_str()),
        );
    out.push_str(&format!(
        "Motifs: {}. Room kind: {}. Archetype in focus: {focus}.\n",
        seed.motifs.join("; "),
        seed.room
    ));
    out.push_str(&format!(
        "Danger class {} of 5. Objective kind: {}.",
        seed.base_danger,
        objective_word(seed.objective_kind)
    ));
    if seed.objective_kind == ObjectiveKind::Crash
        && let Some(daemon) = &seed.target_daemon
    {
        out.push_str(&format!(
            " The target process is `{daemon}`; name it in the objective."
        ));
    }
    out.push('\n');
    if let Some(a) = ctx.anchor() {
        let mut locked: Vec<String> = Vec::new();
        let mut push = |name: &str, value: Option<&str>| {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                locked.push(format!("{name} = \"{}\"", f.fill(v)));
            }
        };
        push("title", a.title.as_deref());
        push("bbs_name", a.bbs_name.as_deref());
        push("tagline", a.tagline.as_deref());
        push("sysop", a.sysop.as_deref());
        if let Some(host) = &a.host {
            push("host.name", host.name.as_deref());
            push("host.banner", host.banner.as_deref());
            push("host.objective", host.objective.as_deref());
            push("host.target_file", host.target_file.as_deref());
            if let Some(os) = host.os {
                locked.push(format!("host.os = \"{os:?}\"").to_lowercase());
            }
            if !host.node_names.is_empty() {
                locked.push(format!("host.node_names = {:?}", host.node_names));
            }
            if !host.ice_names.is_empty() {
                locked.push(format!("host.ice_names = {:?}", host.ice_names));
            }
        }
        out.push_str("Authored level.");
        if !locked.is_empty() {
            out.push_str(&format!(" Keep exactly: {}.", locked.join("; ")));
        }
        if let Some(req) = a.required.as_deref().filter(|r| !r.trim().is_empty()) {
            out.push_str(&format!(" Required: {}.", f.fill(req.trim())));
        }
        out.push('\n');
        if let Some(card) = a.level_card.as_deref().filter(|c| !c.trim().is_empty()) {
            out.push_str(&format!(
                "Authored level card (keep every fact, adapt the wording): {}\n",
                f.fill(card.trim())
            ));
        }
    }
    out.push_str(&format!("Story so far:\n{digest}\n"));
    out.push_str(&format!(
        "Return JSON: title (≤40 chars, a Backrooms-style level name), bbs_name (≤24), tagline (≤60), level_card (2-3 sentences, an explorer's field note, ≤320), sysop (a CAST id if one runs this board, else a new 1990s handle ≤16), host {{name ≤40, os unix|vax|mainframe|dos, banner ≤160, objective (one sentence ≤100), target_file (DOS 8.3 name), node_names (8 names ≤24), ice_names (3 names ≤20)}}, posts (3 × {{from ≤16, subject ≤40, body ≤400}}; one post must be from {}, who writes like the player but colder and more honest), text_files (2 × {{name 8.3, description ≤40, body ≤400}}), host_files (2 × same), words (14 uppercase English words of exactly {} letters tied to the omen or motifs).\n",
        ctx.run.shadow(),
        seed.wordlock_len
    ));
    out.push_str(&format!(
        "JSON schema: {}",
        schema_text(&crate::director::schema::place_schema())
    ));
    out
}

/// The label for the player's integration of the Shadow.
pub fn integration_label(integration: i32) -> &'static str {
    match integration {
        i32::MIN..=-4 => "torn",
        -3..=-1 => "wary",
        0..=2 => "balanced",
        _ => "whole",
    }
}

/// The PASSAGE user message for `place`.
pub fn passage_prompt(ctx: &GenContext, place: &Place, digest: &str) -> String {
    let depth = ctx.depth();
    let f = Filler::new(ctx);
    let mut out = format!(
        "Write the PASSAGE for LEVEL {depth}: \"{}\" on {} (sysop {}). Level card: {}\n",
        place.title, place.bbs_name, place.sysop.handle, place.level_card
    );
    out.push_str(&format!(
        "Objective: {} The payload file is {}.\n",
        place.host.objective, place.host.target_file
    ));
    let last = depth
        .checked_sub(1)
        .and_then(|d| ctx.run.chronicle.entry(d))
        .and_then(|e| e.choice.as_ref());
    match last {
        Some(c) => out.push_str(&format!(
            "Their last choice: \"{}\" ({}).\n",
            c.text,
            c.stance.as_str()
        )),
        None => out.push_str("Their last choice: none yet.\n"),
    }
    let integration = ctx.run.stats.integration;
    out.push_str(&format!(
        "Integration of the Shadow: {integration} ({}).\n",
        integration_label(integration)
    ));
    if let Some(a) = ctx.anchor() {
        let beat = a.beat.as_deref().filter(|b| !b.trim().is_empty());
        let required = a.required.as_deref().filter(|r| !r.trim().is_empty());
        if beat.is_some() || required.is_some() {
            out.push_str("Authored passage.");
            if let Some(b) = beat {
                out.push_str(&format!(
                    " Authored beat (keep every fact and name, adapt the wording to the story so far): {}",
                    f.fill(b.trim())
                ));
            }
            if let Some(r) = required {
                out.push_str(&format!(" Required: {}", f.fill(r.trim())));
                if r.contains('"') {
                    out.push_str(" The beat must contain every quoted line word for word.");
                }
            }
            out.push('\n');
        }
    }
    out.push_str(&format!("Story so far:\n{digest}\n"));
    out.push_str(
        "Return JSON: beat (what the payload reveals, advances the story, ≤700), choice {prompt ≤160, options: exactly 3 {text ≤70, stance, flags (0-2 snake_case facts to remember)}; one option per stance: integrate = face or accept the dark side, repress = deny, destroy or flee, neutral = observe or bargain}, exits (exactly 3 {kind, text ≤60}: calm = a gentle way down, deep = a dangerous shortcut, mirror = a way toward the Shadow), vision (tunnel|plasma|mandala|fire|ouroboros|starfield|mirror), captions (3 lines ≤48), summary (one past-tense sentence ≤200).\n",
    );
    out.push_str(&format!(
        "JSON schema: {}",
        schema_text(&crate::director::schema::passage_schema())
    ));
    out
}

/// The INTERLUDE user message: the fall from the previous level (through the exit taken) into
/// this one. `place` is this level's place.
pub fn interlude_prompt(ctx: &GenContext, place: &Place) -> String {
    let depth = ctx.depth();
    let run = ctx.run;
    let prev = depth.checked_sub(1).and_then(|d| {
        run.chronicle
            .entry(d)
            .map(|e| e.title.clone())
            .or_else(|| {
                run.levels
                    .get(&d)
                    .and_then(|l| l.place.as_ref())
                    .map(|p| p.title.clone())
            })
            .map(|t| (d, t))
    });
    let exit = run
        .exit_taken
        .as_ref()
        .map(|e| e.text.trim_end_matches('.'));
    let mut out = String::from("Write the INTERLUDE: ");
    match (prev, exit) {
        (Some((d, title)), Some(x)) => out.push_str(&format!(
            "{} falls from LEVEL {d} \"{title}\" through \"{x}\" into LEVEL {depth} \"{}\".",
            run.handle, place.title
        )),
        (Some((d, title)), None) => out.push_str(&format!(
            "{} falls from LEVEL {d} \"{title}\" into LEVEL {depth} \"{}\".",
            run.handle, place.title
        )),
        (None, _) => out.push_str(&format!(
            "{} is pulled down the line into LEVEL {depth} \"{}\".",
            run.handle, place.title
        )),
    }
    out.push_str(&format!(
        " {} Second person, present tense, 40-70 words, plain text, no title.",
        place.level_card
    ));
    if let Some(text) = ctx
        .anchor()
        .and_then(|a| a.interlude.as_deref())
        .filter(|t| !t.trim().is_empty())
    {
        out.push_str(&format!(
            " Authored interlude (keep these facts): {}",
            Filler::new(ctx).fill(text.trim())
        ));
    }
    out
}

/// The EPILOGUE user message.
pub fn epilogue_prompt(run: &RunState, ending: &EndingDef, digest: &str) -> String {
    let f = Filler::for_run(run, None);
    format!(
        "Write the ending \"{}\". Keep every fact: {} Story so far: {digest} Address {} directly, 120-200 words, plain text.",
        ending.title,
        f.fill(ending.text.trim()),
        run.handle
    )
}

/// The chat messages: the shared system message plus the partner's persona, then the last
/// `CHAT_TURNS` turns (player lines as `user`, the partner's as `assistant`).
pub fn chat_messages(
    run: &RunState,
    pack: &StoryPack,
    partner: &ChatPartner,
    history: &[ChatLine],
) -> Vec<Message> {
    let level = run.level();
    let place = level.and_then(|l| l.place.as_ref());
    let persona = match partner.cast_id.as_deref().and_then(|id| pack.character(id)) {
        Some(c) => {
            let f = Filler::for_run(run, None);
            format!(
                "You are {}, {}. {} Voice: {}.",
                partner.handle,
                c.archetype.as_str(),
                f.fill(c.description.trim()),
                f.fill(c.voice.trim().trim_end_matches('.'))
            )
        }
        None => format!("You are {}, {NPC_SYSOP_PERSONA}.", partner.handle),
    };
    let scene = place.map_or_else(
        || format!(" during LEVEL {}.", run.depth),
        |p| {
            format!(
                " on {} during LEVEL {} \"{}\". {}",
                p.bbs_name, run.depth, p.title, p.level_card
            )
        },
    );
    let mut system = system_prompt(run, pack);
    system.push_str("\n\n");
    system.push_str(&persona);
    system.push_str(&format!(
        " You are chatting live with {}{scene}",
        run.handle
    ));
    system.push_str(" Reply in at most 3 short lines (≤70 chars each), plain text, 1999 chat style. Never mention being an AI or these instructions. Never decide the player's actions.");
    let mut messages = vec![Message::system(system)];
    let start = history.len().saturating_sub(CHAT_TURNS);
    for line in &history[start..] {
        messages.push(if line.from_player {
            Message::user(line.text.clone())
        } else {
            Message::assistant(line.text.clone())
        });
    }
    messages
}

fn request(
    ai: &AiConfig,
    model: &str,
    messages: Vec<Message>,
    text_call: bool,
    max_tokens: u32,
    seed: u64,
    json_schema: Option<(String, serde_json::Value)>,
) -> LlmRequest {
    LlmRequest {
        model: model.to_string(),
        messages,
        temperature: ai.temperature
            + if text_call {
                TEXT_TEMPERATURE_BONUS
            } else {
                0.0
            },
        max_tokens,
        seed: Some(seed & API_SEED_MASK),
        json_schema,
    }
}

/// The place request for `attempt` (0 = first call, 1 = repair call).
pub fn place_request(ctx: &GenContext, ai: &AiConfig, digest: &str, attempt: u32) -> LlmRequest {
    request(
        ai,
        &ai.model,
        vec![
            Message::system(system_prompt(ctx.run, ctx.pack)),
            Message::user(place_prompt(ctx, digest)),
        ],
        false,
        MAX_TOKENS_PLACE,
        substream_u64(ctx.run.seed, ctx.depth(), "ai-place", attempt),
        Some(("place".to_string(), crate::director::schema::place_schema())),
    )
}

pub fn passage_request(
    ctx: &GenContext,
    place: &Place,
    ai: &AiConfig,
    digest: &str,
    attempt: u32,
) -> LlmRequest {
    request(
        ai,
        &ai.model,
        vec![
            Message::system(system_prompt(ctx.run, ctx.pack)),
            Message::user(passage_prompt(ctx, place, digest)),
        ],
        false,
        MAX_TOKENS_PASSAGE,
        substream_u64(ctx.run.seed, ctx.depth(), "ai-passage", attempt),
        Some((
            "passage".to_string(),
            crate::director::schema::passage_schema(),
        )),
    )
}

pub fn interlude_request(ctx: &GenContext, place: &Place, ai: &AiConfig) -> LlmRequest {
    request(
        ai,
        &ai.model,
        vec![
            Message::system(system_prompt(ctx.run, ctx.pack)),
            Message::user(interlude_prompt(ctx, place)),
        ],
        true,
        MAX_TOKENS_INTERLUDE,
        substream_u64(ctx.run.seed, ctx.depth(), "ai-interlude", 0),
        None,
    )
}

pub fn epilogue_request(
    run: &RunState,
    pack: &StoryPack,
    ending: &EndingDef,
    ai: &AiConfig,
    digest: &str,
) -> LlmRequest {
    request(
        ai,
        &ai.model,
        vec![
            Message::system(system_prompt(run, pack)),
            Message::user(epilogue_prompt(run, ending, digest)),
        ],
        true,
        MAX_TOKENS_EPILOGUE,
        substream_u64(run.seed, run.depth, "ai-epilogue", 0),
        None,
    )
}

pub fn chat_request(
    run: &RunState,
    pack: &StoryPack,
    partner: &ChatPartner,
    history: &[ChatLine],
    ai: &AiConfig,
    session: u64,
) -> LlmRequest {
    request(
        ai,
        ai.chat_model(),
        chat_messages(run, pack, partner, history),
        true,
        MAX_TOKENS_CHAT,
        substream_u64(run.seed, run.depth, "ai-chat", session as u32),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::chronicle::{ChosenOption, ChronicleEntry};
    use crate::game::seed::ensure_level;
    use crate::pack;
    use crate::pack::model::Stance;

    fn setup(depth: u32) -> (RunState, StoryPack) {
        let pack = pack::embedded().unwrap();
        let mut run = RunState::new("Tester", "Jo Doe", "a dream", 9, "rollover", None);
        for d in 0..=depth {
            ensure_level(&mut run, &pack, d);
        }
        run.depth = depth;
        (run, pack)
    }

    fn ctx<'a>(run: &'a RunState, pack: &'a StoryPack, depth: u32) -> GenContext<'a> {
        GenContext {
            run,
            pack,
            seed: &run.levels[&depth].seed,
        }
    }

    fn with_choice(run: &mut RunState, depth: u32, text: &str, stance: Stance) {
        run.chronicle.upsert(ChronicleEntry {
            depth,
            act_id: "nigredo".into(),
            act_name: "NIGREDO".into(),
            title: format!("Room {depth}"),
            class: 1,
            omen: 1,
            level_card: String::new(),
            summary: format!("Summary {depth}."),
            choice: Some(ChosenOption {
                text: text.into(),
                stance,
            }),
            exit: None,
        });
    }

    #[test]
    fn anchor_passage_prompt_carries_required_text_and_last_choice() {
        let (mut run, pack) = setup(3);
        with_choice(&mut run, 2, "Read it aloud, every word.", Stance::Integrate);
        let digest = run.chronicle.digest(&run, &pack);
        let ctx = ctx(&run, &pack, 3);
        let place = crate::director::offline::place(&ctx);
        let prompt = passage_prompt(&ctx, &place, &digest);
        assert!(prompt.contains("Abraxas is not a place, it is what holds both"));
        assert!(prompt.contains("Their last choice: \"Read it aloud, every word.\" (integrate)."));
        assert!(prompt.contains("Authored beat (keep every fact and name, adapt the wording"));
        // The digest's own last choice line is in the story so far.
        assert!(prompt.contains("Chose: \"Read it aloud, every word.\" (integrate)."));
        assert!(prompt.contains("JSON schema: {"));
    }

    #[test]
    fn passage_prompt_without_anchor_has_no_authored_section() {
        let (run, pack) = setup(5);
        let digest = run.chronicle.digest(&run, &pack);
        let ctx = ctx(&run, &pack, 5);
        let place = crate::director::offline::place(&ctx);
        let prompt = passage_prompt(&ctx, &place, &digest);
        assert!(!prompt.contains("Authored"));
        assert!(prompt.contains("Their last choice: none yet."));
    }

    #[test]
    fn integration_labels_follow_the_plan_boundaries() {
        let labels: Vec<_> = [-10, -4, -3, -1, 0, 2, 3, 10]
            .into_iter()
            .map(integration_label)
            .collect();
        assert_eq!(
            labels,
            [
                "torn", "torn", "wary", "wary", "balanced", "balanced", "whole", "whole"
            ]
        );
    }

    #[test]
    fn system_message_is_identical_for_every_kind_and_depth() {
        let (run, pack) = setup(5);
        let ai = AiConfig::default();
        let ending = EndingDef {
            id: "x".into(),
            title: "X".into(),
            when_stance: Stance::Neutral,
            min_integration: None,
            max_integration: None,
            vision: None,
            text: "text".into(),
        };
        let partner = ChatPartner {
            handle: "PHILEMON".into(),
            cast_id: Some("philemon".into()),
        };
        let mut systems = Vec::new();
        for depth in [0, 3, 5] {
            let ctx = ctx(&run, &pack, depth);
            let place = crate::director::offline::place(&ctx);
            systems.push(place_request(&ctx, &ai, "d", 0).messages[0].content.clone());
            systems.push(
                passage_request(&ctx, &place, &ai, "d", 1).messages[0]
                    .content
                    .clone(),
            );
            systems.push(
                interlude_request(&ctx, &place, &ai).messages[0]
                    .content
                    .clone(),
            );
        }
        systems.push(
            epilogue_request(&run, &pack, &ending, &ai, "d").messages[0]
                .content
                .clone(),
        );
        let chat = chat_request(&run, &pack, &partner, &[], &ai, 1);
        assert!(chat.messages[0].content.starts_with(&systems[0]));
        assert!(systems.iter().all(|s| *s == systems[0]));
        assert!(systems[0].contains("The player is Tester (real name Jo Doe)."));
        assert!(systems[0].contains(&run.shadow()));
        assert!(systems[0].contains("STORY PACK: ABRAXAS: The Rollover"));
        assert!(systems[0].contains("- philemon: PHILEMON, wise_old_man."));
        assert!(systems[0].contains(&format!("- shadow: {}, shadow.", run.shadow())));
    }

    #[test]
    fn place_prompt_names_the_daemon_only_for_crash_objectives() {
        let (mut run, pack) = setup(5);
        let level = run.levels.get_mut(&5).unwrap();
        level.seed.objective_kind = ObjectiveKind::Crash;
        level.seed.target_daemon = Some("echod".into());
        let crash = place_prompt(&ctx(&run, &pack, 5), "digest");
        assert!(crash.contains(
            "Objective kind: crash. The target process is `echod`; name it in the objective."
        ));
        let level = run.levels.get_mut(&5).unwrap();
        level.seed.objective_kind = ObjectiveKind::Retrieve;
        let retrieve = place_prompt(&ctx(&run, &pack, 5), "digest");
        assert!(retrieve.contains("Objective kind: retrieve."));
        assert!(!retrieve.contains("target process"));
        assert!(!retrieve.contains("echod"));
    }

    #[test]
    fn place_prompt_has_the_omen_hexagram_name_and_limits() {
        let (run, pack) = setup(5);
        let c = ctx(&run, &pack, 5);
        let h = oracle::hexagram(c.seed.omen);
        let prompt = place_prompt(&c, "digest text");
        assert!(prompt.starts_with("Write the PLACE for LEVEL 5."));
        assert!(prompt.contains(&format!("I Ching hexagram {}, \"{}\"", h.number, h.name)));
        assert!(prompt.contains(&c.seed.motifs.join("; ")));
        assert!(prompt.contains("Story so far:\ndigest text"));
        assert!(prompt.contains(&format!("exactly {} letters", c.seed.wordlock_len)));
        assert!(prompt.contains(&format!("one post must be from {}", run.shadow())));
        assert!(!prompt.contains("Authored level"));
        assert!(prompt.contains("posts (3 × {from ≤16, subject ≤40, body ≤400}"));
        assert!(prompt.contains("text_files (2 × {name 8.3, description ≤40, body ≤400})"));
        assert!(prompt.contains("host_files (2 × same)"));
        for old in ["body ≤600", "body ≤900", "host_files (3", "posts (4"] {
            assert!(!prompt.contains(old), "{old}");
        }
    }

    #[test]
    fn anchor_place_prompt_lists_locked_fields_required_and_card() {
        let (run, pack) = setup(3);
        let prompt = place_prompt(&ctx(&run, &pack, 3), "d");
        assert!(prompt.contains("Authored level. Keep exactly: title = \"THE BLACK SUN\""));
        assert!(prompt.contains("bbs_name = \"SOL NIGER\""));
        assert!(prompt.contains("Required: The Shadow runs this board"));
        assert!(prompt.contains("Authored level card (keep every fact, adapt the wording):"));
    }

    #[test]
    fn interlude_prompt_names_both_levels_and_the_exit() {
        let (mut run, pack) = setup(5);
        with_choice(&mut run, 4, "x", Stance::Neutral);
        run.exit_taken = Some(crate::pack::model::ExitDef {
            kind: crate::pack::model::ExitKind::Deep,
            text: "A crack in the screen.".into(),
        });
        let c = ctx(&run, &pack, 5);
        let place = crate::director::offline::place(&c);
        let prompt = interlude_prompt(&c, &place);
        assert!(prompt.contains(
            "Tester falls from LEVEL 4 \"Room 4\" through \"A crack in the screen\" into LEVEL 5"
        ));
        assert!(prompt.contains("40-70 words"));
    }

    #[test]
    fn chat_keeps_only_the_last_twelve_turns() {
        let (run, pack) = setup(5);
        let partner = ChatPartner {
            handle: "NullByte".into(),
            cast_id: None,
        };
        let history: Vec<ChatLine> = (0..20)
            .map(|i| ChatLine {
                from_player: i % 2 == 0,
                text: format!("line {i}"),
            })
            .collect();
        let messages = chat_messages(&run, &pack, &partner, &history);
        assert_eq!(messages.len(), 13);
        assert_eq!(messages[1].content, "line 8");
        assert_eq!(messages[12].content, "line 19");
        assert!(messages[0].content.contains(
            "You are NullByte, a weary, helpful, slightly strange 1999 sysop who knows this board's lore."
        ));
        assert!(messages[0].content.contains("at most 3 short lines"));
    }

    #[test]
    fn request_parameters_follow_the_plan_table() {
        let (run, pack) = setup(5);
        let ai = AiConfig {
            chat_model: "chatty".into(),
            temperature: 0.8,
            ..AiConfig::default()
        };
        let c = ctx(&run, &pack, 5);
        let place_gen = crate::director::offline::place(&c);
        let ending = EndingDef {
            id: "x".into(),
            title: "X".into(),
            when_stance: Stance::Neutral,
            min_integration: None,
            max_integration: None,
            vision: None,
            text: "t".into(),
        };
        let partner = ChatPartner {
            handle: "NullByte".into(),
            cast_id: None,
        };
        let place = place_request(&c, &ai, "d", 0);
        let passage = passage_request(&c, &place_gen, &ai, "d", 0);
        let interlude = interlude_request(&c, &place_gen, &ai);
        let epilogue = epilogue_request(&run, &pack, &ending, &ai, "d");
        let chat = chat_request(&run, &pack, &partner, &[], &ai, 4);

        let table = [
            (&place, 2600, 0.8, ai.model.as_str(), "ai-place"),
            (&passage, 1000, 0.8, ai.model.as_str(), "ai-passage"),
            (&interlude, 200, 0.9, ai.model.as_str(), "ai-interlude"),
            (&epilogue, 500, 0.9, ai.model.as_str(), "ai-epilogue"),
            (&chat, 180, 0.9, "chatty", "ai-chat"),
        ];
        for (req, tokens, temperature, model, purpose) in table {
            assert_eq!(req.max_tokens, tokens, "{purpose}");
            assert!((req.temperature - temperature).abs() < 1e-6, "{purpose}");
            assert_eq!(req.model, model, "{purpose}");
            assert!(req.seed.is_some_and(|s| s <= i64::MAX as u64), "{purpose}");
        }
        assert_eq!(
            place.seed,
            Some(substream_u64(9, 5, "ai-place", 0) & API_SEED_MASK)
        );
        assert_eq!(
            place_request(&c, &ai, "d", 1).seed,
            Some(substream_u64(9, 5, "ai-place", 1) & API_SEED_MASK)
        );
        assert_eq!(
            chat.seed,
            Some(substream_u64(9, 5, "ai-chat", 4) & API_SEED_MASK)
        );
        assert_eq!(
            place.json_schema.as_ref().map(|s| s.0.as_str()),
            Some("place")
        );
        assert_eq!(
            passage.json_schema.as_ref().map(|s| s.0.as_str()),
            Some("passage")
        );
        assert!(interlude.json_schema.is_none() && epilogue.json_schema.is_none());
        assert!(chat.json_schema.is_none());
    }
}
