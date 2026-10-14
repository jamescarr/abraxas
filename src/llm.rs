//! LLM client interface. The OpenAI-compatible implementation (`OpenAiCompat`) is added by the
//! director step.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub seed: Option<u64>,
    /// `(schema name, JSON schema)` for structured output.
    pub json_schema: Option<(String, serde_json::Value)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Default)]
pub struct LlmReply {
    pub text: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Could not connect or send, before any response byte arrived.
    #[error("connection failed: {0}")]
    Transport(String),
    /// Non-2xx response with a body excerpt.
    #[error("HTTP {0}: {1}")]
    Status(u16, String),
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    /// The stream broke or produced something unreadable after it started.
    #[error("stream error: {0}")]
    Stream(String),
}

/// A streaming chat-completion backend. `on_delta` receives visible text as it arrives (after
/// reasoning has been filtered out); the returned reply holds the full visible text.
pub trait Llm: Send + Sync {
    fn stream(
        &self,
        req: &LlmRequest,
        on_delta: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<LlmReply, LlmError>;
}
