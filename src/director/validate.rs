//! Turns raw AI replies into engine content. The AI can never break the engine: everything it
//! returns is parsed, sanitised, repaired and padded from the anchor and the pack's pool, so a
//! `Place` or `Passage` always satisfies the structural invariants the rest of the game relies on.
//!
//! The same builders also run with an empty candidate to produce the offline levels
//! (`director::offline`), so AI and offline content share one set of invariants.

use std::fmt;

use rand::RngExt;
use rand::seq::{IndexedRandom, SliceRandom};

use crate::director::schema::{PassageGen, PlaceGen};
use crate::game::danger;
use crate::game::state::{HostInfo, LevelSeed, Passage, Place, RunState, Sysop};
use crate::pack::model::{
    AiMode, Anchor, ChoiceDef, ExitDef, ExitKind, FileDef, HostOs, ObjectiveKind, OptionDef,
    PostDef, Stance, StoryPack, VisionKind,
};
use crate::rng::{Rng, substream};
use crate::text::{
    Subst, clean_ai_text, contains_words, fill_placeholders, is_83_name, quoted_spans,
    strip_pipe_codes, to_cp437_safe, truncate,
};

/// Everything validation needs to know about the level being generated.
#[derive(Clone, Copy)]
pub struct GenContext<'a> {
    pub run: &'a RunState,
    pub pack: &'a StoryPack,
    /// The level's seed; the depth is `seed.depth`.
    pub seed: &'a LevelSeed,
}

impl<'a> GenContext<'a> {
    pub fn depth(&self) -> u32 {
        self.seed.depth
    }

    pub(crate) fn anchor(&self) -> Option<&'a Anchor> {
        self.pack.anchor_at(self.seed.depth).map(|e| &e.anchor)
    }
}

/// Why a raw reply was rejected. Its `Display` text is sent back to the model in the repair
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// The reply contains no `{ ... }` span.
    NoJson,
    /// The JSON span did not parse into the expected shape.
    Json(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::NoJson => f.write_str("no JSON object found in the reply"),
            ValidationError::Json(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ValidationError {}

// ---------------------------------------------------------------------------------------------
// Step 1: raw content -> JSON text
// ---------------------------------------------------------------------------------------------

/// Strips `<think>...</think>` blocks (an unterminated one swallows the rest) and Markdown code
/// fences, then returns the text from the first `{` to the last `}`.
pub fn extract_json(raw: &str) -> Result<String, ValidationError> {
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("<think>") {
        text.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    text.push_str(rest);
    // Fence lines (``` or ```json) are removed; a fence inside a JSON string is not a line start.
    let unfenced: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    match (unfenced.find('{'), unfenced.rfind('}')) {
        (Some(a), Some(b)) if a < b => Ok(unfenced[a..=b].to_string()),
        _ => Err(ValidationError::NoJson),
    }
}

/// Extract, parse and fix a place reply. A parse failure is an `Err` (the director retries with
/// a repair message); anything else is repaired.
pub fn parse_place(raw: &str, ctx: &GenContext) -> Result<Place, ValidationError> {
    let json = extract_json(raw)?;
    let generated: PlaceGen =
        serde_json::from_str(&json).map_err(|e| ValidationError::Json(e.to_string()))?;
    Ok(fix_place(generated, ctx))
}

/// Extract, parse and fix a passage reply.
pub fn parse_passage(
    raw: &str,
    ctx: &GenContext,
    place: &Place,
) -> Result<Passage, ValidationError> {
    let json = extract_json(raw)?;
    let generated: PassageGen =
        serde_json::from_str(&json).map_err(|e| ValidationError::Json(e.to_string()))?;
    Ok(fix_passage(generated, ctx, place))
}

// ---------------------------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------------------------

/// Fills placeholders and sanitises text for the run.
pub(crate) struct Filler {
    handle: String,
    shadow: String,
    real_name: String,
    motif: Option<String>,
}

impl Filler {
    pub(crate) fn for_run(run: &RunState, motif: Option<&str>) -> Self {
        Self {
            handle: run.handle.clone(),
            shadow: run.shadow(),
            real_name: run.real_name.clone(),
            motif: motif.map(str::to_string),
        }
    }

    pub(crate) fn new(ctx: &GenContext) -> Self {
        Self::for_run(ctx.run, ctx.seed.motifs.first().map(String::as_str))
    }

    /// Placeholders filled, CP437-safe, pipe codes stripped. Whitespace and length are left
    /// alone (authored text may contain art).
    pub(crate) fn fill_with(&self, text: &str, motif: Option<&str>, exit: Option<&str>) -> String {
        let subst = Subst {
            handle: &self.handle,
            shadow: &self.shadow,
            real_name: &self.real_name,
            motif: motif.or(self.motif.as_deref()),
            exit,
        };
        strip_pipe_codes(&to_cp437_safe(&fill_placeholders(text, &subst)))
    }

    pub(crate) fn fill(&self, text: &str) -> String {
        self.fill_with(text, None, None)
    }

    /// AI text: placeholders filled, then cleaned and truncated to `max` characters.
    fn ai(&self, text: &str, max: usize, keep_newlines: bool) -> String {
        clean_ai_text(&self.fill(text), max, keep_newlines)
    }

    fn ai_opt(&self, text: &str, max: usize, keep_newlines: bool) -> Option<String> {
        Some(self.ai(text, max, keep_newlines)).filter(|s| !s.is_empty())
    }
}

fn valid_flag(f: &str) -> bool {
    let mut chars = f.chars();
    f.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn clean_flags<'a>(flags: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in flags {
        let f = f.trim();
        if valid_flag(f) && !out.iter().any(|o| o == f) {
            out.push(f.to_string());
            if out.len() == 2 {
                break;
            }
        }
    }
    out
}

fn sanitize_handle(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(16)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Sysop and clue post
// ---------------------------------------------------------------------------------------------

/// Resolves a requested sysop: a cast id (or handle) whose `from_depth` has been reached becomes
/// that character; a sanitised 2-16 character NPC handle is used otherwise; failing both, a pool
/// sysop. A cast member who has not appeared yet is never used.
pub fn sysop_for(ctx: &GenContext, requested: &str) -> Sysop {
    let depth = ctx.depth();
    let req = requested.trim();
    let shadow = ctx.run.shadow();
    let resolve = |handle: &str| -> String {
        if handle == "{shadow}" {
            shadow.clone()
        } else {
            sanitize_handle(handle)
        }
    };
    if !req.is_empty() {
        let by_shadow = req.eq_ignore_ascii_case("{shadow}") || req.eq_ignore_ascii_case(&shadow);
        let found = ctx.pack.cast.iter().find(|c| {
            c.id.eq_ignore_ascii_case(req)
                || (c.handle != "{shadow}" && c.handle.eq_ignore_ascii_case(req))
                || (by_shadow && c.archetype == crate::pack::model::Archetype::Shadow)
        });
        match found {
            Some(c) if c.from_depth <= depth => {
                return Sysop {
                    handle: resolve(&c.handle),
                    cast_id: Some(c.id.clone()),
                };
            }
            Some(_) => {}
            None => {
                let npc = sanitize_handle(req);
                if npc.len() >= 2 {
                    return Sysop {
                        handle: npc,
                        cast_id: None,
                    };
                }
            }
        }
    }
    let mut rng = substream(ctx.run.seed, depth, "sysop", 0);
    let handle = ctx
        .pack
        .pool
        .sysops
        .choose(&mut rng)
        .map(|s| sanitize_handle(s))
        .filter(|s| s.len() >= 2)
        .unwrap_or_else(|| "NightOwl".to_string());
    Sysop {
        handle,
        cast_id: None,
    }
}

/// The engine's password-clue post: from `anonymous`, `re: front door`, with the password masked
/// to 2 revealed letters (3 for words of 7 or more letters); every other letter is `_`.
pub fn clue_post(place_host_name: &str, password: &str, rng: &mut Rng) -> PostDef {
    let letters: Vec<char> = password.chars().collect();
    let reveal = if letters.len() <= 6 { 2 } else { 3 }.min(letters.len());
    let mut positions: Vec<usize> = (0..letters.len()).collect();
    positions.shuffle(rng);
    let shown = &positions[..reveal];
    let mask: String = letters
        .iter()
        .enumerate()
        .map(|(i, c)| if shown.contains(&i) { *c } else { '_' })
        .collect();
    PostDef {
        from: "anonymous".to_string(),
        subject: "re: front door".to_string(),
        body: format!(
            "whoever runs {place_host_name} never changed the front door. last time i looked it was {mask}."
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Candidates: sanitised AI output (or nothing, for the offline generator)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct PlaceCand {
    title: Option<String>,
    bbs_name: Option<String>,
    tagline: Option<String>,
    level_card: Option<String>,
    sysop: Option<String>,
    host_name: Option<String>,
    os: Option<HostOs>,
    banner: Option<String>,
    objective: Option<String>,
    target_file: Option<String>,
    node_names: Vec<String>,
    ice_names: Vec<String>,
    posts: Vec<PostDef>,
    text_files: Vec<FileDef>,
    host_files: Vec<FileDef>,
    words: Vec<String>,
}

fn cand_files(f: &Filler, files: &[crate::director::schema::FileGen], max: usize) -> Vec<FileDef> {
    files
        .iter()
        .take(max)
        .filter_map(|g| {
            let body = f.ai(&g.body, AI_BODY_MAX, true);
            if body.is_empty() {
                return None;
            }
            Some(FileDef {
                name: f.ai(&g.name, 12, false).to_ascii_uppercase(),
                description: f.ai(&g.description, 40, false),
                body,
            })
        })
        .collect()
}

fn cand_names(f: &Filler, names: &[String], max_chars: usize) -> Vec<String> {
    names
        .iter()
        .filter_map(|n| f.ai_opt(n, max_chars, false))
        .collect()
}

fn place_cand(generated: &PlaceGen, f: &Filler) -> PlaceCand {
    let shadow = f.shadow.clone();
    let posts = generated
        .posts
        .iter()
        .take(6)
        .filter_map(|p| {
            let body = f.ai_opt(&p.body, AI_BODY_MAX, true)?;
            let from_full = f.fill(&p.from);
            let from = if from_full.trim().eq_ignore_ascii_case(&shadow) {
                shadow.clone()
            } else {
                f.ai_opt(&p.from, 16, false)
                    .unwrap_or_else(|| "anonymous".to_string())
            };
            Some(PostDef {
                from,
                subject: f
                    .ai_opt(&p.subject, 40, false)
                    .unwrap_or_else(|| "(no subject)".to_string()),
                body,
            })
        })
        .collect();
    PlaceCand {
        title: f.ai_opt(&generated.title, 40, false),
        bbs_name: f.ai_opt(&generated.bbs_name, 24, false),
        tagline: f.ai_opt(&generated.tagline, 60, false),
        level_card: f.ai_opt(&generated.level_card, 320, false),
        sysop: Some(generated.sysop.clone()).filter(|s| !s.trim().is_empty()),
        host_name: f.ai_opt(&generated.host.name, 40, false),
        os: Some(generated.host.os),
        banner: f.ai_opt(&generated.host.banner, 160, false),
        objective: f.ai_opt(&generated.host.objective, 100, false),
        target_file: Some(generated.host.target_file.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty()),
        node_names: cand_names(f, &generated.host.node_names, 24),
        ice_names: cand_names(f, &generated.host.ice_names, 20),
        posts,
        text_files: cand_files(f, &generated.text_files, 4),
        host_files: cand_files(f, &generated.host_files, 4),
        words: generated
            .words
            .iter()
            .map(|w| w.trim().to_ascii_uppercase())
            .collect(),
    }
}

/// Validation steps 2-10 for a place. Never fails: anything unusable is replaced from the anchor
/// or the pack's pool.
pub fn fix_place(generated: PlaceGen, ctx: &GenContext) -> Place {
    let f = Filler::new(ctx);
    let cand = place_cand(&generated, &f);
    build_place(ctx, cand, "fill")
}

// ---------------------------------------------------------------------------------------------
// Place builder
// ---------------------------------------------------------------------------------------------

struct PlaceBuilder<'a, 'c> {
    ctx: &'c GenContext<'a>,
    f: Filler,
    rng: Rng,
}

/// Board posts filled from AI or pool text; the engine's clue post is appended after them.
const POST_COUNT: usize = 3;
const TEXT_FILE_COUNT: usize = 2;
const HOST_FILE_COUNT: usize = 2;
/// AI-written post and file bodies are cut to this many characters (authored and pool text is not).
const AI_BODY_MAX: usize = 400;
const NODE_NAME_COUNT: usize = 8;
const ICE_NAME_COUNT: usize = 3;
const AI_WORD_CAP: usize = 14;

/// Builds a complete `Place`: authored anchor content wins, then the candidate (AI) content, then
/// the pool. `purpose` names the rng stream used for pool picks.
pub(crate) fn build_place(ctx: &GenContext, cand: PlaceCand, purpose: &str) -> Place {
    let anchor = ctx.anchor();
    let cand = if anchor.is_some_and(|a| a.ai == AiMode::Off) {
        PlaceCand::default()
    } else {
        cand
    };
    let mut b = PlaceBuilder {
        ctx,
        f: Filler::new(ctx),
        rng: substream(ctx.run.seed, ctx.depth(), purpose, 0),
    };
    b.place(anchor, cand)
}

impl PlaceBuilder<'_, '_> {
    fn place(&mut self, anchor: Option<&Anchor>, cand: PlaceCand) -> Place {
        let ctx = self.ctx;
        let seed = ctx.seed;
        let ahost = anchor.and_then(|a| a.host.as_ref());
        let pool = &ctx.pack.pool;

        let title = match anchor.and_then(|a| a.title.as_deref()) {
            Some(t) => truncate(&self.f.fill(t), 40),
            None => cand.title.clone().unwrap_or_else(|| self.pool_title()),
        };
        let bbs_name = match anchor.and_then(|a| a.bbs_name.as_deref()) {
            Some(t) => truncate(&self.f.fill(t), 24),
            None => cand.bbs_name.clone().unwrap_or_else(|| self.pool_bbs()),
        };
        let tagline = match anchor.and_then(|a| a.tagline.as_deref()) {
            Some(t) => truncate(&self.f.fill(t), 60),
            None => match cand.tagline.clone() {
                Some(t) => t,
                None => self.pick_filled(&pool.taglines, "Nobody dials us. Everybody arrives."),
            },
        };
        let level_card = match (
            cand.level_card.clone(),
            anchor.and_then(|a| a.level_card.as_deref()),
        ) {
            (Some(ai), _) => ai,
            (None, Some(authored)) => self.f.fill(authored),
            (None, None) => {
                let motif = seed.motifs.choose(&mut self.rng).cloned();
                match pool.level_cards.choose(&mut self.rng) {
                    Some(c) => self.f.fill_with(c, motif.as_deref(), None),
                    None => "Field note: a corridor, a hum, a board that was always here.".into(),
                }
            }
        };

        let sysop = match anchor.and_then(|a| a.sysop.as_deref()) {
            Some(s) => sysop_for(ctx, s),
            None => sysop_for(ctx, cand.sysop.as_deref().unwrap_or("")),
        };

        // Host.
        let host_name = match ahost.and_then(|h| h.name.as_deref()) {
            Some(n) => truncate(&self.f.fill(n), 40),
            None => match cand.host_name.clone() {
                Some(n) => n,
                None => self.pick_filled(&pool.host_names, "ODS UNKNOWN EXCHANGE"),
            },
        };
        let os = ahost.and_then(|h| h.os).or(cand.os).unwrap_or_else(|| {
            [HostOs::Unix, HostOs::Vax, HostOs::Mainframe, HostOs::Dos][self.rng.random_range(0..4)]
        });
        let banner = match ahost.and_then(|h| h.banner.as_deref()) {
            Some(b) => truncate(&self.f.fill(b), 160),
            None => cand.banner.clone().unwrap_or_else(|| {
                format!("{host_name} - authorized use only. All sessions are remembered.")
            }),
        };

        let kind = seed.objective_kind;
        let target_file = match kind {
            ObjectiveKind::Plant => "PAYLOAD.PIF".to_string(),
            ObjectiveKind::Crash => String::new(),
            ObjectiveKind::Retrieve => ahost
                .and_then(|h| h.target_file.as_deref())
                .map(|t| t.trim().to_ascii_uppercase())
                .filter(|t| is_83_name(t))
                .or_else(|| cand.target_file.clone().filter(|t| is_83_name(t)))
                .unwrap_or_else(|| format!("DREAM{:02}.LOG", ctx.depth() % 100)),
        };
        let mut objective = match ahost.and_then(|h| h.objective.as_deref()) {
            Some(o) => self.f.fill(o),
            None => cand.objective.clone().unwrap_or_else(|| match kind {
                ObjectiveKind::Retrieve => format!("Retrieve {target_file} from the host."),
                ObjectiveKind::Plant => "Plant PAYLOAD.PIF in the host core.".to_string(),
                ObjectiveKind::Crash => format!(
                    "Kill the {} process at the host core.",
                    seed.target_daemon.as_deref().unwrap_or("daemon")
                ),
            }),
        };
        if kind == ObjectiveKind::Crash
            && let Some(d) = &seed.target_daemon
            && !objective.to_lowercase().contains(&d.to_lowercase())
        {
            objective.push_str(&format!(" (process {d})"));
        }

        let node_names = self.names(
            ahost.map_or(&[][..], |h| &h.node_names),
            &cand.node_names,
            &pool.node_names,
            NODE_NAME_COUNT,
            "NODE",
        );
        let ice_names = self.names(
            ahost.map_or(&[][..], |h| &h.ice_names),
            &cand.ice_names,
            &pool.ice_names,
            ICE_NAME_COUNT,
            "ICE",
        );

        // Words and password.
        let (words, password) = self.words(anchor, &cand.words);

        // Files.
        let text_files = self.files(
            anchor.map_or(&[][..], |a| &a.text_file),
            &cand.text_files,
            &pool.text_file,
            TEXT_FILE_COUNT,
            None,
        );
        let host_files = self.files(
            anchor.map_or(&[][..], |a| &a.host_file),
            &cand.host_files,
            &pool.host_file,
            HOST_FILE_COUNT,
            (kind == ObjectiveKind::Retrieve).then_some(target_file.as_str()),
        );

        // Posts, then the clue.
        let mut posts = self.posts(anchor.map_or(&[][..], |a| &a.post), cand.posts);
        let mut clue_rng = substream(ctx.run.seed, ctx.depth(), "clue", 0);
        posts.push(clue_post(&host_name, &password, &mut clue_rng));

        Place {
            title,
            bbs_name,
            tagline,
            level_card,
            sysop,
            host: HostInfo {
                name: host_name,
                os,
                banner,
                objective,
                target_file,
                objective_ice: ahost.and_then(|h| h.objective_ice),
                nodes: ahost.and_then(|h| h.nodes),
                node_names,
                ice_names,
            },
            posts,
            text_files,
            host_files,
            words,
            password,
        }
    }

    fn title_used(&self, t: &str) -> bool {
        self.ctx.run.used_pool.contains(&format!("title:{t}"))
            || self.ctx.run.chronicle.entries.iter().any(|e| e.title == t)
    }

    /// A pool title not used in this run; when every title is used, the first unused
    /// ` II`/` III`/... variant.
    fn pool_title(&mut self) -> String {
        let titles: Vec<String> = self
            .ctx
            .pack
            .pool
            .titles
            .iter()
            .map(|t| self.f.fill(t))
            .collect();
        for suffix in ["", " II", " III", " IV", " V", " VI", " VII", " VIII"] {
            let free: Vec<String> = titles
                .iter()
                .map(|t| truncate(&format!("{t}{suffix}"), 40))
                .filter(|t| !self.title_used(t))
                .collect();
            if let Some(t) = free.choose(&mut self.rng) {
                return t.clone();
            }
        }
        match titles.choose(&mut self.rng) {
            Some(t) => truncate(&format!("{t} II"), 40),
            None => "THE LONG HALL".to_string(),
        }
    }

    fn pool_bbs(&mut self) -> String {
        let names: Vec<String> = self
            .ctx
            .pack
            .pool
            .bbs_names
            .iter()
            .map(|t| truncate(&self.f.fill(t), 24))
            .collect();
        let free: Vec<&String> = names
            .iter()
            .filter(|n| !self.ctx.run.used_pool.contains(&format!("bbs:{n}")))
            .collect();
        free.choose(&mut self.rng)
            .map(|n| (*n).clone())
            .or_else(|| names.choose(&mut self.rng).cloned())
            .unwrap_or_else(|| "ABRAXAS".to_string())
    }

    fn pick_filled(&mut self, list: &[String], fallback: &str) -> String {
        match list.choose(&mut self.rng) {
            Some(s) => self.f.fill(s),
            None => fallback.to_string(),
        }
    }

    /// Authored names first, then candidate names, then shuffled pool names; unique ignoring
    /// case, padded with numbered names if the pool is too small.
    fn names(
        &mut self,
        authored: &[String],
        cand: &[String],
        pool: &[String],
        count: usize,
        stem: &str,
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(count);
        let add = |out: &mut Vec<String>, n: String| {
            if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
                out.push(n);
            }
        };
        for a in authored {
            add(&mut out, self.f.fill(a));
        }
        for c in cand {
            add(&mut out, c.clone());
        }
        let mut shuffled: Vec<&String> = pool.iter().collect();
        shuffled.shuffle(&mut self.rng);
        for p in shuffled {
            if out.len() >= count {
                break;
            }
            add(&mut out, self.f.fill(p));
        }
        let mut n = 1;
        while out.len() < count {
            add(&mut out, format!("{stem} {n}"));
            n += 1;
        }
        out.truncate(count);
        out
    }

    fn words(&mut self, anchor: Option<&Anchor>, cand: &[String]) -> (Vec<String>, String) {
        let seed = self.ctx.seed;
        let len = seed.wordlock_len;
        let target = 2 * danger::params(seed.base_danger).wordlock_words;
        let valid = |w: &str| w.len() == len && w.bytes().all(|b| b.is_ascii_uppercase());

        let mut words: Vec<String> = Vec::new();
        if let Some(a) = anchor {
            for w in &a.words {
                let w = w.trim().to_ascii_uppercase();
                if valid(&w) && !words.contains(&w) {
                    words.push(w);
                }
            }
        }
        let mut ai_words: Vec<String> = Vec::new();
        for w in cand {
            if ai_words.len() == AI_WORD_CAP {
                break;
            }
            if valid(w) && !ai_words.contains(w) {
                ai_words.push(w.clone());
            }
        }
        for w in &ai_words {
            if !words.contains(w) {
                words.push(w.clone());
            }
        }
        let mut pool: Vec<&String> = self.ctx.pack.pool.words(len).iter().collect();
        pool.shuffle(&mut self.rng);
        for w in pool {
            if words.len() >= target {
                break;
            }
            let w = w.trim().to_ascii_uppercase();
            if valid(&w) && !words.contains(&w) {
                words.push(w);
            }
        }

        let mut pw_rng = substream(self.ctx.run.seed, seed.depth, "password", 0);
        let anchor_pw = anchor
            .and_then(|a| a.password.as_deref())
            .map(|p| p.trim().to_ascii_uppercase())
            .filter(|p| valid(p));
        let password = match anchor_pw {
            Some(p) => p,
            None => {
                let picked = if ai_words.is_empty() {
                    words.choose(&mut pw_rng)
                } else {
                    ai_words.choose(&mut pw_rng)
                };
                match picked {
                    Some(p) => p.clone(),
                    None => "ABRAXAS".chars().cycle().take(len).collect(),
                }
            }
        };
        if !words.contains(&password) {
            words.insert(0, password.clone());
        }
        (words, password)
    }

    /// Authored files first, then candidate files (bad 8.3 names replaced), then pool files, up to
    /// `count` (raised to the authored count, so authored files are never dropped). When `target`
    /// is set that name is guaranteed to exist in the result: it replaces the last non-authored
    /// file, or is appended when every file is authored. Duplicate names get a `1`..`9` suffix.
    fn files(
        &mut self,
        authored: &[FileDef],
        cand: &[FileDef],
        pool: &[FileDef],
        count: usize,
        target: Option<&str>,
    ) -> Vec<FileDef> {
        let count = count.max(authored.len());
        let fill = |f: &Filler, d: &FileDef| FileDef {
            name: f.fill(&d.name).to_ascii_uppercase(),
            description: f.fill(&d.description),
            body: f.fill(&d.body),
        };
        let pool_files: Vec<FileDef> = {
            let mut v: Vec<FileDef> = pool.iter().map(|d| fill(&self.f, d)).collect();
            v.shuffle(&mut self.rng);
            v
        };
        let mut out: Vec<FileDef> = authored.iter().map(|d| fill(&self.f, d)).collect();
        let n_authored = out.len();
        let has = |out: &[FileDef], name: &str| out.iter().any(|d| d.name == name);

        for c in cand {
            if out.len() >= count {
                break;
            }
            let mut c = c.clone();
            if !is_83_name(&c.name) {
                c.name = pool_files
                    .iter()
                    .map(|p| p.name.clone())
                    .find(|n| is_83_name(n) && !has(&out, n) && !cand.iter().any(|x| &x.name == n))
                    .unwrap_or_else(|| format!("NOTE{:03}.TXT", out.len()));
            }
            out.push(c);
        }
        for p in &pool_files {
            if out.len() >= count {
                break;
            }
            if !has(&out, &p.name) {
                out.push(p.clone());
            }
        }
        let mut i = 0;
        while out.len() < count {
            // A pool too small to fill the list: reuse entries (renamed below) or synthesise.
            let mut d = pool_files.get(i).cloned().unwrap_or_else(|| FileDef {
                name: "NOTE.TXT".to_string(),
                description: "Scrap".to_string(),
                body: "The rest of the file is static.".to_string(),
            });
            if has(&out, &d.name) {
                d.name = "NOTE.TXT".to_string();
            }
            out.push(d);
            i += 1;
        }

        let mut target_idx = None;
        if let Some(t) = target {
            let idx = match out.iter().position(|d| d.name == t) {
                Some(i) => i,
                None if out.len() > n_authored => {
                    let i = out.len() - 1;
                    if let Some(slot) = out.get_mut(i) {
                        slot.name = t.to_string();
                    }
                    i
                }
                None => {
                    let mut d = pool_files.first().cloned().unwrap_or_else(|| FileDef {
                        name: t.to_string(),
                        description: "Scrap".to_string(),
                        body: "The rest of the file is static.".to_string(),
                    });
                    d.name = t.to_string();
                    out.push(d);
                    out.len() - 1
                }
            };
            target_idx = Some(idx);
        }

        // Unique names; the target keeps its name.
        let mut seen: Vec<String> = target.map(str::to_string).into_iter().collect();
        for (i, d) in out.iter_mut().enumerate() {
            if Some(i) == target_idx {
                continue;
            }
            if seen.contains(&d.name) {
                d.name = suffix_name(&d.name, &seen);
            }
            seen.push(d.name.clone());
        }
        out
    }

    /// Authored posts first (all kept), then candidates, then pool posts, up to
    /// `max(POST_COUNT, authored)`; a Shadow post is guaranteed from depth 1 on and absent at
    /// depth 0.
    fn posts(&mut self, authored: &[PostDef], cand: Vec<PostDef>) -> Vec<PostDef> {
        let depth = self.ctx.depth();
        let shadow = self.f.shadow.clone();
        let mut out: Vec<PostDef> = authored
            .iter()
            .map(|p| PostDef {
                from: self.f.fill(&p.from),
                subject: self.f.fill(&p.subject),
                body: self.f.fill(&p.body),
            })
            .collect();
        let slots = POST_COUNT.max(out.len());
        let n_authored = out.len();
        for c in cand {
            if out.len() >= slots {
                break;
            }
            if depth == 0 && c.from == shadow {
                continue;
            }
            out.push(c);
        }

        let mut pool: Vec<PostDef> = self
            .ctx
            .pack
            .pool
            .post
            .iter()
            .map(|p| PostDef {
                from: self.f.fill(&p.from),
                subject: self.f.fill(&p.subject),
                body: self.f.fill(&p.body),
            })
            .filter(|p| depth > 0 || p.from != shadow)
            .collect();
        pool.shuffle(&mut self.rng);
        let key = |p: &PostDef| format!("post:{}|{}", p.from, p.subject);
        let same = |out: &[PostDef], p: &PostDef| {
            out.iter()
                .any(|o| o.from == p.from && o.subject == p.subject)
        };
        for p in &pool {
            if out.len() >= slots {
                break;
            }
            if !self.ctx.run.used_pool.contains(&key(p)) && !same(&out, p) {
                out.push(p.clone());
            }
        }
        for p in &pool {
            if out.len() >= slots {
                break;
            }
            if !same(&out, p) {
                out.push(p.clone());
            }
        }
        let mut i = 0;
        while out.len() < slots {
            out.push(pool.get(i).cloned().unwrap_or_else(|| PostDef {
                from: "anonymous".to_string(),
                subject: "static".to_string(),
                body: "nothing but static on the line tonight.".to_string(),
            }));
            i += 1;
        }

        if depth >= 1 && !out.iter().any(|p| p.from == shadow) {
            let idx = (n_authored..out.len()).last().unwrap_or(out.len() - 1);
            out[idx].from = shadow;
        }
        out
    }
}

/// `NAME.EXT` with a `1`..`9` suffix on the base name that is not in `taken`.
fn suffix_name(name: &str, taken: &[String]) -> String {
    let (base, ext) = name.split_once('.').unwrap_or((name, "TXT"));
    let stem: String = base.chars().take(7).collect();
    (1..=9)
        .map(|d| format!("{stem}{d}.{ext}"))
        .find(|n| !taken.contains(n))
        .unwrap_or_else(|| name.to_string())
}

// ---------------------------------------------------------------------------------------------
// Passage
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct PassageCand {
    beat: Option<String>,
    prompt: Option<String>,
    options: Vec<OptionDef>,
    exits: Vec<ExitDef>,
    vision: Option<VisionKind>,
    captions: Vec<String>,
    summary: Option<String>,
}

fn passage_cand(generated: &PassageGen, f: &Filler) -> PassageCand {
    PassageCand {
        beat: f.ai_opt(&generated.beat, 700, true),
        prompt: f.ai_opt(&generated.choice.prompt, 160, false),
        options: generated
            .choice
            .options
            .iter()
            .filter_map(|o| {
                Some(OptionDef {
                    text: f.ai_opt(&o.text, 70, false)?,
                    stance: o.stance,
                    flags: clean_flags(&o.flags),
                })
            })
            .collect(),
        exits: generated
            .exits
            .iter()
            .filter_map(|e| {
                Some(ExitDef {
                    kind: e.kind,
                    text: f.ai_opt(&e.text, 60, false)?,
                })
            })
            .collect(),
        vision: (generated.vision != VisionKind::Noise).then_some(generated.vision),
        captions: generated
            .captions
            .iter()
            .filter_map(|c| f.ai_opt(c, 48, false))
            .collect(),
        summary: f.ai_opt(&generated.summary, 200, false),
    }
}

/// Validation steps 2 and 7-10 for a passage. Never fails.
pub fn fix_passage(generated: PassageGen, ctx: &GenContext, place: &Place) -> Passage {
    let f = Filler::new(ctx);
    let cand = passage_cand(&generated, &f);
    build_passage(ctx, place, cand, "fill-passage")
}

pub(crate) fn build_passage(
    ctx: &GenContext,
    place: &Place,
    cand: PassageCand,
    purpose: &str,
) -> Passage {
    let anchor = ctx.anchor();
    let cand = if anchor.is_some_and(|a| a.ai == AiMode::Off) {
        PassageCand::default()
    } else {
        cand
    };
    let mut rng = substream(ctx.run.seed, ctx.depth(), purpose, 0);
    let f = Filler::new(ctx);
    let pool = &ctx.pack.pool;

    // Beat (adapted). An AI beat that drops a quoted span of `required` is not valid.
    let required = anchor
        .and_then(|a| a.required.as_deref())
        .map(|r| f.fill(r))
        .unwrap_or_default();
    let ai_beat = cand
        .beat
        .filter(|b| quoted_spans(&required).iter().all(|q| contains_words(b, q)));
    let beat = match (ai_beat, anchor.and_then(|a| a.beat.as_deref())) {
        (Some(ai), _) => ai,
        (None, Some(authored)) => f.fill(authored),
        (None, None) => match pool.beats.choose(&mut rng) {
            Some(b) => f.fill(b),
            None => "The file ends in static.".to_string(),
        },
    };

    // Choice (locked by the anchor when authored).
    let mut pool_order: Vec<&ChoiceDef> = pool.choice.iter().collect();
    pool_order.shuffle(&mut rng);
    let (prompt, options) = match anchor.and_then(|a| a.choice.as_ref()) {
        Some(c) => (
            Some(f.fill(&c.prompt)),
            c.options
                .iter()
                .map(|o| OptionDef {
                    text: f.fill(&o.text),
                    stance: o.stance,
                    flags: clean_flags(&o.flags),
                })
                .collect(),
        ),
        None => (cand.prompt, cand.options),
    };
    let prompt = prompt
        .or_else(|| pool_order.first().map(|c| f.fill(&c.prompt)))
        .unwrap_or_else(|| "The payload is open on your screen. What do you do?".to_string());
    let options = repair_options(options, &pool_order, &f);

    // Exits (locked by the anchor when authored).
    let exits = match anchor.map(|a| &a.exit).filter(|e| !e.is_empty()) {
        Some(e) => e
            .iter()
            .map(|x| ExitDef {
                kind: x.kind,
                text: f.fill(&x.text),
            })
            .collect(),
        None => cand.exits,
    };
    let exits = repair_exits(exits, ctx.pack, &f, &mut rng);

    // Vision (locked), else the AI's, else the act's default.
    let vision = anchor
        .and_then(|a| a.vision)
        .or(cand.vision)
        .or_else(|| {
            ctx.pack
                .act_for_depth(ctx.depth())
                .map(|a| a.default_vision)
        })
        .filter(|v| *v != VisionKind::Noise)
        .unwrap_or(VisionKind::Fire);

    // Captions (adapted): a full AI set, else the authored set, else partial AI padded from the pool.
    let authored_captions: Vec<String> = anchor
        .map(|a| a.captions.iter().map(|c| f.fill(c)).collect())
        .unwrap_or_default();
    let mut captions = if cand.captions.len() >= 3 || authored_captions.is_empty() {
        cand.captions
    } else {
        authored_captions
    };
    let mut shuffled: Vec<&String> = pool.captions.iter().collect();
    shuffled.shuffle(&mut rng);
    for c in shuffled {
        if captions.len() >= 3 {
            break;
        }
        let c = f.fill(c);
        if !captions.contains(&c) {
            captions.push(c);
        }
    }
    let mut n = 1;
    while captions.len() < 3 {
        captions.push(format!("DOWN {n}"));
        n += 1;
    }
    captions.truncate(3);

    // Summary (adapted).
    let summary = match (cand.summary, anchor.and_then(|a| a.summary.as_deref())) {
        (Some(ai), _) => ai,
        (None, Some(authored)) => f.fill(authored),
        (None, None) => format!("{} passed through {}.", ctx.run.handle, place.title),
    };

    Passage {
        beat,
        choice: ChoiceDef { prompt, options },
        exits,
        vision,
        captions,
        summary,
    }
}

/// Exactly one option per stance: the first option of each stance keeps it, duplicates take the
/// next missing stance (integrate, repress, neutral), and missing ones come from the pool.
fn repair_options(mut options: Vec<OptionDef>, pool: &[&ChoiceDef], f: &Filler) -> Vec<OptionDef> {
    options.truncate(3);
    let mut taken: Vec<Stance> = Vec::new();
    let mut dupes: Vec<usize> = Vec::new();
    for (i, o) in options.iter().enumerate() {
        if taken.contains(&o.stance) {
            dupes.push(i);
        } else {
            taken.push(o.stance);
        }
    }
    for i in dupes {
        if let Some(s) = Stance::ALL.iter().find(|s| !taken.contains(s)) {
            options[i].stance = *s;
            taken.push(*s);
        }
    }
    for stance in Stance::ALL {
        if taken.contains(&stance) {
            continue;
        }
        let from_pool = pool
            .iter()
            .find_map(|c| c.options.iter().find(|o| o.stance == stance));
        options.push(match from_pool {
            Some(o) => OptionDef {
                text: truncate(&f.fill(&o.text), 70),
                stance,
                flags: clean_flags(&o.flags),
            },
            None => OptionDef {
                text: match stance {
                    Stance::Integrate => "Face it.",
                    Stance::Repress => "Turn away.",
                    Stance::Neutral => "Wait and watch.",
                }
                .to_string(),
                stance,
                flags: Vec::new(),
            },
        });
    }
    options
}

/// Exactly three exits, ordered calm, deep, mirror; duplicates of a kind take the next missing
/// kind and missing kinds come from the pool.
fn repair_exits(
    mut exits: Vec<ExitDef>,
    pack: &StoryPack,
    f: &Filler,
    rng: &mut Rng,
) -> Vec<ExitDef> {
    exits.truncate(3);
    let mut taken: Vec<ExitKind> = Vec::new();
    let mut dupes: Vec<usize> = Vec::new();
    for (i, e) in exits.iter().enumerate() {
        if taken.contains(&e.kind) {
            dupes.push(i);
        } else {
            taken.push(e.kind);
        }
    }
    for i in dupes {
        if let Some(k) = ExitKind::ALL.iter().find(|k| !taken.contains(k)) {
            exits[i].kind = *k;
            taken.push(*k);
        }
    }
    for kind in ExitKind::ALL {
        if taken.contains(&kind) {
            continue;
        }
        let options: Vec<&ExitDef> = pack.pool.exits_of(kind).collect();
        let text = match options.choose(rng) {
            Some(e) => truncate(&f.fill(&e.text), 60),
            None => "A way down.".to_string(),
        };
        exits.push(ExitDef { kind, text });
    }
    exits.sort_by_key(|e| e.kind as u8);
    exits
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::seed::level_seed;
    use crate::pack;
    use crate::text::is_cp437;
    use serde_json::json;

    pub(crate) fn run(seed: u64) -> RunState {
        RunState::new("Tester", "Jo Doe", "a dream", seed, "rollover", None)
    }

    pub(crate) fn seed_at(run: &RunState, pack: &StoryPack, depth: u32) -> LevelSeed {
        level_seed(run, pack, depth)
    }

    /// A pack without the depth-0 anchor, so depth 0 behaves like an ordinary level.
    fn pack_without_anchor_0() -> StoryPack {
        let mut p = pack::embedded().unwrap();
        p.anchors.retain(|a| a.anchor.depth != 0);
        p
    }

    fn place_json() -> serde_json::Value {
        json!({
            "title": "THE WET HALL",
            "bbs_name": "DAMP CARPET",
            "tagline": "Mind the floor.",
            "level_card": "Field note: a hall, a hum.",
            "sysop": "Dr Static!!",
            "host": {
                "name": "ODS TEST EXCHANGE", "os": "vax", "banner": "Welcome.",
                "objective": "Pull NOTES.TXT from the vault.", "target_file": "notes.txt",
                "node_names": ["alpha", "beta"], "ice_names": ["WOLF"]
            },
            "posts": [
                {"from": "ghost", "subject": "hi", "body": "hello {handle}"},
                {"from": "{shadow}", "subject": "me", "body": "i am you"}
            ],
            "text_files": [{"name": "A.TXT", "description": "a", "body": "aaa"}],
            "host_files": [
                {"name": "bad name!", "description": "b", "body": "bbb"},
                {"name": "NOTES.TXT", "description": "n", "body": "nnn"},
                {"name": "notes.txt", "description": "n2", "body": "nnn2"}
            ],
            "words": ["ravens", "CINDER", "toolongword", "AB", "CINDER", "emb3rs"]
        })
    }

    fn parse(depth: u32, mutate: impl FnOnce(&mut LevelSeed), v: serde_json::Value) -> Place {
        let pack = pack::embedded().unwrap();
        let r = run(11);
        let mut seed = seed_at(&r, &pack, depth);
        mutate(&mut seed);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        parse_place(&v.to_string(), &ctx).unwrap()
    }

    fn mask_of(place: &Place) -> String {
        let clue = place.posts.last().unwrap();
        let body = &clue.body;
        let start = body.find("it was ").unwrap() + "it was ".len();
        body[start..body.len() - 1].to_string()
    }

    #[test]
    fn extract_json_handles_fences_preamble_and_think() {
        let raw = "Sure! Here you go:\n```json\n{\"a\": {\"b\": 1}}\n```\nHope that helps.";
        assert_eq!(extract_json(raw).unwrap(), "{\"a\": {\"b\": 1}}");
        let raw = "<think>I should output {\"wrong\": 1} maybe</think>\n{\"right\": 2}";
        assert_eq!(extract_json(raw).unwrap(), "{\"right\": 2}");
        assert_eq!(
            extract_json("<think>never closes {\"a\":1}"),
            Err(ValidationError::NoJson)
        );
        assert_eq!(extract_json("no braces"), Err(ValidationError::NoJson));
        assert_eq!(extract_json("} backwards {"), Err(ValidationError::NoJson));
    }

    #[test]
    fn parse_failures_are_errors_not_repairs() {
        let pack = pack::embedded().unwrap();
        let r = run(1);
        let seed = seed_at(&r, &pack, 5);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        assert!(matches!(
            parse_place("{\"title\": 5}", &ctx),
            Err(ValidationError::Json(_))
        ));
        assert_eq!(parse_place("garbage", &ctx), Err(ValidationError::NoJson));
    }

    #[test]
    fn words_filtered_deduped_topped_up_and_password_is_an_ai_word() {
        let place = parse(5, |_| {}, place_json());
        // Depth 5: danger 2, 6-letter words, 10 words per lock -> 20 candidates.
        assert_eq!(place.words.len(), 20);
        assert!(
            place
                .words
                .iter()
                .all(|w| w.len() == 6 && w.bytes().all(|b| b.is_ascii_uppercase()))
        );
        let mut sorted = place.words.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 20, "duplicates in {:?}", place.words);
        assert_eq!(&place.words[..2], ["RAVENS", "CINDER"]);
        assert!(["RAVENS", "CINDER"].contains(&place.password.as_str()));
        assert_eq!(mask_of(&place).len(), 6);
    }

    #[test]
    fn password_falls_back_to_the_pool_when_the_ai_gave_no_words() {
        let mut v = place_json();
        v["words"] = json!([]);
        let place = parse(5, |_| {}, v);
        assert!(place.words.contains(&place.password));
        assert_eq!(place.words.len(), 20);
    }

    #[test]
    fn clue_mask_reveals_two_or_three_letters() {
        let mut rng = substream(1, 1, "t", 0);
        for (word, shown) in [("OWLET", 2), ("RAVENS", 2), ("TOWERED", 3), ("MIDNIGHT", 3)] {
            let post = clue_post("ODS X", word, &mut rng);
            assert_eq!(post.from, "anonymous");
            assert_eq!(post.subject, "re: front door");
            let body = post.body;
            assert!(body.starts_with("whoever runs ODS X never changed the front door."));
            let mask = body
                ["whoever runs ODS X never changed the front door. last time i looked it was "
                    .len()..]
                .trim_end_matches('.');
            assert_eq!(mask.len(), word.len());
            assert_eq!(mask.chars().filter(|c| *c != '_').count(), shown, "{mask}");
            for (m, w) in mask.chars().zip(word.chars()) {
                assert!(m == '_' || m == w);
            }
        }
    }

    #[test]
    fn clue_is_the_fourth_post_and_matches_the_password() {
        let place = parse(5, |_| {}, place_json());
        assert_eq!(place.posts.len(), 4);
        assert_eq!(place.posts[3].subject, "re: front door");
        assert!(place.posts[3].body.contains("ODS TEST EXCHANGE"));
        for (m, w) in mask_of(&place).chars().zip(place.password.chars()) {
            assert!(m == '_' || m == w);
        }
    }

    #[test]
    fn file_names_repaired_and_deduplicated_and_target_kept() {
        let place = parse(
            5,
            |s| {
                s.objective_kind = ObjectiveKind::Retrieve;
                s.target_daemon = None;
            },
            place_json(),
        );
        let names: Vec<&str> = place.host_files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| is_83_name(n)), "{names:?}");
        assert_eq!(names.iter().filter(|n| **n == "NOTES.TXT").count(), 1);
        assert_eq!(place.host.target_file, "NOTES.TXT");

        // Duplicates get a numeric suffix.
        let mut v = place_json();
        v["host_files"] = json!([
            {"name": "NOTES.TXT", "description": "n", "body": "nnn"},
            {"name": "notes.txt", "description": "n2", "body": "nnn2"}
        ]);
        let place = parse(
            5,
            |s| {
                s.objective_kind = ObjectiveKind::Retrieve;
                s.target_daemon = None;
            },
            v,
        );
        let names: Vec<&str> = place.host_files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["NOTES.TXT", "NOTES1.TXT"]);
    }

    #[test]
    fn retrieve_target_gets_a_host_file_when_the_ai_forgot_it() {
        let mut v = place_json();
        v["host"]["target_file"] = json!("LOST.LOG");
        let place = parse(
            5,
            |s| {
                s.objective_kind = ObjectiveKind::Retrieve;
                s.target_daemon = None;
            },
            v,
        );
        assert_eq!(place.host.target_file, "LOST.LOG");
        assert_eq!(
            place
                .host_files
                .iter()
                .filter(|f| f.name == "LOST.LOG")
                .count(),
            1
        );
        assert_eq!(place.host_files.len(), 2);
    }

    #[test]
    fn plant_and_crash_targets() {
        let plant = parse(5, |s| s.objective_kind = ObjectiveKind::Plant, place_json());
        assert_eq!(plant.host.target_file, "PAYLOAD.PIF");
        let crash = parse(
            5,
            |s| {
                s.objective_kind = ObjectiveKind::Crash;
                s.target_daemon = Some("pleroma".into());
            },
            place_json(),
        );
        assert_eq!(crash.host.target_file, "");
        assert!(
            crash.host.objective.ends_with("(process pleroma)"),
            "{}",
            crash.host.objective
        );
        let mut v = place_json();
        v["host"]["objective"] = json!("Kill the Pleroma daemon.");
        let named = parse(
            5,
            |s| {
                s.objective_kind = ObjectiveKind::Crash;
                s.target_daemon = Some("pleroma".into());
            },
            v,
        );
        assert_eq!(named.host.objective, "Kill the Pleroma daemon.");
    }

    #[test]
    fn shadow_post_is_forced_from_depth_one_but_never_at_depth_zero() {
        let mut v = place_json();
        v["posts"] = json!([
            {"from": "a", "subject": "s1", "body": "b1"},
            {"from": "b", "subject": "s2", "body": "b2"},
            {"from": "c", "subject": "s3", "body": "b3"},
            {"from": "d", "subject": "s4", "body": "b4"}
        ]);
        let r = run(11);
        let shadow = r.shadow();
        let at5 = parse(5, |_| {}, v.clone());
        // Three AI posts, the last one forced to the Shadow, then the clue.
        assert_eq!(at5.posts.len(), 4);
        assert_eq!(at5.posts[2].from, shadow);
        assert_eq!(
            at5.posts[..3].iter().filter(|p| p.from == shadow).count(),
            1
        );

        // Depth 0 without an anchor: an AI shadow post is dropped, none is forced.
        let pack = pack_without_anchor_0();
        let seed = seed_at(&r, &pack, 0);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        let mut with_shadow = place_json();
        with_shadow["posts"][0]["from"] = json!(shadow);
        let at0 = parse_place(&with_shadow.to_string(), &ctx).unwrap();
        assert!(
            at0.posts.iter().all(|p| p.from != shadow),
            "{:?}",
            at0.posts
        );
        assert_eq!(at0.posts.len(), 4);
    }

    #[test]
    fn shadow_literal_and_handle_both_become_the_run_shadow() {
        let r = run(11);
        let mut v = place_json();
        v["posts"][0]["from"] = json!(r.shadow().to_uppercase());
        let place = parse(5, |_| {}, v);
        let shadow_posts = place.posts[..3]
            .iter()
            .filter(|p| p.from == r.shadow())
            .count();
        assert!(shadow_posts >= 1);
        assert!(place.posts.iter().all(|p| p.from != "{shadow}"));
    }

    #[test]
    fn locked_anchor_fields_override_the_ai_and_adapted_fields_use_valid_ai() {
        let mut v = place_json();
        v["title"] = json!("AI TITLE");
        v["bbs_name"] = json!("AI BBS");
        v["sysop"] = json!("somebody");
        v["host"]["name"] = json!("AI HOST");
        v["host"]["target_file"] = json!("OTHER.LOG");
        v["level_card"] = json!("An adapted field note.");
        v["words"] = json!(["PLAINS"]);
        let place = parse(3, |_| {}, v.clone());
        assert_eq!(place.title, "THE BLACK SUN");
        assert_eq!(place.bbs_name, "SOL NIGER");
        assert_eq!(place.sysop.cast_id.as_deref(), Some("shadow"));
        assert_eq!(place.host.name, "ODS LOWTIDE EXCHANGE");
        assert_eq!(place.host.target_file, "WREN-01.LOG");
        assert_eq!(place.password, "RAVENS");
        assert_eq!(place.words[0], "RAVENS");
        assert!(place.words.contains(&"PLAINS".to_string()));
        assert_eq!(place.host.node_names[0], "TRUNK ROOM");
        assert_eq!(place.level_card, "An adapted field note.");
        assert!(place.host_files.iter().any(|f| f.name == "WREN-01.LOG"));
        // Authored posts come first.
        assert_eq!(place.posts[0].subject, "i have been posting as you");

        // An empty AI level card falls back to the authored text.
        v["level_card"] = json!("   ");
        let fallback = parse(3, |_| {}, v);
        assert!(
            fallback
                .level_card
                .starts_with("Field note: Level 3 is a switchboard hall")
        );
    }

    #[test]
    fn text_is_sanitised_filled_and_cp437() {
        let mut v = place_json();
        v["posts"][0]["body"] = json!("hi {handle} \u{4e2d} \u{201c}quoted\u{201d} |12pipe\ttab");
        v["tagline"] = json!("x".repeat(200));
        let place = parse(5, |_| {}, v);
        let body = &place.posts[0].body;
        assert_eq!(body, "hi Tester ? \"quoted\" pipe tab");
        assert_eq!(place.tagline.chars().count(), 60);
        assert!(place.tagline.ends_with("..."));
        for p in &place.posts {
            assert!(p.body.chars().all(is_cp437));
        }
    }

    #[test]
    fn sysop_resolution() {
        let pack = pack::embedded().unwrap();
        let r = run(3);
        let shadow = r.shadow();
        let at = |depth: u32, req: &str| {
            let seed = seed_at(&r, &pack, depth);
            let ctx = GenContext {
                run: &r,
                pack: &pack,
                seed: &seed,
            };
            sysop_for(&ctx, req)
        };
        let s = at(5, "PHILEMON");
        assert_eq!(
            (s.handle.as_str(), s.cast_id.as_deref()),
            ("PHILEMON", Some("philemon"))
        );
        assert_eq!(at(5, "Philemon").cast_id.as_deref(), Some("philemon"));
        let s = at(5, "{shadow}");
        assert_eq!(
            (s.handle.as_str(), s.cast_id.as_deref()),
            (shadow.as_str(), Some("shadow"))
        );
        // Not yet in the story: replaced by a pool sysop, never impersonated.
        let early = at(2, "mercurius");
        assert!(early.cast_id.is_none());
        assert_ne!(early.handle.to_uppercase(), "MERCURIUS");
        assert!(at(0, "shadow").cast_id.is_none());
        let npc = at(5, "Dr Static!!");
        assert_eq!((npc.handle.as_str(), npc.cast_id), ("Dr_Static", None));
        assert!(at(5, "a-very-long-handle-name-indeed").handle.len() <= 16);
        let pool = at(5, "x");
        assert!(
            pack.pool
                .sysops
                .iter()
                .any(|p| sanitize_handle(p) == pool.handle)
        );
        assert_eq!(at(5, "x"), pool);
    }

    #[test]
    fn empty_ai_output_is_fully_repaired() {
        let empty = json!({
            "title": "", "bbs_name": "", "tagline": "", "level_card": "", "sysop": "",
            "host": {"name": "", "os": "unix", "banner": "", "objective": "", "target_file": "",
                     "node_names": [], "ice_names": []},
            "posts": [], "text_files": [], "host_files": [], "words": []
        });
        let place = parse(9, |_| {}, empty);
        assert_eq!(place.posts.len(), 4);
        assert_eq!(place.text_files.len(), 2);
        assert_eq!(place.host_files.len(), 2);
        assert_eq!(place.host.node_names.len(), 8);
        assert_eq!(place.host.ice_names.len(), 3);
        assert!(
            !place.title.is_empty() && !place.bbs_name.is_empty() && !place.level_card.is_empty()
        );
        assert!(place.words.contains(&place.password));
    }

    fn long_text(n: usize) -> String {
        "abcdefghi ".repeat(n / 10 + 1)
    }

    #[test]
    fn ai_bodies_are_truncated_at_400_with_ellipsis() {
        let mut v = place_json();
        let long = long_text(900);
        v["posts"][0]["body"] = json!(long);
        v["text_files"][0]["body"] = json!(long);
        v["host_files"][0]["body"] = json!(long);
        let place = parse(5, |_| {}, v);
        for body in [
            &place.posts[0].body,
            &place.text_files[0].body,
            &place.host_files[0].body,
        ] {
            assert!(body.chars().count() <= 400, "{}", body.chars().count());
            assert!(body.ends_with("..."), "{body}");
        }
    }

    #[test]
    fn authored_anchor_text_over_400_is_not_truncated() {
        let mut pack = pack::embedded().unwrap();
        let long = long_text(700);
        {
            let a = &mut pack
                .anchors
                .iter_mut()
                .find(|e| e.anchor.depth == 3)
                .unwrap()
                .anchor;
            a.post.insert(
                0,
                crate::pack::model::PostDef {
                    from: "authored".into(),
                    subject: "long".into(),
                    body: long.clone(),
                },
            );
            a.text_file.insert(
                0,
                crate::pack::model::FileDef {
                    name: "LONG.TXT".into(),
                    description: "long".into(),
                    body: long.clone(),
                },
            );
            a.host_file.insert(
                0,
                crate::pack::model::FileDef {
                    name: "LONGH.TXT".into(),
                    description: "long".into(),
                    body: long.clone(),
                },
            );
        }
        let r = run(11);
        let seed = seed_at(&r, &pack, 3);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        let mut v = place_json();
        v["posts"][0]["body"] = json!(long_text(900));
        let place = parse_place(&v.to_string(), &ctx).unwrap();
        assert_eq!(place.posts[0].body, long);
        assert_eq!(place.text_files[0].body, long);
        assert_eq!(place.host_files[0].body, long);
    }

    #[test]
    fn anchor_slot_counts_keep_every_authored_entry() {
        let pack = pack::embedded().unwrap();
        for depth in [0u32, 3, 7, 11, 15] {
            let a = &pack.anchor_at(depth).unwrap().anchor;
            let r = run(5);
            let seed = seed_at(&r, &pack, depth);
            let ctx = GenContext {
                run: &r,
                pack: &pack,
                seed: &seed,
            };
            for place in [
                parse_place(&place_json().to_string(), &ctx).unwrap(),
                build_place(&ctx, PlaceCand::default(), "offline-place"),
            ] {
                assert_eq!(
                    place.posts.len(),
                    POST_COUNT.max(a.post.len()) + 1,
                    "L{depth}"
                );
                assert_eq!(
                    place.text_files.len(),
                    TEXT_FILE_COUNT.max(a.text_file.len()),
                    "L{depth}"
                );
                assert_eq!(
                    place.host_files.len(),
                    HOST_FILE_COUNT.max(a.host_file.len()),
                    "L{depth}"
                );
                for (got, want) in place.posts.iter().zip(&a.post) {
                    assert_eq!(got.subject, want.subject);
                }
            }
        }
        // Anchor 0 is the largest: 4 authored posts + clue, 2 text files, 3 host files.
        let r = run(5);
        let seed = seed_at(&r, &pack, 0);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        let p = build_place(&ctx, PlaceCand::default(), "offline-place");
        assert_eq!(
            (p.posts.len(), p.text_files.len(), p.host_files.len()),
            (5, 2, 3)
        );
    }

    #[test]
    fn retrieve_target_survives_in_two_host_files_everywhere() {
        let pack = pack_without_anchor_0();
        for seed in 0..100u64 {
            let r = run(seed);
            for danger in 1..=5u8 {
                for depth in [1u32, 5, 9] {
                    let mut s = seed_at(&r, &pack, depth);
                    s.base_danger = danger;
                    s.objective_kind = ObjectiveKind::Retrieve;
                    s.target_daemon = None;
                    let ctx = GenContext {
                        run: &r,
                        pack: &pack,
                        seed: &s,
                    };
                    let offline = build_place(&ctx, PlaceCand::default(), "offline-place");
                    let mut v = place_json();
                    v["host"]["target_file"] = json!("LOST.LOG");
                    let ai = parse_place(&v.to_string(), &ctx).unwrap();
                    for (label, place) in [("offline", offline), ("ai", ai)] {
                        let ctxt = format!("{label} seed {seed} D{danger} depth {depth}");
                        assert_eq!(place.host_files.len(), 2, "{ctxt}");
                        assert_eq!(place.posts.len(), 4, "{ctxt}");
                        assert_eq!(place.text_files.len(), 2, "{ctxt}");
                        assert!(
                            place
                                .host_files
                                .iter()
                                .any(|f| f.name == place.host.target_file),
                            "{ctxt}"
                        );
                    }
                }
            }
        }
    }

    // ---- passage ----

    fn passage_json() -> serde_json::Value {
        json!({
            "beat": "The file opens. It knows your name.",
            "choice": {
                "prompt": "What now?",
                "options": [
                    {"text": "Read it aloud", "stance": "integrate", "flags": ["Bad Flag", "ok_flag", "also_ok", "third_ok"]},
                    {"text": "Read it again", "stance": "integrate", "flags": []},
                    {"text": "Read it never", "stance": "integrate", "flags": []}
                ]
            },
            "exits": [
                {"kind": "mirror", "text": "first mirror"},
                {"kind": "mirror", "text": "second mirror"},
                {"kind": "calm", "text": "a calm way"}
            ],
            "vision": "noise",
            "captions": ["ONE", "TWO"],
            "summary": "You read the file."
        })
    }

    fn passage_at(depth: u32, v: serde_json::Value) -> (Passage, Place) {
        let pack = pack::embedded().unwrap();
        let r = run(11);
        let seed = seed_at(&r, &pack, depth);
        let ctx = GenContext {
            run: &r,
            pack: &pack,
            seed: &seed,
        };
        let place = crate::director::offline::place(&ctx);
        let passage = parse_passage(&v.to_string(), &ctx, &place).unwrap();
        (passage, place)
    }

    #[test]
    fn duplicate_stances_take_the_next_missing_stance_in_order() {
        let (p, _) = passage_at(5, passage_json());
        let stances: Vec<Stance> = p.choice.options.iter().map(|o| o.stance).collect();
        assert_eq!(
            stances,
            [Stance::Integrate, Stance::Repress, Stance::Neutral]
        );
        assert_eq!(p.choice.options[0].text, "Read it aloud");
        assert_eq!(p.choice.options[1].text, "Read it again");
        assert_eq!(p.choice.options[0].flags, ["ok_flag", "also_ok"]);
        assert_eq!(p.choice.prompt, "What now?");
    }

    #[test]
    fn missing_options_come_from_the_pool() {
        let mut v = passage_json();
        v["choice"]["options"] = json!([{"text": "Only one", "stance": "repress", "flags": []}]);
        let (p, _) = passage_at(5, v);
        let mut stances: Vec<Stance> = p.choice.options.iter().map(|o| o.stance).collect();
        stances.sort();
        assert_eq!(
            stances,
            [Stance::Integrate, Stance::Repress, Stance::Neutral]
        );
        assert_eq!(p.choice.options[0].text, "Only one");
        assert!(p.choice.options.iter().all(|o| !o.text.is_empty()));
    }

    #[test]
    fn exits_are_repaired_and_ordered_calm_deep_mirror() {
        let (p, _) = passage_at(5, passage_json());
        let kinds: Vec<ExitKind> = p.exits.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [ExitKind::Calm, ExitKind::Deep, ExitKind::Mirror]);
        assert_eq!(p.exits[0].text, "a calm way");
        assert_eq!(p.exits[1].text, "second mirror");
        assert_eq!(p.exits[2].text, "first mirror");
    }

    #[test]
    fn noise_vision_becomes_the_act_default_and_captions_pad_to_three() {
        let (p, _) = passage_at(5, passage_json());
        assert_eq!(p.vision, VisionKind::Plasma); // albedo's default
        assert_eq!(p.captions.len(), 3);
        assert_eq!(&p.captions[..2], ["ONE", "TWO"]);
        assert!(!p.captions[2].is_empty());
        assert_eq!(p.summary, "You read the file.");
    }

    #[test]
    fn passage_locked_and_adapted_anchor_fields() {
        let mut v = passage_json();
        v["beat"] = json!("The file opens. Abraxas is not a place, it is what holds both.");
        let (p, _) = passage_at(3, v);
        // Locked: authored choice, exits and vision win over the AI's.
        assert!(p.choice.options[0].text.starts_with("Answer the Shadow"));
        assert_eq!(p.vision, VisionKind::Fire);
        assert_ne!(p.exits[0].text, "a calm way");
        // Adapted: valid AI beat and summary are used; an empty one falls back to authored text.
        assert_eq!(
            p.beat,
            "The file opens. Abraxas is not a place, it is what holds both."
        );
        let mut v = passage_json();
        v["beat"] = json!("");
        v["summary"] = json!("");
        v["captions"] = json!(["A", "B"]);
        let (q, _) = passage_at(3, v);
        assert!(q.beat.contains("Abraxas is not a place"));
        assert!(q.summary.contains("met the Shadow running SOL NIGER"));
        assert_eq!(q.captions[0], "SOL NIGER");
    }

    #[test]
    fn ai_beat_missing_a_required_quote_falls_back_to_the_authored_beat() {
        let mut v = passage_json();
        v["beat"] = json!("The sun came up black. Wren wrote that the board holds everything.");
        let (p, _) = passage_at(3, v);
        assert!(p.beat.starts_with("WREN-01.LOG. 12/11/99"));
        let mut v = passage_json();
        v["beat"] = json!("A black sun. ABRAXAS is not a place -- it is what holds both sides.");
        let (p, _) = passage_at(3, v);
        assert!(p.beat.starts_with("A black sun."));
    }

    #[test]
    fn overlong_beat_is_truncated() {
        let mut v = passage_json();
        v["beat"] = json!("word ".repeat(400));
        let (p, _) = passage_at(5, v);
        assert_eq!(p.beat.chars().count(), 700);
        assert!(p.beat.ends_with("..."));
    }

    #[test]
    fn empty_passage_is_fully_repaired() {
        let v = json!({
            "beat": "", "choice": {"prompt": "", "options": []}, "exits": [],
            "vision": "noise", "captions": [], "summary": ""
        });
        let (p, place) = passage_at(9, v);
        assert_eq!(p.choice.options.len(), 3);
        assert_eq!(p.exits.len(), 3);
        assert_eq!(p.captions.len(), 3);
        assert!(!p.beat.is_empty() && !p.choice.prompt.is_empty());
        assert_eq!(p.summary, format!("Tester passed through {}.", place.title));
    }
}
