/// Server-Sent Events (SSE) parser.
///
/// Parses a raw byte stream from a `reqwest` response body into discrete
/// `SseEvent` structs.  Follows the W3C specification:
/// <https://html.spec.whatwg.org/multipage/server-sent-events.html>
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::error::PiAiError;

// ─── SSE Event ────────────────────────────────────────────────────────────────

/// A single parsed SSE message.
#[derive(Debug, Clone, Default)]
pub struct SseEvent {
    /// Contents of the `event:` field (defaults to `"message"` if absent).
    pub event: String,
    /// Contents of the `data:` field(s), joined with `\n`.
    pub data: String,
    /// Contents of the `id:` field, if present.
    pub id: Option<String>,
    /// Contents of the `retry:` field (milliseconds), if present.
    pub retry: Option<u64>,
}

impl SseEvent {
    /// Returns `true` for the sentinel value that signals the end of the stream
    /// (`data: [DONE]` used by OpenAI-compatible APIs).
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }

    /// Parse the `data` field as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> crate::error::Result<T> {
        serde_json::from_str(&self.data).map_err(PiAiError::Json)
    }
}

// ─── Line-level parser ────────────────────────────────────────────────────────

/// Parse a single field line according to the SSE spec.
fn parse_field(line: &str, event: &mut SseEvent) {
    if let Some(value) = line.strip_prefix("data:") {
        let value = value.strip_prefix(' ').unwrap_or(value);
        if !event.data.is_empty() {
            event.data.push('\n');
        }
        event.data.push_str(value);
    } else if let Some(value) = line.strip_prefix("event:") {
        event.event = value.strip_prefix(' ').unwrap_or(value).to_string();
    } else if let Some(value) = line.strip_prefix("id:") {
        event.id = Some(value.strip_prefix(' ').unwrap_or(value).to_string());
    } else if let Some(value) = line.strip_prefix("retry:") {
        if let Ok(ms) = value.strip_prefix(' ').unwrap_or(value).parse::<u64>() {
            event.retry = Some(ms);
        }
    }
    // Lines starting with ':' are comments — ignored.
}

/// Parse a complete SSE block (lines separated by `\n` or `\r\n`) into an
/// `SseEvent`.  Returns `None` if the block is empty / whitespace-only.
pub fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event = SseEvent { event: "message".to_string(), ..Default::default() };
    let mut has_field = false;

    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if !line.trim().is_empty() {
            parse_field(line, &mut event);
            has_field = true;
        }
    }

    if has_field { Some(event) } else { None }
}

// ─── Streaming parser ─────────────────────────────────────────────────────────

/// Wraps a `reqwest` byte stream and emits `SseEvent`s on demand.
pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    done: bool,
}

impl SseStream {
    pub fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        SseStream {
            inner: Box::pin(stream),
            buffer: String::new(),
            done: false,
        }
    }

    /// Try to extract complete SSE blocks from the internal buffer.
    /// A block is terminated by a blank line (`\n\n` or `\r\n\r\n`).
    fn drain_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // Split on double newline (either \n\n or \r\n\r\n).
        while let Some(pos) = find_double_newline(&self.buffer) {
            let block: String = self.buffer.drain(..pos).collect();
            // Consume the separator.
            let sep_len = if self.buffer.starts_with("\r\n\r\n") { 4 } else { 2 };
            self.buffer.drain(..sep_len);

            if let Some(event) = parse_block(&block) {
                events.push(event);
            }
        }

        events
    }
}

fn find_double_newline(s: &str) -> Option<usize> {
    // Check for \r\n\r\n first, then \n\n.
    if let Some(pos) = find_substr(s, "\r\n\r\n") {
        return Some(pos);
    }
    find_substr(s, "\n\n")
}

fn find_substr(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

impl Stream for SseStream {
    type Item = Result<SseEvent, PiAiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // First: try to return buffered events.
            let buffered = self.drain_events();
            if let Some(first) = buffered.into_iter().next() {
                return Poll::Ready(Some(Ok(first)));
            }

            if self.done {
                return Poll::Ready(None);
            }

            // Poll inner stream for more bytes.
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if let Ok(s) = std::str::from_utf8(&chunk) {
                        self.buffer.push_str(s);
                    }
                    // Loop back to try draining again.
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(PiAiError::Http(e))));
                }
                Poll::Ready(None) => {
                    self.done = true;
                    // Drain any trailing data without a final blank line.
                    let remaining = std::mem::take(&mut self.buffer);
                    if !remaining.trim().is_empty() {
                        if let Some(event) = parse_block(&remaining) {
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ─── Helper: parse from a reqwest Response ───────────────────────────────────

/// Create an SSE stream from a `reqwest::Response`.
pub fn sse_stream_from_response(response: reqwest::Response) -> SseStream {
    SseStream::new(response.bytes_stream())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_block_simple() {
        let block = "data: hello world";
        let event = parse_block(block).unwrap();
        assert_eq!(event.data, "hello world");
        assert_eq!(event.event, "message");
    }

    #[test]
    fn test_parse_block_with_event() {
        let block = "event: delta\ndata: {\"text\": \"hi\"}";
        let event = parse_block(block).unwrap();
        assert_eq!(event.event, "delta");
        assert_eq!(event.data, "{\"text\": \"hi\"}");
    }

    #[test]
    fn test_parse_block_multiline_data() {
        let block = "data: line1\ndata: line2";
        let event = parse_block(block).unwrap();
        assert_eq!(event.data, "line1\nline2");
    }

    #[test]
    fn test_done_sentinel() {
        let block = "data: [DONE]";
        let event = parse_block(block).unwrap();
        assert!(event.is_done());
    }

    #[test]
    fn test_empty_block_returns_none() {
        let event = parse_block("   \n  \n");
        assert!(event.is_none());
    }

    #[test]
    fn test_comment_ignored() {
        let block = ": this is a comment\ndata: actual";
        let event = parse_block(block).unwrap();
        assert_eq!(event.data, "actual");
    }
}
