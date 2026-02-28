use serde::{Deserialize, Serialize};

/// Commands received from stdin (one JSON object per line).
///
/// Each command has a `type` discriminant and an optional `id` that is echoed
/// back in the corresponding `RpcResponse` so the caller can correlate
/// request/response pairs.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum RpcCommand {
    #[serde(rename = "prompt")]
    Prompt { id: Option<String>, message: String },
    #[serde(rename = "abort")]
    Abort { id: Option<String> },
    #[serde(rename = "get_state")]
    GetState { id: Option<String> },
    #[serde(rename = "get_messages")]
    GetMessages { id: Option<String> },
    #[serde(rename = "compact")]
    Compact {
        id: Option<String>,
        /// Optional extra instructions to append to the compaction prompt.
        custom_instructions: Option<String>,
    },
    #[serde(rename = "set_auto_compaction")]
    SetAutoCompaction {
        id: Option<String>,
        /// Whether to enable (`true`) or disable (`false`) auto-compaction.
        enabled: bool,
    },
    /// Return the current agent configuration (model, thinking level, compaction settings).
    #[serde(rename = "get_config")]
    GetConfig { id: Option<String> },
}

impl RpcCommand {
    /// Return the caller-supplied correlation id, if any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Prompt { id, .. } => id.as_deref(),
            Self::Abort { id, .. } => id.as_deref(),
            Self::GetState { id, .. } => id.as_deref(),
            Self::GetMessages { id, .. } => id.as_deref(),
            Self::Compact { id, .. } => id.as_deref(),
            Self::SetAutoCompaction { id, .. } => id.as_deref(),
            Self::GetConfig { id, .. } => id.as_deref(),
        }
    }

    /// The command type name (matches the `type` discriminant in the JSON).
    pub fn type_name(&self) -> &str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Abort { .. } => "abort",
            Self::GetState { .. } => "get_state",
            Self::GetMessages { .. } => "get_messages",
            Self::Compact { .. } => "compact",
            Self::SetAutoCompaction { .. } => "set_auto_compaction",
            Self::GetConfig { .. } => "get_config",
        }
    }
}

/// Response sent to stdout (one JSON object per line).
#[derive(Debug, Serialize)]
pub struct RpcResponse {
    /// Echo of the caller's correlation id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Always `"response"`.
    #[serde(rename = "type")]
    pub response_type: String,
    /// The command this is responding to (e.g. `"prompt"`, `"abort"`).
    pub command: String,
    /// Whether the command succeeded.
    pub success: bool,
    /// Payload on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Error message on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcResponse {
    pub fn success(id: Option<String>, command: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            id,
            response_type: "response".to_string(),
            command: command.to_string(),
            success: true,
            data,
            error: None,
        }
    }

    pub fn error(id: Option<String>, command: &str, message: &str) -> Self {
        Self {
            id,
            response_type: "response".to_string(),
            command: command.to_string(),
            success: false,
            data: None,
            error: Some(message.to_string()),
        }
    }
}

/// Session state returned by `get_state`.
#[derive(Debug, Serialize)]
pub struct RpcSessionState {
    pub is_streaming: bool,
    pub message_count: usize,
    pub auto_compaction_enabled: bool,
}

/// Compaction result returned by the `compact` command.
#[derive(Debug, Serialize)]
pub struct RpcCompactionResult {
    /// Token count before compaction.
    pub tokens_before: u64,
    /// Token count after compaction.
    pub tokens_after: u64,
    /// Number of messages that were summarised and replaced.
    pub messages_compacted: usize,
}

/// Agent configuration snapshot returned by `get_config`.
#[derive(Debug, Serialize)]
pub struct RpcAgentConfig {
    /// Model id currently in use (e.g. `"claude-opus-4-5"`).
    pub model_id: String,
    /// Human-readable model name.
    pub model_name: String,
    /// Thinking/reasoning level, if configured (e.g. `"medium"`).
    pub thinking_level: Option<String>,
    /// Whether auto-compaction is currently enabled (runtime value).
    pub auto_compaction_enabled: bool,
    /// Token threshold below the context window that triggers auto-compaction.
    pub compaction_reserve_tokens: u64,
    /// Minimum number of recent tokens always preserved from compaction.
    pub compaction_keep_recent_tokens: u64,
}

/// Event wrapper written to stdout (one JSON object per line).
///
/// The `event` field contains the serialised `AgentEvent`.
#[derive(Debug, Serialize)]
pub struct RpcEvent {
    /// Always `"event"`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// The serialised agent event.
    pub event: serde_json::Value,
}
