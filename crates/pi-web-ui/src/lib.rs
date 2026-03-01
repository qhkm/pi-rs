//! Web UI components for pi in the browser.
//!
//! Provides storage-backed session management and tool execution
//! for a web-based interface (WebAssembly compatible).

pub mod attachments;
pub mod storage;
pub mod tools;

use pi_ai::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Web UI session handle
#[derive(Debug, Clone)]
pub struct WebSession {
    pub session_id: String,
    pub storage: storage::Storage,
}

impl WebSession {
    /// Create a new web session
    pub fn new(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            storage: storage::Storage::new(&session_id),
            session_id,
        }
    }

    /// Load an existing session
    pub fn load(session_id: impl Into<String>) -> anyhow::Result<Self> {
        let session_id = session_id.into();
        let storage = storage::Storage::new(&session_id);
        
        // Verify session exists
        if !storage.exists() {
            anyhow::bail!("Session '{}' not found", session_id);
        }
        
        Ok(Self {
            storage,
            session_id,
        })
    }

    /// Add a user message to the session
    pub fn add_user_message(&mut self, content: impl Into<String>) -> anyhow::Result<()> {
        let message = Message::user(content);
        self.storage.append_message(&message)?;
        Ok(())
    }

    /// Get all messages in the session
    pub fn get_messages(&self) -> anyhow::Result<Vec<Message>> {
        self.storage.get_messages()
    }

    /// Get session metadata
    pub fn get_metadata(&self) -> anyhow::Result<storage::types::SessionMetadata> {
        self.storage.get_metadata()
    }

    /// Update session metadata
    pub fn update_metadata(&mut self, metadata: &storage::types::SessionMetadata) -> anyhow::Result<()> {
        self.storage.save_metadata(metadata)
    }

    /// Clear the session
    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.storage.clear()
    }

    /// Export session to JSON
    pub fn export_json(&self) -> anyhow::Result<String> {
        let messages = self.get_messages()?;
        let metadata = self.get_metadata()?;
        
        let export = SessionExport {
            session_id: self.session_id.clone(),
            metadata,
            messages: messages.into_iter().map(|m| m.text_content()).collect(),
        };
        
        Ok(serde_json::to_string_pretty(&export)?)
    }
}

/// Export format for sessions
#[derive(Debug, Serialize, Deserialize)]
struct SessionExport {
    session_id: String,
    metadata: storage::types::SessionMetadata,
    messages: Vec<String>,
}

/// List all available sessions
pub fn list_sessions() -> anyhow::Result<Vec<storage::types::SessionMetadata>> {
    storage::Storage::list_sessions()
}

/// Delete a session
pub fn delete_session(session_id: &str) -> anyhow::Result<()> {
    storage::Storage::delete(session_id)
}

/// Web UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub default_provider: String,
    pub default_model: String,
    pub theme: String,
    pub api_keys: HashMap<String, String>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            default_provider: "openai".to_string(),
            default_model: "gpt-4".to_string(),
            theme: "dark".to_string(),
            api_keys: HashMap::new(),
        }
    }
}

impl WebConfig {
    /// Load configuration
    pub fn load() -> anyhow::Result<Self> {
        storage::Storage::load_config()
    }

    /// Save configuration
    pub fn save(&self) -> anyhow::Result<()> {
        storage::Storage::save_config(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_session_creation() {
        let session = WebSession::new("test-session");
        assert_eq!(session.session_id, "test-session");
    }

    #[test]
    fn test_web_config_default() {
        let config = WebConfig::default();
        assert_eq!(config.default_provider, "openai");
        assert_eq!(config.theme, "dark");
    }
}
