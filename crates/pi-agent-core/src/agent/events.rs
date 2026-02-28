use pi_ai::{Message, StreamEvent, Usage};
use serde::{Deserialize, Serialize};

/// Reason the agent stopped
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEndReason {
    Completed,
    MaxTurns,
    Aborted,
    Error(String),
    ContextOverflow,
}

/// Events emitted by the agent runtime
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent execution started
    AgentStart { agent_id: String },
    /// Agent execution ended
    AgentEnd { agent_id: String, reason: AgentEndReason },
    /// A new turn in the agent loop started
    TurnStart { turn_index: usize },
    /// A turn in the agent loop ended
    TurnEnd { turn_index: usize, message: Option<Message> },
    /// A message (user/assistant/tool_result) started being processed
    MessageStart { message_id: String, role: String },
    /// Streaming update to the current assistant message
    MessageUpdate { message_id: String, event: StreamEvent },
    /// A message finished
    MessageEnd { message_id: String, usage: Option<Usage> },
    /// Tool execution started
    ToolExecutionStart {
        tool_name: String,
        call_id: String,
        arguments: serde_json::Value,
    },
    /// Tool execution progress update (streaming)
    ToolExecutionUpdate {
        call_id: String,
        progress: String,
    },
    /// Tool execution completed
    ToolExecutionEnd {
        call_id: String,
        tool_name: String,
        result: String,
        duration_ms: u64,
        is_error: bool,
    },
    /// Auto-compaction started
    AutoCompactionStart { reason: String },
    /// Auto-compaction ended
    AutoCompactionEnd {
        success: bool,
        tokens_before: u64,
        tokens_after: Option<u64>,
        error: Option<String>,
    },
}
