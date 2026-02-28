use chrono::{DateTime, Utc};
use pi_agent_core::messages::AgentMessage;
use serde::{Deserialize, Serialize};

/// Session file header (first line of JSONL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String, // always "session"
    pub version: u32, // currently 3
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

impl SessionHeader {
    pub fn new(id: String, cwd: String) -> Self {
        Self {
            entry_type: "session".to_string(),
            version: 3,
            id,
            timestamp: Utc::now(),
            cwd,
            parent_session: None,
        }
    }
}

/// A single entry in the JSONL session file
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    /// An LLM message in the conversation
    Message {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
        message: AgentMessage,
    },
    /// A compaction summary
    Compaction {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
    },
    /// Model change
    ModelChange {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
        model: String,
        provider: String,
    },
    /// Thinking level change
    ThinkingLevelChange {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
        level: String,
    },
    /// User-defined label/bookmark
    Label {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        timestamp: DateTime<Utc>,
        label: String,
    },
}

impl SessionEntry {
    /// Stable entry ID for tree/thread relationships.
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message { id, .. }
            | SessionEntry::Compaction { id, .. }
            | SessionEntry::ModelChange { id, .. }
            | SessionEntry::ThinkingLevelChange { id, .. }
            | SessionEntry::Label { id, .. } => id,
        }
    }
}
