use crate::messages::types::{AssistantMessage, StopReason};
use crate::tools::schema::ToolCall;

// ─── Stream events ────────────────────────────────────────────────────────────

/// All events that can be emitted during a streaming LLM response.
///
/// Each variant carries a `partial` snapshot of the `AssistantMessage` as it
/// has been built so far, so consumers can render intermediate state without
/// keeping their own accumulation buffers.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// The stream has started; `partial` is the empty initial message.
    Start {
        partial: AssistantMessage,
    },

    // ── Text events ──────────────────────────────────────────────────────────

    /// A new text content block is starting at `content_index`.
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    /// A text delta has been appended at `content_index`.
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    /// The text block at `content_index` is complete; `content` is the full
    /// accumulated text.
    TextEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },

    // ── Thinking / reasoning events ──────────────────────────────────────────

    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },

    // ── Tool call events ─────────────────────────────────────────────────────

    ToolCallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    /// A delta to the JSON arguments string of the tool call.
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    /// The tool call at `content_index` is complete.
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },

    // ── Terminal events ──────────────────────────────────────────────────────

    /// The model finished successfully.
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    /// The model finished with an error (e.g. content policy, auth failure).
    Error {
        reason: StopReason,
        error: AssistantMessage,
    },
}

impl StreamEvent {
    /// Returns `true` for the two terminal events.
    pub fn is_complete(&self) -> bool {
        matches!(self, StreamEvent::Done { .. } | StreamEvent::Error { .. })
    }

    /// Returns a reference to the partial (or final) `AssistantMessage`
    /// carried by every event variant.
    pub fn partial_message(&self) -> &AssistantMessage {
        match self {
            StreamEvent::Start { partial }
            | StreamEvent::TextStart { partial, .. }
            | StreamEvent::TextDelta { partial, .. }
            | StreamEvent::TextEnd { partial, .. }
            | StreamEvent::ThinkingStart { partial, .. }
            | StreamEvent::ThinkingDelta { partial, .. }
            | StreamEvent::ThinkingEnd { partial, .. }
            | StreamEvent::ToolCallStart { partial, .. }
            | StreamEvent::ToolCallDelta { partial, .. }
            | StreamEvent::ToolCallEnd { partial, .. } => partial,
            StreamEvent::Done { message, .. } => message,
            StreamEvent::Error { error, .. } => error,
        }
    }

    /// Returns the stop reason if this is a terminal event.
    pub fn stop_reason(&self) -> Option<&StopReason> {
        match self {
            StreamEvent::Done { reason, .. } | StreamEvent::Error { reason, .. } => Some(reason),
            _ => None,
        }
    }
}
