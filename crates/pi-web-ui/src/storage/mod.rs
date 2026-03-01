//! Web storage implementation using localStorage (WASM) or filesystem.

pub mod types;

use anyhow::{Context, Result};
use pi_ai::Message;
use serde::{Deserialize, Serialize};
use types::{SessionMetadata, SessionUsage};

/// Storage backend for web sessions
#[derive(Debug, Clone)]
pub struct Storage {
    session_id: String,
}

/// Storage format for messages
#[derive(Debug, Serialize, Deserialize)]
struct StoredMessage {
    pub role: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Session data stored in a single JSON file
#[derive(Debug, Serialize, Deserialize)]
struct SessionData {
    pub metadata: SessionMetadata,
    pub messages: Vec<StoredMessage>,
}

impl Storage {
    /// Create a new storage instance for a session
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }

    /// Get the storage path
    fn storage_path(&self) -> std::path::PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".pi")
            .join("web-ui")
            .join("sessions")
            .join(format!("{}.json", self.session_id))
    }

    /// Check if session exists
    pub fn exists(&self) -> bool {
        self.storage_path().exists()
    }

    /// Initialize a new session
    fn init_session(&self) -> Result<SessionData> {
        let now = chrono::Utc::now();
        let data = SessionData {
            metadata: SessionMetadata {
                id: self.session_id.clone(),
                title: "New Conversation".to_string(),
                created_at: now,
                last_modified: now,
                message_count: 0,
                thinking_level: pi_ai::ThinkingLevel::Minimal,
                preview: String::new(),
                usage: SessionUsage::default(),
            },
            messages: Vec::new(),
        };

        self.save_data(&data)?;
        Ok(data)
    }

    /// Load session data
    fn load_data(&self) -> Result<SessionData> {
        let path = self.storage_path();
        
        if !path.exists() {
            return self.init_session();
        }

        let content = std::fs::read_to_string(&path)?;
        let data: SessionData = serde_json::from_str(&content)?;
        Ok(data)
    }

    /// Save session data
    fn save_data(&self, data: &SessionData) -> Result<()> {
        let path = self.storage_path();
        
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(data)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Append a message to the session
    pub fn append_message(&mut self, message: &Message) -> Result<()> {
        let mut data = self.load_data()?;

        let role = match message {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "tool",
        };

        let stored = StoredMessage {
            role: role.to_string(),
            content: message.text_content(),
            timestamp: chrono::Utc::now(),
        };

        data.messages.push(stored);
        data.metadata.message_count = data.messages.len();
        data.metadata.last_modified = chrono::Utc::now();

        // Update preview (first 100 chars of last user message)
        if role == "user" {
            let preview = message.text_content();
            data.metadata.preview = if preview.len() > 100 {
                format!("{}...", &preview[..100])
            } else {
                preview
            };
        }

        self.save_data(&data)?;
        Ok(())
    }

    /// Get all messages
    pub fn get_messages(&self) -> Result<Vec<Message>> {
        let data = self.load_data()?;
        
        let messages: Vec<Message> = data
            .messages
            .into_iter()
            .map(|m| {
                match m.role.as_str() {
                    "user" => Message::user(m.content),
                    _ => Message::user(format!("[assistant] {}", m.content)), // Simplified
                }
            })
            .collect();

        Ok(messages)
    }

    /// Get metadata
    pub fn get_metadata(&self) -> Result<SessionMetadata> {
        let data = self.load_data()?;
        Ok(data.metadata)
    }

    /// Save metadata
    pub fn save_metadata(&mut self, metadata: &SessionMetadata) -> Result<()> {
        let mut data = self.load_data()?;
        data.metadata = metadata.clone();
        self.save_data(&data)?;
        Ok(())
    }

    /// Clear all messages
    pub fn clear(&mut self) -> Result<()> {
        let mut data = self.load_data()?;
        data.messages.clear();
        data.metadata.message_count = 0;
        data.metadata.preview = String::new();
        data.metadata.last_modified = chrono::Utc::now();
        self.save_data(&data)?;
        Ok(())
    }

    /// List all sessions
    pub fn list_sessions() -> Result<Vec<SessionMetadata>> {
        let sessions_dir = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".pi")
            .join("web-ui")
            .join("sessions");

        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(data) = serde_json::from_str::<SessionData>(&content) {
                        sessions.push(data.metadata);
                    }
                }
            }
        }

        // Sort by last modified, newest first
        sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
        
        Ok(sessions)
    }

    /// Delete a session
    pub fn delete(session_id: &str) -> Result<()> {
        let path = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".pi")
            .join("web-ui")
            .join("sessions")
            .join(format!("{}.json", session_id));

        if path.exists() {
            std::fs::remove_file(&path)?;
        }

        Ok(())
    }

    /// Load global config
    pub fn load_config<T: serde::de::DeserializeOwned>() -> Result<T> {
        let path = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".pi")
            .join("web-ui")
            .join("config.json");

        if !path.exists() {
            anyhow::bail!("Config not found");
        }

        let content = std::fs::read_to_string(&path)?;
        let config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Save global config
    pub fn save_config<T: serde::Serialize>(config: &T) -> Result<()> {
        let path = dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".pi")
            .join("web-ui")
            .join("config.json");

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(config)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_creation() {
        let storage = Storage::new("test");
        assert_eq!(storage.session_id, "test");
    }

    #[test]
    fn test_session_data_serialization() {
        let data = SessionData {
            metadata: SessionMetadata {
                id: "test".to_string(),
                title: "Test".to_string(),
                created_at: chrono::Utc::now(),
                last_modified: chrono::Utc::now(),
                message_count: 0,
                thinking_level: pi_ai::ThinkingLevel::Minimal,
                preview: String::new(),
                usage: SessionUsage::default(),
            },
            messages: vec![],
        };

        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("test"));
    }
}
