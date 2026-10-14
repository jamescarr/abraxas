//! The AI director: schedules generation jobs, falls back to the offline generator, and reports
//! results as `DirectorEvent`s.
//!
//! SCAFFOLD: the data types below are final; the `Director` methods are inert placeholders (they
//! never produce events) that the director step replaces.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::config::AiConfig;
use crate::game::state::{GenSource, Passage, Place, RunState};
use crate::llm::Llm;
use crate::pack::model::{EndingDef, StoryPack};

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

pub struct Director {
    pending: BTreeSet<(JobKind, u32)>,
}

impl Director {
    pub fn new(
        _llm: Option<Arc<dyn Llm>>,
        _pack: Arc<StoryPack>,
        _ai: AiConfig,
        _tx: Sender<DirectorEvent>,
    ) -> Self {
        Self {
            pending: BTreeSet::new(),
        }
    }

    pub fn warmup(&self) {}

    pub fn request_place(&mut self, _run: &RunState, _depth: u32) {}

    pub fn request_passage(&mut self, _run: &RunState, _depth: u32) {}

    pub fn request_interlude(&mut self, _run: &RunState, _depth: u32) {}

    /// Returns a session id echoed by `ChatDelta`/`ChatDone`.
    pub fn request_chat(
        &mut self,
        _run: &RunState,
        _partner: &ChatPartner,
        _history: &[ChatLine],
    ) -> u64 {
        0
    }

    pub fn request_epilogue(&mut self, _run: &RunState, _ending: &EndingDef) {}

    pub fn is_pending(&self, kind: JobKind, depth: u32) -> bool {
        self.pending.contains(&(kind, depth))
    }

    /// Clear pending markers for a finished job.
    pub fn on_event(&mut self, _ev: &DirectorEvent) {}

    pub fn cancel_all(&mut self) {}
}
