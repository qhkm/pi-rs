/// Server-Sent Events (SSE) parser.
///
/// Parses a raw byte stream from a `reqwest` response body into discrete
/// `SseEvent` structs.  Follows the W3C specification:
/// <https://html.spec.whatwg.org/multipage/server-sent-events.html>
use bytes::Bytes;
use futures::Stream;
use std::collections::VecDeque;
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
    let mut event = SseEvent {
        event: "message".to_string(),
        ..Default::default()
    };
    let mut has_field = false;

    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if !line.trim().is_empty() {
            parse_field(line, &mut event);
            has_field = true;
        }
    }

    if has_field {
        Some(event)
    } else {
        None
    }
}

// ─── Streaming parser ─────────────────────────────────────────────────────────

/// Wraps a `reqwest` byte stream and emits `SseEvent`s on demand.
pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    /// Events that have been parsed from the buffer but not yet yielded.
    pending_events: VecDeque<SseEvent>,
    done: bool,
}

impl SseStream {
    pub fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        SseStream {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending_events: VecDeque::new(),
            done: false,
        }
    }

    /// Extract all complete SSE blocks from the internal buffer and push each
    /// parsed event onto `self.pending_events`.  A block is terminated by a
    /// blank line (`\n\n` or `\r\n\r\n`).
    fn drain_events(&mut self) {
        while let Some(pos) = find_double_newline(&self.buffer) {
            let block: String = self.buffer.drain(..pos).collect();
            // Consume the separator.
            let sep_len = if self.buffer.starts_with("\r\n\r\n") {
                4
            } else {
                2
            };
            self.buffer.drain(..sep_len);

            if let Some(event) = parse_block(&block) {
                self.pending_events.push_back(event);
            }
        }
    }
}

/// Return the position of the earliest double-newline separator in `s`.
///
/// Both `\n\n` and `\r\n\r\n` are recognised as SSE block terminators.
/// Whichever starts at a lower byte offset is returned so that events are
/// never reordered when a stream mixes the two styles.
fn find_double_newline(s: &str) -> Option<usize> {
    let crlf = find_substr(s, "\r\n\r\n");
    let lf = find_substr(s, "\n\n");

    match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find_substr(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

impl Stream for SseStream {
    type Item = Result<SseEvent, PiAiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // First: return any already-parsed events one at a time.
            if let Some(event) = self.pending_events.pop_front() {
                return Poll::Ready(Some(Ok(event)));
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
                    // Parse whatever complete blocks are now available, then
                    // loop back to yield them from pending_events.
                    self.drain_events();
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
    use futures::StreamExt;

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

    // ── Bug-fix regression tests ──────────────────────────────────────────────

    /// Bug 1: multiple events arriving in one chunk must all be returned, not
    /// just the first one.
    #[tokio::test]
    async fn test_multiple_events_in_one_chunk_all_returned() {
        // Two complete SSE events concatenated in a single chunk.
        let chunk = Bytes::from("data: first\n\ndata: second\n\n");

        let byte_stream = futures::stream::iter(vec![Ok::<Bytes, reqwest::Error>(chunk)]);
        let mut sse = SseStream::new(byte_stream);

        let first = sse.next().await.expect("expected first event").unwrap();
        assert_eq!(first.data, "first", "first event must not be dropped");

        let second = sse.next().await.expect("expected second event").unwrap();
        assert_eq!(second.data, "second", "second event must not be dropped");

        assert!(
            sse.next().await.is_none(),
            "stream should be exhausted after two events"
        );
    }

    /// Bug 2: when `\n\n` appears before `\r\n\r\n` in the buffer, the earlier
    /// `\n\n` separator must be chosen so that events are not reordered.
    #[test]
    fn test_find_double_newline_prefers_earlier_position() {
        // Layout: "data: a\n\ndata: b\r\n\r\n"
        //                      ^^            ^^^^
        //         \n\n at offset 8, \r\n\r\n at offset 17
        let s = "data: a\n\ndata: b\r\n\r\n";
        let pos = find_double_newline(s).unwrap();
        // The \n\n is at byte 7 (after "data: a"), which is less than 15
        // (\r\n\r\n after "data: b").
        assert_eq!(
            pos,
            s.find("\n\n").unwrap(),
            "should pick the earlier \\n\\n, not the later \\r\\n\\r\\n"
        );
    }

    /// Mixed separators: a stream where blocks are separated by `\n\n` and
    /// `\r\n\r\n` in alternation must yield every event in order.
    #[tokio::test]
    async fn test_mixed_separators_all_events_returned() {
        // Block 1 terminated by \n\n, block 2 terminated by \r\n\r\n.
        let chunk = Bytes::from("data: alpha\n\ndata: beta\r\n\r\ndata: gamma\n\n");

        let byte_stream = futures::stream::iter(vec![Ok::<Bytes, reqwest::Error>(chunk)]);
        let mut sse = SseStream::new(byte_stream);

        let e1 = sse.next().await.unwrap().unwrap();
        assert_eq!(e1.data, "alpha");

        let e2 = sse.next().await.unwrap().unwrap();
        assert_eq!(e2.data, "beta");

        let e3 = sse.next().await.unwrap().unwrap();
        assert_eq!(e3.data, "gamma");

        assert!(sse.next().await.is_none());
    }

    /// A single SSE event whose bytes arrive split across two separate chunks
    /// must still be assembled and returned correctly.
    #[tokio::test]
    async fn test_partial_event_across_chunk_boundaries() {
        // The blank-line terminator (\n\n) is in the second chunk.
        let chunk1 = Bytes::from("data: par");
        let chunk2 = Bytes::from("tial\n\n");

        let byte_stream = futures::stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(chunk1),
            Ok::<Bytes, reqwest::Error>(chunk2),
        ]);
        let mut sse = SseStream::new(byte_stream);

        let event = sse.next().await.expect("expected one event").unwrap();
        assert_eq!(
            event.data, "partial",
            "event split across chunks must be reassembled correctly"
        );

        assert!(sse.next().await.is_none());
    }
}
