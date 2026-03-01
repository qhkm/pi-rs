//! Per-channel state management for Slack bot.

pub mod channel;

use anyhow::Result;
use channel::ChannelState;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Global state manager for all channels
pub struct StateManager {
    channels: Arc<RwLock<HashMap<String, ChannelState>>>,
}

impl StateManager {
    /// Create a new state manager
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create channel state
    pub fn get_channel(&self, channel_id: &str) -> ChannelState {
        let mut channels = self.channels.write().unwrap();
        channels
            .entry(channel_id.to_string())
            .or_insert_with(|| ChannelState::new(channel_id))
            .clone()
    }

    /// Remove a channel's state
    pub fn remove_channel(&self, channel_id: &str) -> Result<()> {
        let mut channels = self.channels.write().unwrap();
        channels.remove(channel_id);
        Ok(())
    }

    /// List all active channels
    pub fn list_channels(&self) -> Vec<String> {
        let channels = self.channels.read().unwrap();
        channels.keys().cloned().collect()
    }

    /// Clear all state
    pub fn clear(&self) -> Result<()> {
        let mut channels = self.channels.write().unwrap();
        channels.clear();
        Ok(())
    }
}

impl Default for StateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_manager() {
        let manager = StateManager::new();
        let channel = manager.get_channel("C123");
        assert_eq!(channel.id, "C123");
        
        let channels = manager.list_channels();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0], "C123");
    }
}
