//! Incremental parsing of OpenAI-style SSE chat streams and `<think>` filtering.

use serde_json::Value;

use crate::llm::Usage;

/// One decoded item from the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// Visible content from `choices[0].delta.content` (reasoning fields are ignored).
    Delta(String),
    /// Token usage from the final chunk.
    Usage(Usage),
    /// `data: [DONE]`.
    Done,
    /// An `error` object carried in the stream.
    Error(String),
}

/// Line-oriented SSE parser. Feed it text in arbitrary chunks; lines may be split across chunks.
///
/// Lines that are blank, comments (`:`), non-`data` fields, or `data:` payloads that are not valid
/// JSON are skipped (malformed JSON is logged at debug level and is not fatal).
#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut out = Vec::new();
        self.buf.push_str(chunk);
        let mut start = 0;
        while let Some(rel) = self.buf[start..].find('\n') {
            let end = start + rel;
            parse_line(&self.buf[start..end], &mut out);
            start = end + 1;
        }
        self.buf.drain(..start);
        out
    }

    /// Process a trailing line that never got a newline (end of stream).
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        parse_line(&self.buf, &mut out);
        self.buf.clear();
        out
    }
}

fn parse_line(line: &str, out: &mut Vec<SseEvent>) {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let Some(rest) = line.strip_prefix("data:") else {
        return;
    };
    let data = rest.strip_prefix(' ').unwrap_or(rest).trim();
    if data.is_empty() {
        return;
    }
    if data == "[DONE]" {
        out.push(SseEvent::Done);
        return;
    }
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("skipping malformed SSE data line: {e}");
            return;
        }
    };
    if let Some(err) = value.get("error").filter(|e| !e.is_null()) {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| err.as_str().map(str::to_owned))
            .unwrap_or_else(|| err.to_string());
        out.push(SseEvent::Error(msg));
        return;
    }
    if let Some(text) = value
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        out.push(SseEvent::Delta(text.to_owned()));
    }
    if let Some(u) = value.get("usage").filter(|u| u.is_object()) {
        let n = |k: &str| {
            u.get(k)
                .and_then(Value::as_u64)
                .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX))
        };
        out.push(SseEvent::Usage(Usage {
            prompt_tokens: n("prompt_tokens"),
            completion_tokens: n("completion_tokens"),
        }));
    }
}

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

/// Removes `<think>…</think>` blocks from streamed text, tolerating tags split across chunks.
/// An unterminated block suppresses everything after it.
#[derive(Debug, Default)]
pub struct ThinkFilter {
    inside: bool,
    /// Held-back text that might be the start of a tag.
    held: String,
}

/// Length of the longest proper prefix of `tag` that `s` ends with.
fn partial_tag_suffix(s: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&n| s.ends_with(&tag[..n]))
        .unwrap_or(0)
}

impl ThinkFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed text; returns the newly visible text.
    pub fn push(&mut self, text: &str) -> String {
        self.held.push_str(text);
        let mut out = String::new();
        loop {
            if self.inside {
                if let Some(i) = self.held.find(CLOSE) {
                    self.held.drain(..i + CLOSE.len());
                    self.inside = false;
                } else {
                    let keep = partial_tag_suffix(&self.held, CLOSE);
                    let cut = self.held.len() - keep;
                    self.held.drain(..cut);
                    return out;
                }
            } else if let Some(i) = self.held.find(OPEN) {
                out.push_str(&self.held[..i]);
                self.held.drain(..i + OPEN.len());
                self.inside = true;
            } else {
                let keep = partial_tag_suffix(&self.held, OPEN);
                let cut = self.held.len() - keep;
                out.push_str(&self.held[..cut]);
                self.held.drain(..cut);
                return out;
            }
        }
    }

    /// End of stream: returns held-back visible text (a partial tag that never completed).
    /// Anything inside an unterminated think block is dropped.
    pub fn finish(&mut self) -> String {
        let held = std::mem::take(&mut self.held);
        if self.inside { String::new() } else { held }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = concat!(
        ": keep-alive\r\n",
        "\r\n",
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\r\n",
        "\r\n",
        "event: ping\n",
        "data:{\"choices\":[{\"delta\":{\"reasoning\":\"hmm\",\"reasoning_content\":\"hmm\"}}]}\n",
        "data: \n",
        "data: not json\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo \\u00e9\"}}]}\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\n",
        "data: [DONE]\n",
    );

    fn expected() -> Vec<SseEvent> {
        vec![
            SseEvent::Delta("Hel".into()),
            SseEvent::Delta("lo \u{e9}".into()),
            SseEvent::Usage(Usage {
                prompt_tokens: 12,
                completion_tokens: 3,
            }),
            SseEvent::Done,
        ]
    }

    #[test]
    fn parses_whole_body() {
        let mut p = SseParser::new();
        assert_eq!(p.feed(BODY), expected());
    }

    #[test]
    fn every_split_point_gives_same_events() {
        for i in (0..=BODY.len()).filter(|&i| BODY.is_char_boundary(i)) {
            let mut p = SseParser::new();
            let mut got = p.feed(&BODY[..i]);
            got.extend(p.feed(&BODY[i..]));
            assert_eq!(got, expected(), "split at {i}");
        }
    }

    #[test]
    fn byte_at_a_time() {
        let mut p = SseParser::new();
        let mut got = Vec::new();
        let mut tmp = [0u8; 4];
        for c in BODY.chars() {
            got.extend(p.feed(c.encode_utf8(&mut tmp)));
        }
        assert_eq!(got, expected());
    }

    #[test]
    fn error_object_is_surfaced() {
        let mut p = SseParser::new();
        let ev = p.feed("data: {\"error\":{\"message\":\"model gone\"}}\n");
        assert_eq!(ev, vec![SseEvent::Error("model gone".into())]);
    }

    #[test]
    fn trailing_line_without_newline_needs_finish() {
        let mut p = SseParser::new();
        assert!(p.feed("data: [DONE]").is_empty());
        assert_eq!(p.finish(), vec![SseEvent::Done]);
    }

    fn run_filter(chunks: &[&str]) -> String {
        let mut f = ThinkFilter::new();
        let mut s: String = chunks.iter().map(|c| f.push(c)).collect();
        s.push_str(&f.finish());
        s
    }

    #[test]
    fn think_split_at_every_offset() {
        let text = "a<think>secret</think>b";
        for i in 0..=text.len() {
            assert_eq!(run_filter(&[&text[..i], &text[i..]]), "ab", "split {i}");
        }
    }

    #[test]
    fn think_char_at_a_time() {
        let text = "x<think>one</think>y<think>two</think>\nz";
        let chunks: Vec<String> = text.chars().map(String::from).collect();
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        assert_eq!(run_filter(&refs), "xy\nz");
    }

    #[test]
    fn unterminated_think_suppresses_rest() {
        assert_eq!(run_filter(&["hi <think>never ", "ends </thin"]), "hi ");
    }

    #[test]
    fn non_tags_pass_through_intact() {
        assert_eq!(
            run_filter(&["a < b <th", "ing and <thi"]),
            "a < b <thing and <thi"
        );
        assert_eq!(run_filter(&["<", "/think> stray"]), "</think> stray");
    }

    #[test]
    fn nested_looking_text_closes_at_first_close_tag() {
        assert_eq!(
            run_filter(&["<think>a<think>b</think>c</think>d"]),
            "c</think>d"
        );
    }
}
