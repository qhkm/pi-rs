use anyhow::{Context, Result};
use chrono::Utc;
use pi_agent_core::messages::AgentMessage;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::persistence::{SessionEntry, SessionHeader};

/// Manages session lifecycle: create, load, save, list
pub struct SessionManager {
    /// Directory where sessions are stored
    sessions_dir: PathBuf,
    /// Currently active session file
    current_session: Option<PathBuf>,
    /// Session ID
    session_id: Option<String>,
    /// Last entry ID for parent threading
    last_entry_id: Option<String>,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            current_session: None,
            session_id: None,
            last_entry_id: None,
        }
    }

    /// Create a new session
    pub async fn create_session(&mut self, cwd: &str) -> Result<String> {
        fs::create_dir_all(&self.sessions_dir).await?;
        let id = Uuid::new_v4().to_string();
        let filename = format!("{}.jsonl", &id[..8]);
        let path = self.sessions_dir.join(filename);
        self.initialize_session_file(cwd, path, id).await
    }

    /// Create a new session at an explicit file path.
    pub async fn create_session_at(&mut self, cwd: &str, path: &Path) -> Result<String> {
        self.initialize_session_file(cwd, path.to_path_buf(), Uuid::new_v4().to_string())
            .await
    }

    /// Load an existing session file and return the serialized conversation messages.
    pub async fn load_session(&mut self, path: &Path) -> Result<Vec<AgentMessage>> {
        let data = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        let mut lines = data.lines();
        let header_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Session file is empty: {}", path.display()))?;
        let header: SessionHeader = serde_json::from_str(header_line)
            .with_context(|| format!("Invalid session header in {}", path.display()))?;

        let mut messages = Vec::new();
        let mut last_entry_id = None;

        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let entry: SessionEntry = match serde_json::from_str(line) {
                Ok(entry) => entry,
                Err(_) => {
                    // Keep valid portions of the session usable even if some lines are malformed.
                    continue;
                }
            };
            let entry_id = entry.id().to_string();
            if let SessionEntry::Message { message, .. } = entry {
                messages.push(message);
            }
            last_entry_id = Some(entry_id);
        }

        self.current_session = Some(path.to_path_buf());
        self.session_id = Some(header.id);
        self.last_entry_id = last_entry_id;
        Ok(messages)
    }

    /// Open an existing session, or create/recover one at this path.
    ///
    /// Mirrors pi-mono behavior: if a file exists but cannot be parsed as a valid
    /// session, rewrite it with a fresh header so the CLI can continue.
    pub async fn open_or_create_session(
        &mut self,
        cwd: &str,
        path: &Path,
    ) -> Result<Vec<AgentMessage>> {
        if path.exists() {
            match self.load_session(path).await {
                Ok(messages) => Ok(messages),
                Err(_) => {
                    self.create_session_at(cwd, path).await?;
                    Ok(Vec::new())
                }
            }
        } else {
            self.create_session_at(cwd, path).await?;
            Ok(Vec::new())
        }
    }

    /// Append an agent conversation message as a session entry.
    pub async fn append_message(&mut self, message: AgentMessage) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry::Message {
            id: id.clone(),
            parent_id: self.last_entry_id.clone(),
            timestamp: Utc::now(),
            message,
        };
        self.append_entry(&entry).await?;
        Ok(id)
    }

    /// Append an entry to the current session
    pub async fn append_entry(&mut self, entry: &SessionEntry) -> Result<()> {
        let path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?;

        let json = serde_json::to_string(entry)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        self.last_entry_id = Some(entry.id().to_string());
        Ok(())
    }

    /// List available sessions (returns session IDs and paths), oldest first.
    pub async fn list_sessions(&self) -> Result<Vec<(String, PathBuf)>> {
        let mut sessions: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }

        let mut dir = fs::read_dir(&self.sessions_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Ok(content) = fs::read_to_string(&path).await {
                    if let Some(first_line) = content.lines().next() {
                        if let Ok(header) = serde_json::from_str::<SessionHeader>(first_line) {
                            let modified = fs::metadata(&path)
                                .await
                                .ok()
                                .and_then(|meta| meta.modified().ok())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            sessions.push((modified, header.id, path));
                        }
                    }
                }
            }
        }
        sessions.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(sessions
            .into_iter()
            .map(|(_, id, path)| (id, path))
            .collect())
    }

    /// Get current session ID
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Get current session path
    pub fn session_path(&self) -> Option<&Path> {
        self.current_session.as_deref()
    }

    /// Branch the session to a specific entry ID.
    ///
    /// All entries after `branch_from_id` are abandoned (but remain in the file).
    /// New entries will be appended with `branch_from_id` as their parent.
    pub fn branch(&mut self, branch_from_id: &str) {
        self.last_entry_id = Some(branch_from_id.to_string());
    }

    /// Fork the session: create a new session file containing entries up to `fork_from_id`.
    ///
    /// Returns the path to the new session file.
    pub async fn fork(&mut self, fork_from_id: &str, cwd: &str) -> Result<PathBuf> {
        let current_path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?
            .clone();

        let content = tokio::fs::read_to_string(&current_path).await?;
        let lines: Vec<&str> = content.lines().collect();

        if lines.is_empty() {
            return Err(anyhow::anyhow!("Session file is empty"));
        }

        // Collect lines up to and including the fork point
        let mut fork_lines = Vec::new();

        // Always include the header (first line)
        fork_lines.push(lines[0].to_string());

        let mut found = false;
        for line in &lines[1..] {
            if line.trim().is_empty() {
                continue;
            }
            fork_lines.push(line.to_string());
            // Check if this entry has the target ID
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                if entry.id() == fork_from_id {
                    found = true;
                    break;
                }
            }
        }

        if !found {
            return Err(anyhow::anyhow!(
                "Entry ID '{}' not found in session",
                fork_from_id
            ));
        }

        // Create a new session file (this sets current_session to the new path)
        self.create_session(cwd).await?;
        let new_session_path = self.current_session.as_ref().unwrap().clone();

        // Rewrite the header to point back to the parent session
        let mut header: serde_json::Value = serde_json::from_str(&fork_lines[0])?;
        if let Some(obj) = header.as_object_mut() {
            obj.insert(
                "parent_session".to_string(),
                serde_json::Value::String(current_path.display().to_string()),
            );
            // Keep the new session ID
            obj.insert(
                "id".to_string(),
                serde_json::Value::String(
                    self.session_id.as_ref().unwrap().clone(),
                ),
            );
        }
        fork_lines[0] = serde_json::to_string(&header)?;

        let file_content = fork_lines.join("\n") + "\n";
        tokio::fs::write(&new_session_path, file_content).await?;

        // Set last_entry_id to fork point so new entries chain from there
        self.last_entry_id = Some(fork_from_id.to_string());

        Ok(new_session_path)
    }

    async fn initialize_session_file(
        &mut self,
        cwd: &str,
        path: PathBuf,
        id: String,
    ) -> Result<String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let header = SessionHeader::new(id.clone(), cwd.to_string());
        let mut file = fs::File::create(&path).await?;
        let header_json = serde_json::to_string(&header)?;
        file.write_all(header_json.as_bytes()).await?;
        file.write_all(b"\n").await?;

        self.current_session = Some(path);
        self.session_id = Some(id.clone());
        self.last_entry_id = None;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use pi_ai::Message;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pi-rs-session-test-{name}-{}", Uuid::new_v4()))
    }

    fn message_entry_id(entry: &SessionEntry) -> Option<&str> {
        match entry {
            SessionEntry::Message { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    async fn read_lines(path: &Path) -> Vec<String> {
        let content = fs::read_to_string(path).expect("session file should exist");
        content.lines().map(|s| s.to_string()).collect()
    }

    fn parse_header(line: &str) -> SessionHeader {
        serde_json::from_str(line).expect("valid session header json")
    }

    fn parse_entry(line: &str) -> SessionEntry {
        serde_json::from_str(line).expect("valid session entry json")
    }

    #[tokio::test]
    async fn create_session_writes_header() {
        let dir = temp_dir("create");
        let mut manager = SessionManager::new(dir.clone());

        let id = manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        let path = manager
            .session_path()
            .expect("session path set")
            .to_path_buf();
        let lines = read_lines(&path).await;

        assert_eq!(lines.len(), 1);
        let header = parse_header(&lines[0]);
        assert_eq!(header.entry_type, "session");
        assert_eq!(header.version, 3);
        assert_eq!(header.id, id);
        assert_eq!(header.cwd, "/tmp/work");
        assert_eq!(manager.session_id(), Some(id.as_str()));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn create_session_at_uses_explicit_path() {
        let dir = temp_dir("create-at");
        let path = dir.join("nested").join("custom.jsonl");
        let mut manager = SessionManager::new(dir.clone());

        manager
            .create_session_at("/tmp/work", &path)
            .await
            .expect("session created");

        assert_eq!(manager.session_path(), Some(path.as_path()));
        assert!(path.exists());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn append_message_sets_parent_chain() {
        let dir = temp_dir("append");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        let path = manager
            .session_path()
            .expect("session path set")
            .to_path_buf();

        manager
            .append_message(AgentMessage::from_llm(Message::user("first")))
            .await
            .expect("first message");
        manager
            .append_message(AgentMessage::from_llm(Message::user("second")))
            .await
            .expect("second message");

        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 3);
        let first = parse_entry(&lines[1]);
        let second = parse_entry(&lines[2]);

        let first_id = message_entry_id(&first)
            .expect("first message id")
            .to_string();
        match &first {
            SessionEntry::Message { parent_id, .. } => assert!(parent_id.is_none()),
            _ => panic!("expected message"),
        }
        match second {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some(first_id.as_str()))
            }
            _ => panic!("expected message"),
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn load_session_restores_messages_and_uses_last_entry_for_parent() {
        let dir = temp_dir("load");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("session.jsonl");
        let header = SessionHeader {
            entry_type: "session".to_string(),
            version: 3,
            id: "sess-1".to_string(),
            timestamp: Utc::now(),
            cwd: "/tmp/work".to_string(),
            parent_session: None,
        };
        let msg_entry = SessionEntry::Message {
            id: "m1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("hello")),
        };
        let model_entry = SessionEntry::ModelChange {
            id: "mc1".to_string(),
            parent_id: Some("m1".to_string()),
            timestamp: Utc::now(),
            model: "model-a".to_string(),
            provider: "provider-a".to_string(),
        };
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&header).expect("header json"),
            serde_json::to_string(&msg_entry).expect("message json"),
            serde_json::to_string(&model_entry).expect("model json")
        );
        fs::write(&path, content).expect("write session file");

        let mut manager = SessionManager::new(dir.clone());
        let restored = manager.load_session(&path).await.expect("load session");
        assert_eq!(restored.len(), 1);

        manager
            .append_message(AgentMessage::from_llm(Message::user("next")))
            .await
            .expect("append after load");
        let lines = read_lines(&path).await;
        let appended = parse_entry(lines.last().expect("new appended line"));
        match appended {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some("mc1"));
            }
            _ => panic!("expected message"),
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn list_sessions_orders_by_file_mtime() {
        let dir = temp_dir("list");
        fs::create_dir_all(&dir).expect("create temp dir");
        let old_path = dir.join("old.jsonl");
        let new_path = dir.join("new.jsonl");

        // Intentionally invert header timestamps so ordering must come from mtime.
        let old_ts = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .expect("valid old ts")
            .with_timezone(&Utc);
        let new_ts = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .expect("valid new ts")
            .with_timezone(&Utc);

        let old_header = SessionHeader {
            entry_type: "session".to_string(),
            version: 3,
            id: "old-id".to_string(),
            timestamp: old_ts,
            cwd: "/tmp/work".to_string(),
            parent_session: None,
        };
        let new_header = SessionHeader {
            entry_type: "session".to_string(),
            version: 3,
            id: "new-id".to_string(),
            timestamp: new_ts,
            cwd: "/tmp/work".to_string(),
            parent_session: None,
        };

        fs::write(
            &old_path,
            format!(
                "{}\n",
                serde_json::to_string(&old_header).expect("old json")
            ),
        )
        .expect("write old file");
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(
            &new_path,
            format!(
                "{}\n",
                serde_json::to_string(&new_header).expect("new json")
            ),
        )
        .expect("write new file");

        let manager = SessionManager::new(dir.clone());
        let listed = manager.list_sessions().await.expect("list sessions");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].0, "old-id");
        assert_eq!(listed[1].0, "new-id");

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn load_session_errors_on_empty_file() {
        let dir = temp_dir("empty");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("empty.jsonl");
        fs::write(&path, "").expect("write empty file");

        let mut manager = SessionManager::new(dir.clone());
        let err = manager
            .load_session(&path)
            .await
            .expect_err("empty file should fail");
        let msg = err.to_string();
        assert!(msg.contains("Session file is empty"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn load_session_errors_on_invalid_header() {
        let dir = temp_dir("invalid-header");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("bad-header.jsonl");
        fs::write(&path, "{\"type\":\"message\"}\n").expect("write invalid header");

        let mut manager = SessionManager::new(dir.clone());
        let err = manager
            .load_session(&path)
            .await
            .expect_err("invalid header should fail");
        assert!(err.to_string().contains("Invalid session header"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn load_session_skips_malformed_entries_and_keeps_valid_ones() {
        let dir = temp_dir("skip-invalid-entry");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("mixed.jsonl");
        let header = SessionHeader::new("sess-1".to_string(), "/tmp/work".to_string());
        let valid = SessionEntry::Message {
            id: "m1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("hello")),
        };
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&header).expect("header json"),
            "not-json",
            serde_json::to_string(&valid).expect("valid entry json")
        );
        fs::write(&path, content).expect("write file");

        let mut manager = SessionManager::new(dir.clone());
        let restored = manager.load_session(&path).await.expect("load mixed file");
        assert_eq!(restored.len(), 1);

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn append_entry_requires_active_session() {
        let dir = temp_dir("no-active");
        let mut manager = SessionManager::new(dir.clone());
        let entry = SessionEntry::Label {
            id: "label-1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            label: "bookmark".to_string(),
        };

        let err = manager
            .append_entry(&entry)
            .await
            .expect_err("append without active session");
        assert!(err.to_string().contains("No active session"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn open_or_create_creates_missing_file() {
        let dir = temp_dir("open-create");
        let path = dir.join("new-session.jsonl");
        let mut manager = SessionManager::new(dir.clone());

        let restored = manager
            .open_or_create_session("/tmp/work", &path)
            .await
            .expect("open or create");
        assert!(restored.is_empty());
        assert!(path.exists());

        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 1);
        let header = parse_header(&lines[0]);
        assert_eq!(header.entry_type, "session");

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn open_or_create_recovers_corrupted_existing_file() {
        let dir = temp_dir("recover");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("corrupt.jsonl");
        fs::write(&path, "garbage\n").expect("write corrupt file");
        let mut manager = SessionManager::new(dir.clone());

        let restored = manager
            .open_or_create_session("/tmp/work", &path)
            .await
            .expect("recover corrupt file");
        assert!(restored.is_empty());

        let lines = read_lines(&path).await;
        assert_eq!(lines.len(), 1);
        let header = parse_header(&lines[0]);
        assert_eq!(header.entry_type, "session");
        assert_eq!(header.cwd, "/tmp/work");

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn branch_sets_last_entry_id() {
        let dir = temp_dir("branch");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        let path = manager
            .session_path()
            .expect("session path set")
            .to_path_buf();

        // Append three messages
        let id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("first")))
            .await
            .expect("first message");
        let _id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("second")))
            .await
            .expect("second message");
        let _id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("third")))
            .await
            .expect("third message");

        // Branch back to the first message
        manager.branch(&id1);

        // The next appended message should have id1 as its parent
        manager
            .append_message(AgentMessage::from_llm(Message::user("branched")))
            .await
            .expect("branched message");

        let lines = read_lines(&path).await;
        // header + 3 original + 1 branched = 5
        assert_eq!(lines.len(), 5);
        let branched_entry = parse_entry(lines.last().expect("last line"));
        match branched_entry {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some(id1.as_str()));
            }
            _ => panic!("expected message"),
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn fork_creates_new_session_up_to_fork_point() {
        let dir = temp_dir("fork");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        let original_path = manager
            .session_path()
            .expect("session path set")
            .to_path_buf();

        // Append three messages
        let _id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("first")))
            .await
            .expect("first message");
        let id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("second")))
            .await
            .expect("second message");
        let _id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("third")))
            .await
            .expect("third message");

        // Fork at the second message
        let forked_path = manager
            .fork(&id2, "/tmp/work")
            .await
            .expect("fork session");

        // The forked session file should exist and differ from the original
        assert!(forked_path.exists());
        assert_ne!(forked_path, original_path);

        // The forked file should contain header + 2 entries (up to id2)
        let forked_lines = read_lines(&forked_path).await;
        assert_eq!(forked_lines.len(), 3); // header + 2 messages

        // The header should reference the parent session
        let forked_header = parse_header(&forked_lines[0]);
        assert_eq!(
            forked_header.parent_session.as_deref(),
            Some(original_path.to_str().unwrap())
        );

        // The current session should now point to the forked path
        assert_eq!(manager.session_path(), Some(forked_path.as_path()));

        // Appending after fork should chain from id2
        manager
            .append_message(AgentMessage::from_llm(Message::user("forked-new")))
            .await
            .expect("append after fork");
        let updated_lines = read_lines(&forked_path).await;
        let last_entry = parse_entry(updated_lines.last().expect("last line"));
        match last_entry {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some(id2.as_str()));
            }
            _ => panic!("expected message"),
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn fork_errors_on_missing_entry_id() {
        let dir = temp_dir("fork-missing");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        manager
            .append_message(AgentMessage::from_llm(Message::user("hello")))
            .await
            .expect("append message");

        let err = manager
            .fork("nonexistent-id", "/tmp/work")
            .await
            .expect_err("fork with missing ID should fail");
        assert!(err.to_string().contains("not found in session"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn fork_errors_without_active_session() {
        let dir = temp_dir("fork-no-session");
        let mut manager = SessionManager::new(dir.clone());

        let err = manager
            .fork("some-id", "/tmp/work")
            .await
            .expect_err("fork without session should fail");
        assert!(err.to_string().contains("No active session"));

        fs::remove_dir_all(dir).ok();
    }
}
