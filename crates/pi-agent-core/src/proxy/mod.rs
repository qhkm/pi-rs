use serde::{Deserialize, Serialize};
use pi_ai::StreamEvent;

/// A serializable stream event for transport over HTTP/WebSocket.
/// StreamEvent itself isn't serializable (it carries full AssistantMessage),
/// so this provides a lightweight wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyEvent {
    /// Stream started
    Start,
    /// Text content delta
    TextDelta { delta: String },
    /// Thinking/reasoning delta
    ThinkingDelta { delta: String },
    /// Tool call started
    ToolCallStart {
        content_index: usize,
    },
    /// Tool call argument delta
    ToolCallDelta {
        content_index: usize,
        delta: String,
    },
    /// Tool call completed
    ToolCallEnd {
        content_index: usize,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Stream completed
    Done {
        stop_reason: String,
        text: String,
    },
    /// Stream errored
    Error {
        message: String,
    },
}

impl ProxyEvent {
    /// Convert a pi_ai StreamEvent to a serializable ProxyEvent
    pub fn from_stream_event(event: &StreamEvent) -> Self {
        match event {
            StreamEvent::Start { .. } => ProxyEvent::Start,
            StreamEvent::TextDelta { delta, .. } => ProxyEvent::TextDelta { delta: delta.clone() },
            StreamEvent::ThinkingDelta { delta, .. } => ProxyEvent::ThinkingDelta { delta: delta.clone() },
            StreamEvent::ToolCallStart { content_index, .. } => ProxyEvent::ToolCallStart { content_index: *content_index },
            StreamEvent::ToolCallDelta { content_index, delta, .. } => ProxyEvent::ToolCallDelta {
                content_index: *content_index,
                delta: delta.clone(),
            },
            StreamEvent::ToolCallEnd { content_index, tool_call, .. } => ProxyEvent::ToolCallEnd {
                content_index: *content_index,
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            },
            StreamEvent::Done { reason, message, .. } => ProxyEvent::Done {
                stop_reason: reason.to_string(),
                text: message.text(),
            },
            StreamEvent::Error { error, .. } => ProxyEvent::Error {
                message: error.error_message.clone().unwrap_or_default(),
            },
            // For events we don't need to proxy individually
            StreamEvent::TextStart { .. }
            | StreamEvent::TextEnd { .. }
            | StreamEvent::ThinkingStart { .. }
            | StreamEvent::ThinkingEnd { .. } => {
                ProxyEvent::Start // Simplified
            }
        }
    }

    /// Serialize to JSON line (for JSONL transport)
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Helper to convert a slice of ProxyEvents to newline-delimited JSON
pub fn events_to_ndjson(events: &[ProxyEvent]) -> String {
    events.iter()
        .map(|e| e.to_json_line())
        .collect::<Vec<_>>()
        .join("\n")
}
