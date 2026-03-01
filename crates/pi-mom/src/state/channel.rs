//! Per-channel conversation state.

use anyhow::Result;
use pi_agent_core::messages::AgentMessage;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// State for a single Slack channel/DM
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub id: String,
    inner: Arc<RwLock<ChannelStateInner>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChannelStateInner {
    pub messages: Vec<AgentMessage>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub thread_ts: Option<String>,
}

impl ChannelState {
    /// Create new channel state
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            inner: Arc::new(RwLock::new(ChannelStateInner {
                messages: Vec::new(),
                last_activity: chrono::Utc::now(),
                thread_ts: None,
            })),
        }
    }

    /// Add a message to the conversation
    pub fn add_message(&self, message: AgentMessage) -> Result<()> {
        let mut inner = self.inner.write().unwrap();
        inner.messages.push(message);
        inner.last_activity = chrono::Utc::now();
        Ok(())
    }

    /// Get all messages in the conversation
    pub fn get_messages(&self) -> Vec<AgentMessage> {
        let inner = self.inner.read().unwrap();
        inner.messages.clone()
    }

    /// Get recent messages (up to N)
    pub fn get_recent(&self, n: usize) -> Vec<AgentMessage> {
        let inner = self.inner.read().unwrap();
        inner.messages.iter().rev().take(n).rev().cloned().collect()
    }

    /// Clear conversation history
    pub fn clear(&self) -> Result<()> {
        let mut inner = self.inner.write().unwrap();
        inner.messages.clear();
        inner.last_activity = chrono::Utc::now();
        Ok(())
    }

    /// Set thread timestamp for this conversation
    pub fn set_thread_ts(&self, ts: impl Into<String>) -> Result<()> {
        let mut inner = self.inner.write().unwrap();
        inner.thread_ts = Some(ts.into());
        Ok(())
    }

    /// Get thread timestamp
    pub fn get_thread_ts(&self) -> Option<String> {
        let inner = self.inner.read().unwrap();
        inner.thread_ts.clone()
    }

    /// Get message count
    pub fn message_count(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner.messages.len()
    }

    /// Get last activity time
    pub fn last_activity(&self) -> chrono::DateTime<chrono::Utc> {
        let inner = self.inner.read().unwrap();
        inner.last_activity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_ai::Message;

    #[test]
    fn test_channel_state() {
        let state = ChannelState::new("C123");
        assert_eq!(state.id, "C123");
        
        let message = AgentMessage::from_llm(Message::user("Hello"));
        state.add_message(message).unwrap();
        
        assert_eq!(state.message_count(), 1);
        
        let messages = state.get_messages();
        assert_eq!(messages.len(), 1);
    }
}
