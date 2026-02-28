use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use anyhow::Result;

use super::persistence::{SessionHeader, SessionEntry};

/// Manages session lifecycle: create, load, save, list
pub struct SessionManager {
    /// Directory where sessions are stored
    sessions_dir: PathBuf,
    /// Currently active session file
    current_session: Option<PathBuf>,
    /// Session ID
    session_id: Option<String>,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            current_session: None,
            session_id: None,
        }
    }

    /// Create a new session
    pub async fn create_session(&mut self, cwd: &str) -> Result<String> {
        fs::create_dir_all(&self.sessions_dir).await?;
        let id = Uuid::new_v4().to_string();
        let header = SessionHeader::new(id.clone(), cwd.to_string());

        let filename = format!("{}.jsonl", &id[..8]);
        let path = self.sessions_dir.join(&filename);

        let mut file = fs::File::create(&path).await?;
        let header_json = serde_json::to_string(&header)?;
        file.write_all(header_json.as_bytes()).await?;
        file.write_all(b"\n").await?;

        self.current_session = Some(path);
        self.session_id = Some(id.clone());
        Ok(id)
    }

    /// Append an entry to the current session
    pub async fn append_entry(&self, entry: &SessionEntry) -> Result<()> {
        let path = self.current_session.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?;

        let json = serde_json::to_string(entry)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        Ok(())
    }

    /// List available sessions (returns session IDs and paths)
    pub async fn list_sessions(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut sessions = Vec::new();
        if !self.sessions_dir.exists() {
            return Ok(sessions);
        }

        let mut dir = fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Ok(content) = fs::read_to_string(&path).await {
                    if let Some(first_line) = content.lines().next() {
                        if let Ok(header) = serde_json::from_str::<SessionHeader>(first_line) {
                            sessions.push((header.id, path));
                        }
                    }
                }
            }
        }
        Ok(sessions)
    }

    /// Get current session ID
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Get current session path
    pub fn session_path(&self) -> Option<&Path> {
        self.current_session.as_deref()
    }
}
