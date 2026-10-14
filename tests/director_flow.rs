//! Director behaviour with a scripted fake `Llm`: the retry/repair/offline chain, streaming,
//! pending markers, cancellation and the synchronous offline path.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use abraxas::config::AiConfig;
use abraxas::director::schema::{
    ChoiceGen, ExitGen, FileGen, HostGen, OptionGen, PassageGen, PlaceGen, PostGen,
};
use abraxas::director::validate::ValidationError;
use abraxas::director::{
    ChatLine, ChatPartner, Director, DirectorEvent, JobKind, offline_chat_reply,
};
use abraxas::game::seed::ensure_level;
use abraxas::game::state::{GenSource, Passage, Place, RunState};
use abraxas::llm::{Llm, LlmError, LlmReply, LlmRequest, Role};
use abraxas::pack::{self, model::*};

const WAIT: Duration = Duration::from_secs(10);

enum Step {
    /// Streams the text in two deltas, then succeeds.
    Reply(String),
    /// Fails before any byte.
    Transport,
    /// Streams the text, then the connection breaks.
    Partial(String),
    /// Streams forever until cancelled.
    Slow,
    Panic,
}

#[derive(Default)]
struct Fake {
    steps: Mutex<VecDeque<Step>>,
    calls: Mutex<Vec<LlmRequest>>,
    finished: AtomicUsize,
}

impl Fake {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            ..Self::default()
        })
    }

    fn calls(&self) -> Vec<LlmRequest> {
        self.calls.lock().unwrap().clone()
    }
}

impl Llm for Fake {
    fn stream(
        &self,
        req: &LlmRequest,
        on_delta: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<LlmReply, LlmError> {
        self.calls.lock().unwrap().push(req.clone());
        let step = self.steps.lock().unwrap().pop_front();
        let result = match step {
            None => Err(LlmError::Transport("script exhausted".into())),
            Some(Step::Transport) => Err(LlmError::Transport("refused".into())),
            Some(Step::Reply(text)) => {
                let mid = text
                    .char_indices()
                    .nth(text.chars().count() / 2)
                    .map_or(0, |(i, _)| i);
                on_delta(&text[..mid]);
                on_delta(&text[mid..]);
                Ok(LlmReply { text, usage: None })
            }
            Some(Step::Partial(text)) => {
                on_delta(&text);
                Err(LlmError::Stream("reset".into()))
            }
            Some(Step::Slow) => {
                let started = Instant::now();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break Err(LlmError::Cancelled);
                    }
                    if started.elapsed() > Duration::from_secs(20) {
                        break Err(LlmError::Timeout);
                    }
                    on_delta("x");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Some(Step::Panic) => panic!("fake llm panic"),
        };
        self.finished.fetch_add(1, Ordering::SeqCst);
        result
    }
}

struct Rig {
    director: Director,
    rx: Receiver<DirectorEvent>,
    run: RunState,
    pack: Arc<StoryPack>,
}

fn rig(llm: Option<Arc<Fake>>, depth: u32) -> Rig {
    rig_with(llm, depth, AiConfig::default())
}

fn rig_with(llm: Option<Arc<Fake>>, depth: u32, ai: AiConfig) -> Rig {
    let pack = Arc::new(pack::embedded().unwrap());
    let (tx, rx) = mpsc::channel();
    let llm: Option<Arc<dyn Llm>> = llm.map(|l| l as Arc<dyn Llm>);
    let director =
        Director::new(llm, Arc::clone(&pack), ai, tx).with_retry_delay(Duration::from_millis(1));
    let mut run = RunState::new("Tester", "Jo Doe", "a dream", 11, "rollover", None);
    for d in 0..=depth {
        ensure_level(&mut run, &pack, d);
    }
    run.depth = depth;
    Rig {
        director,
        rx,
        run,
        pack,
    }
}

impl Rig {
    /// Events up to and including the first one `done` accepts.
    fn until(&mut self, done: impl Fn(&DirectorEvent) -> bool) -> Vec<DirectorEvent> {
        let mut out = Vec::new();
        loop {
            let ev = self
                .rx
                .recv_timeout(WAIT)
                .expect("director event within the timeout");
            self.director.on_event(&ev);
            let stop = done(&ev);
            out.push(ev);
            if stop {
                return out;
            }
        }
    }

    fn place(&mut self, depth: u32) -> (Place, GenSource, Vec<DirectorEvent>) {
        let events = self.until(|e| matches!(e, DirectorEvent::PlaceReady { .. }));
        match events.last() {
            Some(DirectorEvent::PlaceReady {
                depth: d,
                place,
                source,
            }) => {
                assert_eq!(*d, depth);
                (place.clone(), source.clone(), events)
            }
            _ => unreachable!(),
        }
    }

    fn passage(&mut self) -> (Passage, GenSource, Vec<DirectorEvent>) {
        let events = self.until(|e| matches!(e, DirectorEvent::PassageReady { .. }));
        match events.last() {
            Some(DirectorEvent::PassageReady {
                passage, source, ..
            }) => (passage.clone(), source.clone(), events),
            _ => unreachable!(),
        }
    }

    fn adopt_place(&mut self, depth: u32, place: Place) {
        self.run.levels.get_mut(&depth).unwrap().place = Some(place);
    }
}

fn place_json() -> String {
    let gen_place = PlaceGen {
        title: "THE FLOODED ATRIUM".into(),
        bbs_name: "TIDE LINE".into(),
        tagline: "the water is a kind of memory".into(),
        level_card: "Field note: the atrium is knee deep. Phones ring underwater.".into(),
        sysop: "philemon".into(),
        host: HostGen {
            name: "ODS TIDE EXCHANGE".into(),
            os: HostOs::Unix,
            banner: "AUTHORIZED USE ONLY".into(),
            objective: "Pull TIDE.LOG from the archive.".into(),
            target_file: "TIDE.LOG".into(),
            node_names: vec!["reeds".into()],
            ice_names: vec![],
        },
        posts: vec![PostGen {
            from: "gull".into(),
            subject: "low tide".into(),
            body: "the carpet floats".into(),
        }],
        text_files: vec![FileGen {
            name: "README.TXT".into(),
            description: "read me".into(),
            body: "hello".into(),
        }],
        host_files: vec![],
        words: vec!["TIDAL".into(), "REEDS".into(), "BRINE".into()],
    };
    serde_json::to_string(&gen_place).unwrap()
}

fn passage_json() -> String {
    let option = |text: &str, stance| OptionGen {
        text: text.into(),
        stance,
        flags: vec!["waded_in".into()],
    };
    let exit = |kind, text: &str| ExitGen {
        kind,
        text: text.into(),
    };
    let gen_passage = PassageGen {
        beat: "The log is wet. It says: keep going.".into(),
        choice: ChoiceGen {
            prompt: "The water rises.".into(),
            options: vec![
                option("Wade in.", Stance::Integrate),
                option("Climb out.", Stance::Repress),
                option("Wait and watch.", Stance::Neutral),
            ],
        },
        exits: vec![
            exit(ExitKind::Calm, "A dry stair."),
            exit(ExitKind::Deep, "The drain."),
            exit(ExitKind::Mirror, "The still surface."),
        ],
        vision: VisionKind::Plasma,
        captions: vec!["ONE".into(), "TWO".into(), "THREE".into()],
        summary: "Tester waded into the atrium.".into(),
    };
    serde_json::to_string(&gen_passage).unwrap()
}

fn assert_place_invariants(place: &Place, run: &RunState, depth: u32) {
    assert_eq!(place.posts.len(), 5, "4 posts plus the clue post");
    assert!(place.posts.iter().any(|p| p.subject == "re: front door"));
    if depth >= 1 {
        assert!(place.posts.iter().any(|p| p.from == run.shadow()));
    }
    assert_eq!(place.text_files.len(), 2);
    assert_eq!(place.host_files.len(), 3);
    assert_eq!(place.host.node_names.len(), 8);
    assert_eq!(place.host.ice_names.len(), 3);
    assert!(place.words.contains(&place.password));
    let len = run.levels[&depth].seed.wordlock_len;
    assert!(
        place
            .words
            .iter()
            .all(|w| w.len() == len && w.chars().all(|c| c.is_ascii_uppercase()))
    );
    assert!(!place.title.is_empty() && !place.bbs_name.is_empty());
}

fn assert_passage_invariants(passage: &Passage) {
    let stances: Vec<Stance> = passage.choice.options.iter().map(|o| o.stance).collect();
    assert_eq!(stances, Stance::ALL.to_vec());
    let kinds: Vec<ExitKind> = passage.exits.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, ExitKind::ALL.to_vec());
    assert_eq!(passage.captions.len(), 3);
    assert!(!passage.beat.is_empty() && !passage.summary.is_empty());
}

fn offline_reason(source: &GenSource) -> &str {
    match source {
        GenSource::Offline { reason } => reason,
        other => panic!("expected an offline source, got {other:?}"),
    }
}

// --- JSON jobs ---------------------------------------------------------------------------------

#[test]
fn garbage_twice_falls_back_to_an_offline_place() {
    let fake = Fake::new(vec![
        Step::Reply("nothing like json".into()),
        Step::Reply("still {not json".into()),
    ]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    let (place, source, events) = r.place(5);
    assert!(
        !offline_reason(&source).is_empty(),
        "the reason says why the AI was dropped"
    );
    assert_eq!(fake.calls().len(), 2, "one call plus one repair call");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DirectorEvent::CallFailed))
    );
    assert_place_invariants(&place, &r.run, 5);
    assert!(!r.director.is_pending(JobKind::Place, 5));
}

#[test]
fn garbage_then_valid_json_on_the_repair_call_is_ai() {
    let fake = Fake::new(vec![
        Step::Reply("garbage".into()),
        Step::Reply(format!("```json\n{}\n```", place_json())),
    ]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    let (place, source, events) = r.place(5);
    assert!(matches!(&source, GenSource::Ai { model, .. } if model == "qwen3.8"));
    assert_eq!(place.title, "THE FLOODED ATRIUM");
    assert_place_invariants(&place, &r.run, 5);

    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    let repair = &calls[1].messages;
    let first = &calls[0].messages;
    assert_eq!(repair.len(), first.len() + 2);
    assert_eq!(&repair[..first.len()], first.as_slice());
    assert_eq!(repair[first.len()].role, Role::Assistant);
    assert_eq!(repair[first.len()].content, "garbage");
    assert_eq!(repair[first.len() + 1].role, Role::User);
    assert_eq!(
        repair[first.len() + 1].content,
        format!(
            "Your JSON was invalid: {}. Reply again with corrected JSON only.",
            ValidationError::NoJson
        )
    );
    assert_ne!(calls[0].seed, calls[1].seed);
    assert_eq!(calls[0].max_tokens, 2600);
    assert_eq!(
        calls[0].json_schema.as_ref().map(|s| s.0.as_str()),
        Some("place")
    );
    // Modem lights: a request per call, chunks for the streamed text, no failure.
    let requests = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                DirectorEvent::Request {
                    kind: JobKind::Place
                }
            )
        })
        .count();
    assert_eq!(requests, 2);
    assert!(events.iter().any(|e| matches!(e, DirectorEvent::Chunk)));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::CallFailed))
    );
}

#[test]
fn a_transport_error_is_retried_once_with_the_same_request() {
    let fake = Fake::new(vec![Step::Transport, Step::Reply(place_json())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    let (_, source, events) = r.place(5);
    assert!(matches!(source, GenSource::Ai { .. }));
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].messages, calls[1].messages);
    assert_eq!(calls[0].seed, calls[1].seed);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, DirectorEvent::CallFailed))
            .count(),
        1,
        "only the failed first call lights CD red"
    );
}

#[test]
fn two_transport_errors_fall_back_with_the_connection_error() {
    let fake = Fake::new(vec![Step::Transport, Step::Transport]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    let (place, source, _) = r.place(5);
    assert!(offline_reason(&source).contains("connection failed"));
    assert_eq!(fake.calls().len(), 2);
    assert_place_invariants(&place, &r.run, 5);
}

#[test]
fn a_place_stream_that_breaks_midway_falls_back_without_retrying() {
    let fake = Fake::new(vec![Step::Partial("{\"title\": \"HALF".into())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    let (_, source, _) = r.place(5);
    assert!(offline_reason(&source).contains("stream error"));
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn a_panicking_job_still_delivers_an_offline_place() {
    let fake = Fake::new(vec![Step::Panic]);
    let mut r = rig(Some(fake), 5);
    r.director.request_place(&r.run, 5);
    let (place, source, _) = r.place(5);
    assert!(offline_reason(&source).contains("panicked"));
    assert_place_invariants(&place, &r.run, 5);
    assert!(!r.director.is_pending(JobKind::Place, 5));
}

#[test]
fn passage_is_validated_against_the_place_in_the_run() {
    let fake = Fake::new(vec![Step::Reply(passage_json())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    // Place from the offline director so the passage prompt names a real title.
    let mut offline = rig(None, 5);
    offline.director.request_place(&offline.run, 5);
    let (place, _, _) = offline.place(5);
    r.adopt_place(5, place.clone());
    r.director.request_passage(&r.run, 5);
    let (passage, source, _) = r.passage();
    assert!(matches!(source, GenSource::Ai { .. }));
    assert_eq!(passage.beat, "The log is wet. It says: keep going.");
    assert_eq!(passage.vision, VisionKind::Plasma);
    assert_passage_invariants(&passage);
    let calls = fake.calls();
    assert_eq!(calls[0].max_tokens, 1000);
    assert!(calls[0].messages[1].content.contains(&place.title));
    assert!(
        calls[0].messages[1]
            .content
            .contains(&place.host.target_file)
    );
}

#[test]
fn an_anchor_with_ai_off_never_touches_the_llm() {
    let fake = Fake::new(vec![]);
    let mut r = rig(Some(Arc::clone(&fake)), 0);
    r.director.request_place(&r.run, 0);
    let (place, source, _) = r.place(0);
    assert_eq!(source, GenSource::Authored);
    assert_eq!(place.title, "THE LOBBY");
    r.adopt_place(0, place);
    r.director.request_passage(&r.run, 0);
    let (passage, source, _) = r.passage();
    assert_eq!(source, GenSource::Authored);
    assert_passage_invariants(&passage);
    r.director.request_interlude(&r.run, 0);
    let events = r.until(|e| matches!(e, DirectorEvent::InterludeDone { .. }));
    assert!(matches!(
        events.last(),
        Some(DirectorEvent::InterludeDone {
            source: GenSource::Authored,
            ..
        })
    ));
    assert!(fake.calls().is_empty());
}

#[test]
fn an_adapting_anchor_reaches_the_ai_and_keeps_locked_fields() {
    let fake = Fake::new(vec![Step::Reply(place_json())]);
    let mut r = rig(Some(Arc::clone(&fake)), 3);
    r.director.request_place(&r.run, 3);
    let (place, source, _) = r.place(3);
    assert!(matches!(source, GenSource::Ai { .. }));
    assert_eq!(place.title, "THE BLACK SUN", "the AI's title is ignored");
    assert_eq!(place.bbs_name, "SOL NIGER");
    assert_eq!(place.sysop.cast_id.as_deref(), Some("shadow"));
    assert_place_invariants(&place, &r.run, 3);
    let prompt = &fake.calls()[0].messages[1].content;
    assert!(prompt.contains("Authored level. Keep exactly"));
}

// --- Text jobs ---------------------------------------------------------------------------------

const INTERLUDE_TEXT: &str =
    "You fall through the drain. The water is warm and remembers your name.";

fn interlude_events(r: &mut Rig) -> (Vec<String>, String, GenSource) {
    let events = r.until(|e| matches!(e, DirectorEvent::InterludeDone { .. }));
    let deltas = events
        .iter()
        .filter_map(|e| match e {
            DirectorEvent::InterludeDelta { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    match events.last() {
        Some(DirectorEvent::InterludeDone { text, source, .. }) => {
            (deltas, text.clone(), source.clone())
        }
        _ => unreachable!(),
    }
}

#[test]
fn interlude_streams_deltas_then_done() {
    let fake = Fake::new(vec![Step::Reply(INTERLUDE_TEXT.into())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_interlude(&r.run, 5);
    assert!(r.director.is_pending(JobKind::Interlude, 5));
    let (deltas, text, source) = interlude_events(&mut r);
    assert!(deltas.len() >= 2);
    assert_eq!(deltas.concat(), INTERLUDE_TEXT);
    assert_eq!(text, INTERLUDE_TEXT);
    assert!(matches!(source, GenSource::Ai { .. }));
    let call = &fake.calls()[0];
    assert_eq!(call.max_tokens, 200);
    assert!(call.json_schema.is_none());
    assert!(
        (call.temperature - 1.0).abs() < 1e-6,
        "0.9 + 0.1 for text calls"
    );
}

#[test]
fn a_text_error_before_any_text_is_retried_once() {
    let fake = Fake::new(vec![Step::Transport, Step::Reply(INTERLUDE_TEXT.into())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_interlude(&r.run, 5);
    let (_, text, source) = interlude_events(&mut r);
    assert_eq!(text, INTERLUDE_TEXT);
    assert!(matches!(source, GenSource::Ai { .. }));
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn text_errors_before_any_text_twice_fall_back_offline() {
    let fake = Fake::new(vec![Step::Transport, Step::Transport]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_interlude(&r.run, 5);
    let (_, text, source) = interlude_events(&mut r);
    assert!(!text.is_empty());
    assert!(offline_reason(&source).contains("connection failed"));
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn a_mid_stream_failure_keeps_text_of_twenty_characters_or_more() {
    let long = "You fall and the water remembers.";
    let fake = Fake::new(vec![Step::Partial(long.into())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_interlude(&r.run, 5);
    let (_, text, source) = interlude_events(&mut r);
    assert_eq!(text, long);
    assert!(matches!(source, GenSource::Ai { .. }));
    assert_eq!(fake.calls().len(), 1, "text was received, so no retry");
}

#[test]
fn a_mid_stream_failure_with_little_text_falls_back() {
    let fake = Fake::new(vec![Step::Partial("You fall".into())]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_interlude(&r.run, 5);
    let (_, text, source) = interlude_events(&mut r);
    assert_ne!(text, "You fall");
    assert!(offline_reason(&source).contains("stream error"));
}

#[test]
fn ai_text_is_cleaned_of_pipe_codes_and_markup_characters() {
    let fake = Fake::new(vec![Step::Reply(
        "|12You fall \u{201c}through\u{201d} the drain\u{2026} and the water is warm.".into(),
    )]);
    let mut r = rig(Some(fake), 5);
    r.director.request_interlude(&r.run, 5);
    let (_, text, _) = interlude_events(&mut r);
    assert_eq!(
        text,
        "You fall \"through\" the drain... and the water is warm."
    );
}

fn partner() -> ChatPartner {
    ChatPartner {
        handle: "PHILEMON".into(),
        cast_id: Some("philemon".into()),
    }
}

fn player_line(text: &str) -> ChatLine {
    ChatLine {
        from_player: true,
        text: text.into(),
    }
}

#[test]
fn chat_streams_with_the_chat_model_and_the_last_twelve_turns() {
    let fake = Fake::new(vec![Step::Reply("i hear you. stay on the line.".into())]);
    let ai = AiConfig {
        chat_model: "chatty".into(),
        ..AiConfig::default()
    };
    let mut r = rig_with(Some(Arc::clone(&fake)), 5, ai);
    let history: Vec<ChatLine> = (0..20)
        .map(|i| ChatLine {
            from_player: i % 2 == 0,
            text: format!("turn {i}"),
        })
        .collect();
    let session = r.director.request_chat(&r.run, &partner(), &history);
    let events = r.until(|e| matches!(e, DirectorEvent::ChatDone { .. }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DirectorEvent::ChatDelta { session: s, .. } if *s == session))
    );
    match events.last() {
        Some(DirectorEvent::ChatDone {
            session: s,
            text,
            source,
        }) => {
            assert_eq!(*s, session);
            assert_eq!(text, "i hear you. stay on the line.");
            assert!(matches!(source, GenSource::Ai { model, .. } if model == "chatty"));
        }
        _ => unreachable!(),
    }
    let call = &fake.calls()[0];
    assert_eq!(call.model, "chatty");
    assert_eq!(call.max_tokens, 180);
    assert_eq!(call.messages.len(), 13, "system plus the last 12 turns");
    assert_eq!(call.messages[1].content, "turn 8");
    assert!(!r.director.is_pending(JobKind::Chat, 0));
}

#[test]
fn chat_failure_answers_with_eliza_marked_offline() {
    let fake = Fake::new(vec![Step::Transport, Step::Transport]);
    let mut r = rig(Some(fake), 5);
    let history = vec![player_line("where is my sister wren?")];
    let session = r.director.request_chat(&r.run, &partner(), &history);
    let events = r.until(|e| matches!(e, DirectorEvent::ChatDone { .. }));
    match events.last() {
        Some(DirectorEvent::ChatDone {
            session: s,
            text,
            source,
        }) => {
            assert_eq!(*s, session);
            assert_eq!(
                text,
                &offline_chat_reply(&r.pack, &partner(), "where is my sister wren?", 0)
            );
            assert!(offline_reason(source).contains("connection failed"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn epilogue_falls_back_to_the_authored_ending_text() {
    let fake = Fake::new(vec![Step::Transport, Step::Transport]);
    let mut r = rig(Some(fake), 15);
    let ending = r
        .pack
        .anchor_at(15)
        .unwrap()
        .anchor
        .ending
        .iter()
        .find(|e| e.id == "abraxas")
        .unwrap()
        .clone();
    r.director.request_epilogue(&r.run, &ending);
    assert!(r.director.is_pending(JobKind::Epilogue, 15));
    let events = r.until(|e| matches!(e, DirectorEvent::EpilogueDone { .. }));
    match events.last() {
        Some(DirectorEvent::EpilogueDone { text, source }) => {
            assert!(text.contains("sysop"), "authored text: {text}");
            assert!(!offline_reason(source).is_empty());
        }
        _ => unreachable!(),
    }
    assert!(!r.director.is_pending(JobKind::Epilogue, 15));
}

// --- Scheduling --------------------------------------------------------------------------------

#[test]
fn pending_markers_prevent_duplicate_requests_and_clear_on_events() {
    let mut r = rig(None, 5);
    assert!(!r.director.is_pending(JobKind::Place, 5));
    r.director.request_place(&r.run, 5);
    assert!(r.director.is_pending(JobKind::Place, 5));
    r.director.request_place(&r.run, 5);
    let queued: Vec<DirectorEvent> = r.rx.try_iter().collect();
    let ready: Vec<&DirectorEvent> = queued
        .iter()
        .filter(|e| matches!(e, DirectorEvent::PlaceReady { .. }))
        .collect();
    assert_eq!(ready.len(), 1, "the second request was ignored");
    assert!(
        r.director.is_pending(JobKind::Place, 5),
        "cleared only by the event"
    );
    r.director.on_event(ready[0]);
    assert!(!r.director.is_pending(JobKind::Place, 5));
    r.director.request_place(&r.run, 5);
    assert_eq!(
        r.rx.try_iter()
            .filter(|e| matches!(e, DirectorEvent::PlaceReady { .. }))
            .count(),
        1,
        "after the event a fresh request is accepted"
    );
    // Other depths and kinds are independent.
    assert!(!r.director.is_pending(JobKind::Place, 6));
    assert!(!r.director.is_pending(JobKind::Passage, 5));
}

#[test]
fn cancel_all_stops_a_long_stream_quickly_and_drops_its_result() {
    let fake = Fake::new(vec![Step::Slow]);
    let mut r = rig(Some(Arc::clone(&fake)), 5);
    r.director.request_place(&r.run, 5);
    r.until(|e| matches!(e, DirectorEvent::Chunk));
    assert!(r.director.is_pending(JobKind::Place, 5));
    let started = Instant::now();
    r.director.cancel_all();
    assert!(!r.director.is_pending(JobKind::Place, 5));
    while fake.finished.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stream kept running"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    let late: Vec<DirectorEvent> = r.rx.try_iter().collect();
    assert!(
        !late
            .iter()
            .any(|e| matches!(e, DirectorEvent::PlaceReady { .. })),
        "a cancelled job delivers nothing"
    );
    // The director works again afterwards.
    fake.steps
        .lock()
        .unwrap()
        .push_back(Step::Reply(place_json()));
    r.director.request_place(&r.run, 5);
    let (_, source, _) = r.place(5);
    assert!(matches!(source, GenSource::Ai { .. }));
}

// --- No LLM: synchronous offline path ----------------------------------------------------------

#[test]
fn without_an_llm_every_part_arrives_before_the_request_returns() {
    let mut r = rig(None, 3);
    r.director.request_place(&r.run, 3);
    let ev = r.rx.try_recv().expect("place queued synchronously");
    let DirectorEvent::PlaceReady { place, source, .. } = ev else {
        panic!("expected PlaceReady, got {ev:?}");
    };
    assert_eq!(source, GenSource::Authored, "anchor level in offline mode");
    assert_eq!(place.title, "THE BLACK SUN");
    assert_place_invariants(&place, &r.run, 3);
    assert!(r.rx.try_recv().is_err(), "no modem-light traffic offline");
    r.adopt_place(3, place);

    r.director.request_passage(&r.run, 3);
    let DirectorEvent::PassageReady {
        passage, source, ..
    } = r.rx.try_recv().unwrap()
    else {
        panic!("expected PassageReady");
    };
    assert_eq!(source, GenSource::Authored);
    assert!(passage.beat.contains("Abraxas is not a place"));
    assert_passage_invariants(&passage);

    r.director.request_interlude(&r.run, 3);
    let DirectorEvent::InterludeDone { text, .. } = r.rx.try_recv().unwrap() else {
        panic!("expected InterludeDone");
    };
    assert!(text.contains("switchboards"));

    let session = r
        .director
        .request_chat(&r.run, &partner(), &[player_line("hello wren")]);
    match r.rx.try_recv().unwrap() {
        DirectorEvent::ChatDone {
            session: s,
            text,
            source,
        } => {
            assert_eq!(s, session);
            assert_eq!(
                text,
                offline_chat_reply(&r.pack, &partner(), "hello wren", 0)
            );
            assert_eq!(
                source,
                GenSource::Offline {
                    reason: String::new()
                }
            );
        }
        other => panic!("expected ChatDone, got {other:?}"),
    }
}

#[test]
fn offline_levels_without_an_anchor_are_marked_plain_offline() {
    let mut r = rig(None, 5);
    r.director.request_place(&r.run, 5);
    let DirectorEvent::PlaceReady { source, .. } = r.rx.try_recv().unwrap() else {
        panic!("expected PlaceReady");
    };
    assert_eq!(
        source,
        GenSource::Offline {
            reason: String::new()
        }
    );
}

#[test]
fn offline_epilogue_is_the_authored_ending_text() {
    let mut r = rig(None, 15);
    let ending = r.pack.anchor_at(15).unwrap().anchor.ending[0].clone();
    r.director.request_epilogue(&r.run, &ending);
    let DirectorEvent::EpilogueDone { text, source } = r.rx.try_recv().unwrap() else {
        panic!("expected EpilogueDone");
    };
    assert_eq!(source, GenSource::Authored);
    assert!(!text.is_empty());
}
