//! LLM client interface and the OpenAI-compatible streaming implementation (`OpenAiCompat`).

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::config::{AiConfig, ResponseFormatKind};
use crate::director::stream::{SseEvent, SseParser, ThinkFilter};

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

/// Streaming client for any OpenAI-compatible `/chat/completions` endpoint (Ollama by default).
pub struct OpenAiCompat {
    agent: ureq::Agent,
    base_url: String,
    api_key: Option<String>,
    reasoning_effort: Option<String>,
    response_format: ResponseFormatKind,
}

impl std::fmt::Debug for OpenAiCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompat")
            .field("base_url", &self.base_url)
            .field("has_api_key", &self.api_key.is_some())
            .field("reasoning_effort", &self.reasoning_effort)
            .field("response_format", &self.response_format)
            .finish()
    }
}

impl OpenAiCompat {
    /// Build from config. The key comes from the env var named by `ai.api_key_env`.
    pub fn new(ai: &AiConfig) -> Self {
        let api_key = if ai.api_key_env.is_empty() {
            None
        } else {
            match std::env::var(&ai.api_key_env) {
                Ok(k) if !k.is_empty() => Some(k),
                _ => {
                    tracing::warn!(
                        "env var {} is not set; sending no Authorization header",
                        ai.api_key_env
                    );
                    None
                }
            }
        };
        let effort = (!ai.reasoning_effort.is_empty()).then(|| ai.reasoning_effort.clone());
        Self::with_base_url(
            &ai.base_url,
            api_key,
            effort,
            ai.response_format,
            Duration::from_secs(ai.timeout_secs),
        )
    }

    /// Explicit constructor (tests, tools). `timeout` bounds receiving the response body.
    pub fn with_base_url(
        base_url: &str,
        api_key: Option<String>,
        reasoning_effort: Option<String>,
        response_format: ResponseFormatKind,
        timeout: Duration,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            // A request queued behind another on a single-slot server (Ollama's default) sends no
            // headers until its turn comes, so waiting for them gets the full generation budget.
            .timeout_recv_response(Some(timeout.max(Duration::from_secs(120))))
            .timeout_recv_body(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            reasoning_effort,
            response_format,
        }
    }

    /// Preload the model: a `max_tokens: 1` request whose result is ignored.
    pub fn warmup(&self, model: &str) {
        let req = LlmRequest {
            model: model.to_owned(),
            messages: vec![Message::user("hi")],
            temperature: 0.0,
            max_tokens: 1,
            seed: None,
            json_schema: None,
        };
        let cancel = AtomicBool::new(false);
        let _ = self.stream(&req, &mut |_| {}, &cancel);
    }

    fn request_body(&self, req: &LlmRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(seed) = req.seed {
            body["seed"] = seed.into();
        }
        if let Some(effort) = &self.reasoning_effort {
            body["reasoning_effort"] = effort.as_str().into();
        }
        if let Some((name, schema)) = &req.json_schema {
            body["response_format"] = match self.response_format {
                ResponseFormatKind::JsonSchema => serde_json::json!({
                    "type": "json_schema",
                    "json_schema": { "name": name, "schema": schema, "strict": false },
                }),
                ResponseFormatKind::JsonObject => serde_json::json!({ "type": "json_object" }),
            };
        }
        body
    }
}

fn map_send_error(e: ureq::Error) -> LlmError {
    match e {
        ureq::Error::Timeout(
            ureq::Timeout::Resolve | ureq::Timeout::Connect | ureq::Timeout::SendRequest,
        ) => LlmError::Transport(e.to_string()),
        ureq::Error::Timeout(_) => LlmError::Timeout,
        other => LlmError::Transport(other.to_string()),
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::TimedOut
        || e.get_ref()
            .and_then(|i| i.downcast_ref::<ureq::Error>())
            .is_some_and(|u| matches!(u, ureq::Error::Timeout(_)))
}

fn excerpt(body: &str) -> String {
    let t = body.trim();
    match t.char_indices().nth(300) {
        Some((i, _)) => t[..i].to_owned(),
        None => t.to_owned(),
    }
}

/// Feeds parser events through the think filter; returns `Ok(true)` once `[DONE]` is seen.
fn apply_events(
    events: Vec<SseEvent>,
    filter: &mut ThinkFilter,
    reply: &mut LlmReply,
    on_delta: &mut dyn FnMut(&str),
) -> Result<bool, LlmError> {
    let mut done = false;
    for ev in events {
        match ev {
            SseEvent::Delta(d) => {
                let vis = filter.push(&d);
                if !vis.is_empty() {
                    reply.text.push_str(&vis);
                    on_delta(&vis);
                }
            }
            SseEvent::Usage(u) => reply.usage = Some(u),
            SseEvent::Done => done = true,
            SseEvent::Error(msg) => return Err(LlmError::Stream(msg)),
        }
    }
    Ok(done)
}

impl Llm for OpenAiCompat {
    /// Streams a completion. Ending without `[DONE]` (e.g. a dropped connection) is a
    /// `LlmError::Stream`; text received up to then has already been passed to `on_delta`.
    fn stream(
        &self,
        req: &LlmRequest,
        on_delta: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<LlmReply, LlmError> {
        let body = self.request_body(req);
        let url = format!("{}/chat/completions", self.base_url);
        tracing::debug!(
            url = %url,
            model = %req.model,
            messages = req.messages.len(),
            prompt_chars = req.messages.iter().map(|m| m.content.chars().count()).sum::<usize>(),
            "llm request"
        );
        let mut builder = self.agent.post(&url);
        if let Some(key) = &self.api_key {
            builder = builder.header("Authorization", &format!("Bearer {key}"));
        }
        let mut resp = builder.send_json(&body).map_err(map_send_error)?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(LlmError::Status(status, excerpt(&text)));
        }

        let mut reader = BufReader::new(resp.into_body().into_reader());
        let mut parser = SseParser::new();
        let mut filter = ThinkFilter::new();
        let mut reply = LlmReply::default();
        let mut line = Vec::new();
        let mut done = false;
        while !done {
            if cancel.load(Ordering::Relaxed) {
                return Err(LlmError::Cancelled);
            }
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if is_timeout(&e) => return Err(LlmError::Timeout),
                Err(e) => return Err(LlmError::Stream(e.to_string())),
            }
            let events = parser.feed(&String::from_utf8_lossy(&line));
            done = apply_events(events, &mut filter, &mut reply, on_delta)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(LlmError::Cancelled);
            }
        }
        if !done {
            let events = parser.finish();
            done = apply_events(events, &mut filter, &mut reply, on_delta)?;
        }
        let tail = filter.finish();
        if !tail.is_empty() {
            reply.text.push_str(&tail);
            on_delta(&tail);
        }
        if !done {
            return Err(LlmError::Stream("stream ended before [DONE]".to_owned()));
        }
        Ok(reply)
    }
}
