//! The AI director: schedules generation jobs, falls back to the offline generator, and reports
//! results as `DirectorEvent`s.
//!
//! Every request snapshots what it needs into an owned job and runs on its own thread (or inline
//! when there is no `Llm`). A job always ends in exactly one Ready/Done event unless it was
//! cancelled: AI failures after the retry chain, and even a panic inside a job, produce the
//! offline result instead, so the game never waits on the AI.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::config::AiConfig;
use crate::game::seed;
use crate::game::state::{GenSource, LevelSeed, Passage, Place, RunState};
use crate::llm::{Llm, LlmError, LlmReply, LlmRequest, Message};
use crate::pack::model::{AiMode, EndingDef, StoryPack};
use crate::text::clean_ai_text;
use validate::{GenContext, ValidationError};

pub mod offline;
pub mod prompts;
pub mod schema;
pub mod stream;
pub mod validate;

/// The kinds of generation job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobKind {
    Place,
    Passage,
    Interlude,
    Chat,
    Epilogue,
}

/// Results and modem-light signals sent from generation threads to the UI.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum DirectorEvent {
    PlaceReady {
        depth: u32,
        place: Place,
        source: GenSource,
    },
    PassageReady {
        depth: u32,
        passage: Passage,
        source: GenSource,
    },
    InterludeDelta {
        depth: u32,
        text: String,
    },
    InterludeDone {
        depth: u32,
        text: String,
        source: GenSource,
    },
    ChatDelta {
        session: u64,
        text: String,
    },
    ChatDone {
        session: u64,
        text: String,
        source: GenSource,
    },
    EpilogueDelta {
        text: String,
    },
    EpilogueDone {
        text: String,
        source: GenSource,
    },
    /// A request went out (SD light).
    Request {
        kind: JobKind,
    },
    /// A chunk arrived (RD light).
    Chunk,
    /// A call succeeded (CD green).
    CallOk,
    /// A call failed (CD red).
    CallFailed,
}

/// Who the player is chatting with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPartner {
    pub handle: String,
    /// Set for cast members; `None` for an NPC sysop.
    pub cast_id: Option<String>,
}

/// One line of a chat transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub from_player: bool,
    pub text: String,
}

/// Default pause before the single transport/empty-reply retry.
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Longest error text kept as a `GenSource::Offline` reason.
const REASON_MAX: usize = 100;
/// A partial text reply shorter than this is not worth keeping.
const MIN_PARTIAL_CHARS: usize = 20;

const INTERLUDE_MAX_CHARS: usize = 700;
const EPILOGUE_MAX_CHARS: usize = 1800;
const CHAT_MAX_CHARS: usize = 320;

/// The live director. Requests are idempotent per `(JobKind, depth)`: a part that is already in
/// flight is never requested again until its Ready/Done event passes through [`on_event`].
///
/// [`on_event`]: Director::on_event
pub struct Director {
    llm: Option<Arc<dyn Llm>>,
    pack: Arc<StoryPack>,
    ai: AiConfig,
    tx: Sender<DirectorEvent>,
    pending: BTreeSet<(JobKind, u32)>,
    cancel: Arc<AtomicBool>,
    retry_delay: Duration,
    next_session: u64,
}

/// Everything a job thread owns besides its snapshot.
#[derive(Clone)]
struct Env {
    llm: Option<Arc<dyn Llm>>,
    pack: Arc<StoryPack>,
    ai: AiConfig,
    tx: Sender<DirectorEvent>,
    cancel: Arc<AtomicBool>,
    retry_delay: Duration,
}

impl Env {
    fn send(&self, ev: DirectorEvent) {
        // A closed receiver means the game is shutting down.
        let _ = self.tx.send(ev);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The client to use for a level, or `None` when the AI is off for the run or for this
    /// anchor (`ai = "off"` anchors never call the AI).
    fn llm_for(&self, anchor_at: Option<&crate::pack::model::Anchor>) -> Option<&Arc<dyn Llm>> {
        self.llm
            .as_ref()
            .filter(|_| anchor_at.is_none_or(|a| a.ai != AiMode::Off))
    }

    /// Sleep for the retry delay in short slices; `false` when cancelled meanwhile.
    fn pause(&self) -> bool {
        let deadline = Instant::now() + self.retry_delay;
        while Instant::now() < deadline {
            if self.cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10).min(self.retry_delay));
        }
        !self.cancelled()
    }
}

/// The source of a result produced without any AI call.
fn authored_or_offline(has_anchor: bool) -> GenSource {
    if has_anchor {
        GenSource::Authored
    } else {
        GenSource::Offline {
            reason: String::new(),
        }
    }
}

fn offline_failed(reason: &str) -> GenSource {
    GenSource::Offline {
        reason: reason.to_string(),
    }
}

/// One line, at most `REASON_MAX` characters.
fn short_reason(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= REASON_MAX {
        line
    } else {
        let head: String = line.chars().take(REASON_MAX - 3).collect();
        format!("{head}...")
    }
}

fn millis_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn prompt_chars(req: &LlmRequest) -> usize {
    req.messages.iter().map(|m| m.content.chars().count()).sum()
}

// ---------------------------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------------------------

enum Fail {
    Cancelled,
    Reason(String),
}

fn fail_from(e: &LlmError) -> Fail {
    match e {
        LlmError::Cancelled => Fail::Cancelled,
        other => Fail::Reason(short_reason(&other.to_string())),
    }
}

enum Outcome<T> {
    Done(T, u64),
    Failed(String),
    Cancelled,
}

/// One HTTP call: reports the request, a chunk per delta and the call result to the modem lights.
fn call_once(
    env: &Env,
    llm: &dyn Llm,
    kind: JobKind,
    req: &LlmRequest,
    on_text: &mut dyn FnMut(&str),
) -> Result<LlmReply, LlmError> {
    env.send(DirectorEvent::Request { kind });
    let result = llm.stream(
        req,
        &mut |delta| {
            if !delta.is_empty() {
                env.send(DirectorEvent::Chunk);
                on_text(delta);
            }
        },
        &env.cancel,
    );
    match &result {
        Ok(_) => env.send(DirectorEvent::CallOk),
        Err(LlmError::Cancelled) => {}
        Err(_) => env.send(DirectorEvent::CallFailed),
    }
    result
}

/// A JSON call; a transport error (nothing received) is retried once after the retry delay.
fn call_json(
    env: &Env,
    llm: &dyn Llm,
    kind: JobKind,
    req: &LlmRequest,
    response_chars: &mut usize,
) -> Result<LlmReply, Fail> {
    let mut result = call_once(env, llm, kind, req, &mut |_| {});
    if matches!(result, Err(LlmError::Transport(_))) {
        if !env.pause() {
            return Err(Fail::Cancelled);
        }
        result = call_once(env, llm, kind, req, &mut |_| {});
    }
    let reply = result.map_err(|e| fail_from(&e))?;
    *response_chars += reply.text.chars().count();
    Ok(reply)
}

fn repair_message(error: &ValidationError) -> String {
    format!("Your JSON was invalid: {error}. Reply again with corrected JSON only.")
}

/// The JSON chain: call, parse; on a parse failure one repair call with the bad reply and the
/// error appended; then give up.
fn json_job<T>(
    env: &Env,
    llm: &dyn Llm,
    kind: JobKind,
    depth: u32,
    make_req: &dyn Fn(u32) -> LlmRequest,
    parse: &dyn Fn(&str) -> Result<T, ValidationError>,
) -> Outcome<T> {
    let started = Instant::now();
    let first = make_req(0);
    let mut response_chars = 0usize;
    let mut sent_chars = prompt_chars(&first);
    let result = (|| {
        let reply = call_json(env, llm, kind, &first, &mut response_chars)?;
        tracing::debug!(?kind, depth, prompt = ?first.messages, response = %reply.text, "ai call");
        let error = match parse(&reply.text) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let mut repair = make_req(1);
        repair.messages = first.messages.clone();
        repair.messages.push(Message::assistant(reply.text.clone()));
        repair.messages.push(Message::user(repair_message(&error)));
        sent_chars += prompt_chars(&repair);
        let second = call_json(env, llm, kind, &repair, &mut response_chars)?;
        tracing::debug!(?kind, depth, response = %second.text, "ai repair call");
        parse(&second.text).map_err(|e| Fail::Reason(short_reason(&format!("invalid JSON: {e}"))))
    })();
    let millis = millis_since(started);
    let outcome = match &result {
        Ok(_) => "ok".to_string(),
        Err(Fail::Cancelled) => "cancelled".to_string(),
        Err(Fail::Reason(r)) => format!("failed: {r}"),
    };
    tracing::info!(
        ?kind, depth, model = %first.model, millis, %outcome,
        prompt_chars = sent_chars, response_chars, "ai job"
    );
    match result {
        Ok(v) => Outcome::Done(v, millis),
        Err(Fail::Cancelled) => Outcome::Cancelled,
        Err(Fail::Reason(r)) => Outcome::Failed(r),
    }
}

/// How a text reply is cleaned.
#[derive(Clone, Copy)]
struct TextShape {
    max_chars: usize,
    keep_newlines: bool,
}

/// The text chain: an error before any text is retried once; an error mid-stream keeps the text
/// when it is long enough.
fn text_job(
    env: &Env,
    llm: &dyn Llm,
    kind: JobKind,
    depth: u32,
    req: &LlmRequest,
    delta_event: &dyn Fn(String) -> DirectorEvent,
    shape: TextShape,
) -> Outcome<String> {
    let started = Instant::now();
    let mut response_chars = 0usize;
    let mut sent_chars = 0usize;
    let mut last_reason = String::from("no text");
    let result: Result<String, Fail> = 'attempts: {
        for attempt in 0..2 {
            let mut got = String::new();
            sent_chars += prompt_chars(req);
            let res = call_once(env, llm, kind, req, &mut |d| {
                got.push_str(d);
                env.send(delta_event(d.to_string()));
            });
            match res {
                Ok(reply) => {
                    response_chars += reply.text.chars().count();
                    tracing::debug!(?kind, depth, prompt = ?req.messages, response = %reply.text, "ai call");
                    let cleaned = clean_ai_text(&reply.text, shape.max_chars, shape.keep_newlines);
                    if !cleaned.is_empty() {
                        break 'attempts Ok(cleaned);
                    }
                    last_reason = "empty reply".to_string();
                }
                Err(LlmError::Cancelled) => break 'attempts Err(Fail::Cancelled),
                Err(e) => {
                    response_chars += got.chars().count();
                    last_reason = short_reason(&e.to_string());
                    if !got.is_empty() {
                        let cleaned = clean_ai_text(&got, shape.max_chars, shape.keep_newlines);
                        if cleaned.chars().count() >= MIN_PARTIAL_CHARS {
                            break 'attempts Ok(cleaned);
                        }
                        break 'attempts Err(Fail::Reason(last_reason));
                    }
                }
            }
            if attempt == 0 && !env.pause() {
                break 'attempts Err(Fail::Cancelled);
            }
        }
        Err(Fail::Reason(last_reason))
    };
    let millis = millis_since(started);
    let outcome = match &result {
        Ok(_) => "ok".to_string(),
        Err(Fail::Cancelled) => "cancelled".to_string(),
        Err(Fail::Reason(r)) => format!("failed: {r}"),
    };
    tracing::info!(
        ?kind, depth, model = %req.model, millis, %outcome,
        prompt_chars = sent_chars, response_chars, "ai job"
    );
    match result {
        Ok(v) => Outcome::Done(v, millis),
        Err(Fail::Cancelled) => Outcome::Cancelled,
        Err(Fail::Reason(r)) => Outcome::Failed(r),
    }
}

// ---------------------------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------------------------

/// Runs `generate`; a panic becomes the offline fallback so the caller still gets a result.
fn guarded<T>(
    generate: impl FnOnce() -> Option<(T, GenSource)>,
    fallback: impl FnOnce() -> T,
) -> Option<(T, GenSource)> {
    match catch_unwind(AssertUnwindSafe(generate)) {
        Ok(result) => result,
        Err(_) => {
            tracing::error!("generation job panicked; using the offline result");
            Some((fallback(), offline_failed("generation panicked")))
        }
    }
}

fn place_job(env: &Env, run: &RunState, level_seed: &LevelSeed, digest: &str) {
    let ctx = GenContext {
        run,
        pack: &env.pack,
        seed: level_seed,
    };
    let depth = ctx.depth();
    let result = guarded(
        || {
            let anchor = ctx.anchor();
            let Some(llm) = env.llm_for(anchor) else {
                return Some((offline::place(&ctx), authored_or_offline(anchor.is_some())));
            };
            let make = |attempt| prompts::place_request(&ctx, &env.ai, digest, attempt);
            match json_job(env, llm.as_ref(), JobKind::Place, depth, &make, &|raw| {
                validate::parse_place(raw, &ctx)
            }) {
                Outcome::Done(place, millis) => Some((
                    place,
                    GenSource::Ai {
                        model: env.ai.model.clone(),
                        millis,
                    },
                )),
                Outcome::Failed(reason) => {
                    env.send(DirectorEvent::CallFailed);
                    Some((offline::place(&ctx), offline_failed(&reason)))
                }
                Outcome::Cancelled => None,
            }
        },
        || offline::place(&ctx),
    );
    if let Some((place, source)) = result
        && !env.cancelled()
    {
        env.send(DirectorEvent::PlaceReady {
            depth,
            place,
            source,
        });
    }
}

/// The validated place a passage or interlude is written for: the one in the run, else the
/// offline place for that level.
fn place_for(ctx: &GenContext) -> Place {
    ctx.run
        .levels
        .get(&ctx.depth())
        .and_then(|l| l.place.clone())
        .unwrap_or_else(|| offline::place(ctx))
}

fn passage_job(env: &Env, run: &RunState, level_seed: &LevelSeed, digest: &str) {
    let ctx = GenContext {
        run,
        pack: &env.pack,
        seed: level_seed,
    };
    let depth = ctx.depth();
    let place = place_for(&ctx);
    let result = guarded(
        || {
            let anchor = ctx.anchor();
            let Some(llm) = env.llm_for(anchor) else {
                return Some((
                    offline::passage(&ctx, &place),
                    authored_or_offline(anchor.is_some()),
                ));
            };
            let make = |attempt| prompts::passage_request(&ctx, &place, &env.ai, digest, attempt);
            match json_job(env, llm.as_ref(), JobKind::Passage, depth, &make, &|raw| {
                validate::parse_passage(raw, &ctx, &place)
            }) {
                Outcome::Done(passage, millis) => Some((
                    passage,
                    GenSource::Ai {
                        model: env.ai.model.clone(),
                        millis,
                    },
                )),
                Outcome::Failed(reason) => {
                    env.send(DirectorEvent::CallFailed);
                    Some((offline::passage(&ctx, &place), offline_failed(&reason)))
                }
                Outcome::Cancelled => None,
            }
        },
        || offline::passage(&ctx, &place),
    );
    if let Some((passage, source)) = result
        && !env.cancelled()
    {
        env.send(DirectorEvent::PassageReady {
            depth,
            passage,
            source,
        });
    }
}

fn interlude_job(env: &Env, run: &RunState, level_seed: &LevelSeed) {
    let ctx = GenContext {
        run,
        pack: &env.pack,
        seed: level_seed,
    };
    let depth = ctx.depth();
    let place = place_for(&ctx);
    let offline_text = || offline::interlude(&ctx, &place, run.exit_taken.as_ref());
    let result = guarded(
        || {
            let anchor = ctx.anchor();
            let Some(llm) = env.llm_for(anchor) else {
                let authored = anchor.is_some_and(|a| a.interlude.is_some());
                return Some((offline_text(), authored_or_offline(authored)));
            };
            let req = prompts::interlude_request(&ctx, &place, &env.ai);
            match text_job(
                env,
                llm.as_ref(),
                JobKind::Interlude,
                depth,
                &req,
                &|text| DirectorEvent::InterludeDelta { depth, text },
                TextShape {
                    max_chars: INTERLUDE_MAX_CHARS,
                    keep_newlines: false,
                },
            ) {
                Outcome::Done(text, millis) => Some((
                    text,
                    GenSource::Ai {
                        model: env.ai.model.clone(),
                        millis,
                    },
                )),
                Outcome::Failed(reason) => {
                    env.send(DirectorEvent::CallFailed);
                    Some((offline_text(), offline_failed(&reason)))
                }
                Outcome::Cancelled => None,
            }
        },
        offline_text,
    );
    if let Some((text, source)) = result
        && !env.cancelled()
    {
        env.send(DirectorEvent::InterludeDone {
            depth,
            text,
            source,
        });
    }
}

fn epilogue_job(env: &Env, run: &RunState, ending: &EndingDef, digest: &str) {
    let anchor = env.pack.anchor_at(run.depth).map(|e| &e.anchor);
    let offline_text = || offline::epilogue(ending, run);
    let result = guarded(
        || {
            let Some(llm) = env.llm_for(anchor) else {
                return Some((offline_text(), GenSource::Authored));
            };
            let req = prompts::epilogue_request(run, &env.pack, ending, &env.ai, digest);
            match text_job(
                env,
                llm.as_ref(),
                JobKind::Epilogue,
                run.depth,
                &req,
                &|text| DirectorEvent::EpilogueDelta { text },
                TextShape {
                    max_chars: EPILOGUE_MAX_CHARS,
                    keep_newlines: true,
                },
            ) {
                Outcome::Done(text, millis) => Some((
                    text,
                    GenSource::Ai {
                        model: env.ai.model.clone(),
                        millis,
                    },
                )),
                Outcome::Failed(reason) => {
                    env.send(DirectorEvent::CallFailed);
                    Some((offline_text(), offline_failed(&reason)))
                }
                Outcome::Cancelled => None,
            }
        },
        offline_text,
    );
    if let Some((text, source)) = result
        && !env.cancelled()
    {
        env.send(DirectorEvent::EpilogueDone { text, source });
    }
}

fn chat_job(env: &Env, run: &RunState, partner: &ChatPartner, history: &[ChatLine], session: u64) {
    let anchor = env.pack.anchor_at(run.depth).map(|e| &e.anchor);
    let offline_text = || {
        let player_lines: Vec<&ChatLine> = history.iter().filter(|l| l.from_player).collect();
        let last = player_lines.last().map_or("", |l| l.text.as_str());
        offline_chat_reply(
            &env.pack,
            partner,
            last,
            player_lines.len().saturating_sub(1),
        )
    };
    let result = guarded(
        || {
            let Some(llm) = env.llm_for(anchor) else {
                return Some((offline_text(), authored_or_offline(false)));
            };
            let req = prompts::chat_request(run, &env.pack, partner, history, &env.ai, session);
            match text_job(
                env,
                llm.as_ref(),
                JobKind::Chat,
                run.depth,
                &req,
                &|text| DirectorEvent::ChatDelta { session, text },
                TextShape {
                    max_chars: CHAT_MAX_CHARS,
                    keep_newlines: true,
                },
            ) {
                Outcome::Done(text, millis) => Some((
                    text,
                    GenSource::Ai {
                        model: env.ai.chat_model().to_string(),
                        millis,
                    },
                )),
                Outcome::Failed(reason) => {
                    env.send(DirectorEvent::CallFailed);
                    Some((offline_text(), offline_failed(&reason)))
                }
                Outcome::Cancelled => None,
            }
        },
        offline_text,
    );
    if let Some((text, source)) = result
        && !env.cancelled()
    {
        env.send(DirectorEvent::ChatDone {
            session,
            text,
            source,
        });
    }
}

/// An ELIZA-style chat reply from the pack: the partner's keyword groups (or the pool's for an
/// NPC sysop) matched against `text`, cycling replies with `turn`.
pub fn offline_chat_reply(
    pack: &StoryPack,
    partner: &ChatPartner,
    text: &str,
    turn: usize,
) -> String {
    let character = partner.cast_id.as_deref().and_then(|id| pack.character(id));
    offline::eliza_reply(pack, character, text, turn)
}

// ---------------------------------------------------------------------------------------------
// Director
// ---------------------------------------------------------------------------------------------

impl Director {
    pub fn new(
        llm: Option<Arc<dyn Llm>>,
        pack: Arc<StoryPack>,
        ai: AiConfig,
        tx: Sender<DirectorEvent>,
    ) -> Self {
        Self {
            llm,
            pack,
            ai,
            tx,
            pending: BTreeSet::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            retry_delay: RETRY_DELAY,
            next_session: 0,
        }
    }

    /// Like [`Director::new`] with a different pause before the one retry (the default is 2 s).
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    fn env(&self) -> Env {
        Env {
            llm: self.llm.clone(),
            pack: Arc::clone(&self.pack),
            ai: self.ai.clone(),
            tx: self.tx.clone(),
            cancel: Arc::clone(&self.cancel),
            retry_delay: self.retry_delay,
        }
    }

    /// Runs `job` on its own thread; without an `Llm` the job is offline-only, cheap and
    /// deterministic, so it runs inline and its events are queued before this call returns.
    fn dispatch(&self, name: &'static str, job: impl FnOnce(Env) + Send + 'static) {
        let env = self.env();
        if env.llm.is_none() {
            job(env);
        } else if let Err(e) = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || job(env))
        {
            tracing::error!("cannot spawn the {name} thread: {e}");
        }
    }

    fn level_seed_for(&self, run: &RunState, depth: u32) -> LevelSeed {
        run.levels.get(&depth).map_or_else(
            || seed::level_seed(run, &self.pack, depth),
            |l| l.seed.clone(),
        )
    }

    /// Preload the model with a one-token request; the result is ignored.
    pub fn warmup(&self) {
        let Some(llm) = self.llm.clone() else {
            return;
        };
        let req = LlmRequest {
            model: self.ai.model.clone(),
            messages: vec![Message::user("Hello.")],
            temperature: 0.0,
            max_tokens: 1,
            seed: None,
            json_schema: None,
        };
        let cancel = Arc::clone(&self.cancel);
        let spawned = std::thread::Builder::new()
            .name("director-warmup".to_string())
            .spawn(move || {
                let _ = llm.stream(&req, &mut |_| {}, &cancel);
            });
        if let Err(e) = spawned {
            tracing::warn!("cannot spawn the warmup thread: {e}");
        }
    }

    /// Request the place for `depth` (idempotent while pending). The level's seed comes from
    /// `run.levels[depth]` when present.
    pub fn request_place(&mut self, run: &RunState, depth: u32) {
        if !self.pending.insert((JobKind::Place, depth)) {
            return;
        }
        let run = run.clone();
        let level_seed = self.level_seed_for(&run, depth);
        let digest = run.chronicle.digest(&run, &self.pack);
        self.dispatch("director-place", move |env| {
            place_job(&env, &run, &level_seed, &digest);
        });
    }

    /// Request the passage for `depth`, written for the validated place in `run.levels[depth]`.
    pub fn request_passage(&mut self, run: &RunState, depth: u32) {
        if !self.pending.insert((JobKind::Passage, depth)) {
            return;
        }
        let run = run.clone();
        let level_seed = self.level_seed_for(&run, depth);
        let digest = run.chronicle.digest(&run, &self.pack);
        self.dispatch("director-passage", move |env| {
            passage_job(&env, &run, &level_seed, &digest);
        });
    }

    /// Request the interlude into `depth` for the exit in `run.exit_taken`.
    pub fn request_interlude(&mut self, run: &RunState, depth: u32) {
        if !self.pending.insert((JobKind::Interlude, depth)) {
            return;
        }
        let run = run.clone();
        let level_seed = self.level_seed_for(&run, depth);
        self.dispatch("director-interlude", move |env| {
            interlude_job(&env, &run, &level_seed);
        });
    }

    /// Returns a session id echoed by `ChatDelta`/`ChatDone`. `history` ends with the player's
    /// latest line.
    pub fn request_chat(
        &mut self,
        run: &RunState,
        partner: &ChatPartner,
        history: &[ChatLine],
    ) -> u64 {
        self.next_session += 1;
        let session = self.next_session;
        self.pending.insert((JobKind::Chat, 0));
        let run = run.clone();
        let partner = partner.clone();
        let history = history.to_vec();
        self.dispatch("director-chat", move |env| {
            chat_job(&env, &run, &partner, &history, session);
        });
        session
    }

    /// Request the epilogue for `ending` (idempotent while pending; keyed by `run.depth`).
    pub fn request_epilogue(&mut self, run: &RunState, ending: &EndingDef) {
        if !self.pending.insert((JobKind::Epilogue, run.depth)) {
            return;
        }
        let run = run.clone();
        let ending = ending.clone();
        let digest = run.chronicle.digest(&run, &self.pack);
        self.dispatch("director-epilogue", move |env| {
            epilogue_job(&env, &run, &ending, &digest);
        });
    }

    pub fn is_pending(&self, kind: JobKind, depth: u32) -> bool {
        self.pending.contains(&(kind, depth))
    }

    /// Clear pending markers for a finished job.
    pub fn on_event(&mut self, ev: &DirectorEvent) {
        match ev {
            DirectorEvent::PlaceReady { depth, .. } => {
                self.pending.remove(&(JobKind::Place, *depth));
            }
            DirectorEvent::PassageReady { depth, .. } => {
                self.pending.remove(&(JobKind::Passage, *depth));
            }
            DirectorEvent::InterludeDone { depth, .. } => {
                self.pending.remove(&(JobKind::Interlude, *depth));
            }
            DirectorEvent::ChatDone { .. } => self.pending.retain(|(k, _)| *k != JobKind::Chat),
            DirectorEvent::EpilogueDone { .. } => {
                self.pending.retain(|(k, _)| *k != JobKind::Epilogue);
            }
            _ => {}
        }
    }

    /// Stop every running job (they send nothing further) and forget what was pending.
    pub fn cancel_all(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.pending.clear();
    }
}

impl Drop for Director {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
