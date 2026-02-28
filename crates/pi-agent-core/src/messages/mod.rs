pub mod queue;

use pi_ai::Message;
use serde::{Deserialize, Serialize};

/// Agent-level message that wraps pi-ai Messages with agent-specific variants.
/// In the TS version this uses declaration merging; in Rust we use an enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentMessage {
    /// Standard LLM message
    Llm(Message),
    /// System-injected context (skills, memory, etc.)
    SystemContext {
        content: String,
        source: String,
    },
    /// Compaction summary replacing older messages
    CompactionSummary {
        summary: String,
        replaced_count: usize,
        original_token_count: u64,
    },
    /// Extension-injected custom message
    Extension {
        type_name: String,
        data: serde_json::Value,
        /// Whether this message should be included in LLM context
        include_in_context: bool,
    },
}

impl AgentMessage {
    /// Convert to an LLM message for API calls.
    /// Returns None for messages that shouldn't be in the LLM context.
    pub fn to_llm_message(&self) -> Option<Message> {
        match self {
            AgentMessage::Llm(msg) => Some(msg.clone()),
            AgentMessage::SystemContext { content, .. } => {
                Some(Message::user(content.clone()))
            }
            AgentMessage::CompactionSummary { summary, .. } => {
                Some(Message::user(
                    format!("[Previous conversation summary]\n{summary}"),
                ))
            }
            AgentMessage::Extension { data, include_in_context, .. } => {
                if *include_in_context {
                    Some(Message::user(data.to_string()))
                } else {
                    None
                }
            }
        }
    }

    /// Create from an LLM message
    pub fn from_llm(msg: Message) -> Self {
        AgentMessage::Llm(msg)
    }

    /// Check if this is an LLM message
    pub fn as_llm(&self) -> Option<&Message> {
        match self {
            AgentMessage::Llm(msg) => Some(msg),
            _ => None,
        }
    }
}

/// Convert a slice of AgentMessages to LLM Messages for API calls
pub fn to_llm_messages(messages: &[AgentMessage]) -> Vec<Message> {
    messages.iter().filter_map(|m| m.to_llm_message()).collect()
}

/// Estimate token count for an agent message (chars/4 heuristic, same as TS version)
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars = match message {
        AgentMessage::Llm(msg) => {
            // text_content() returns the concatenated text across all content blocks
            msg.text_content().len()
        }
        AgentMessage::SystemContext { content, .. } => content.len(),
        AgentMessage::CompactionSummary { summary, .. } => summary.len(),
        AgentMessage::Extension { data, .. } => data.to_string().len(),
    };
    (chars as u64) / 4
}
