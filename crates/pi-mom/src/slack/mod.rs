pub mod events;
pub mod socket_mode;

use serde::{Deserialize, Serialize};

/// A Slack event received by the bot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackEvent {
    pub event_type: SlackEventType,
    pub channel: String,
    pub ts: String,
    pub user: String,
    pub text: String,
    #[serde(default)]
    pub files: Vec<SlackFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackEventType {
    Mention,
    DirectMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackFile {
    pub name: String,
    pub url: String,
}

/// Context for responding to Slack messages
pub struct SlackContext {
    pub channel: String,
    pub thread_ts: Option<String>,
    pub user: String,
}
