use anyhow::{Context, Result};
use chrono::Utc;
use pi_agent_core::context::compaction::{
    build_branch_summary_prompt, estimate_tokens_str, serialize_conversation,
    BranchSummarizationSettings,
};
use pi_agent_core::messages::{AgentMessage, to_llm_messages};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::lock::SessionLock;
use super::persistence::{SessionEntry, SessionHeader};

/// A node in the session entry tree.
///
/// Each entry in the JSONL file forms a DAG (typically a tree) via parent_id links.
/// `TreeNode` surfaces this structure for navigation and display.
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// The unique ID of this entry.
    pub entry_id: String,
    /// The ID of the parent entry, or `None` for root entries.
    pub parent_id: Option<String>,
    /// IDs of all direct children of this node.
    pub children: Vec<String>,
    /// The discriminant of the underlying `SessionEntry` variant, e.g. `"message"`.
    pub entry_type: String,
    /// A short human-readable description of the entry for display purposes.
    pub summary: String,
}

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
    /// Advisory file lock held for the lifetime of the active session.
    ///
    /// Acquiring the lock prevents a second process (or a second
    /// `SessionManager` in the same process) from opening the same session
    /// file concurrently and corrupting it with interleaved writes.
    active_lock: Option<SessionLock>,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            current_session: None,
            session_id: None,
            last_entry_id: None,
            active_lock: None,
        }
    }

    /// Default timeout used when trying to acquire a session lock.
    const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

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

        // Release any lock held from a previous session before acquiring a
        // new one.  This ensures we never hold two locks simultaneously (which
        // could deadlock if the manager is reused) and prevents a WouldBlock
        // error when opening the same path that was previously loaded.
        self.active_lock = None;

        // Acquire an exclusive advisory lock before committing the state
        // change.  This prevents a second SessionManager (in another process
        // or task) from opening the same file concurrently.
        let lock = SessionLock::acquire_with_timeout(path, Self::LOCK_TIMEOUT)
            .with_context(|| {
                format!(
                    "Could not acquire lock for session file '{}'. \
                     Another pi process may already have this session open.",
                    path.display()
                )
            })?;

        self.current_session = Some(path.to_path_buf());
        self.session_id = Some(header.id);
        self.last_entry_id = last_entry_id;
        self.active_lock = Some(lock);
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
                serde_json::Value::String(self.session_id.as_ref().unwrap().clone()),
            );
        }
        fork_lines[0] = serde_json::to_string(&header)?;

        let file_content = fork_lines.join("\n") + "\n";
        tokio::fs::write(&new_session_path, file_content).await?;

        // Set last_entry_id to fork point so new entries chain from there
        self.last_entry_id = Some(fork_from_id.to_string());

        Ok(new_session_path)
    }

    // -----------------------------------------------------------------------
    // Tree navigation
    // -----------------------------------------------------------------------

    /// Read all entries from the active session file and return them as a flat
    /// list of [`TreeNode`]s with `children` already populated.
    ///
    /// The ordering of the returned `Vec` mirrors the on-disk order (i.e. the
    /// order entries were appended), so the root node(s) appear near the front.
    pub async fn get_tree(&self) -> Result<Vec<TreeNode>> {
        let path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?;

        let data = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        // Pass 1: parse every non-header line into (id, parent_id, entry_type, summary).
        let mut raw: Vec<(String, Option<String>, String, String)> = Vec::new();
        for (i, line) in data.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                // Skip the session header and blank lines.
                continue;
            }
            let entry: SessionEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let (entry_type, parent_id, summary) = Self::describe_entry(&entry);
            raw.push((entry.id().to_string(), parent_id, entry_type, summary));
        }

        // Pass 2: build an id → children index.
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
        for (id, parent_id, _, _) in &raw {
            // Ensure every node has an entry (even if it ends up with no children).
            children_map.entry(id.clone()).or_default();
            if let Some(pid) = parent_id {
                children_map
                    .entry(pid.clone())
                    .or_default()
                    .push(id.clone());
            }
        }

        // Pass 3: assemble TreeNode list in original order.
        let nodes = raw
            .into_iter()
            .map(|(id, parent_id, entry_type, summary)| {
                let children = children_map.get(&id).cloned().unwrap_or_default();
                TreeNode {
                    entry_id: id,
                    parent_id,
                    children,
                    entry_type,
                    summary,
                }
            })
            .collect();

        Ok(nodes)
    }

    /// Navigate to a specific entry by its ID.
    ///
    /// Traces the ancestor chain from `entry_id` back to the root, then returns
    /// the `AgentMessage` payloads for every `Message` entry along that path in
    /// root-first order.  This sequence can be used directly as the conversation
    /// context to restore when resuming at `entry_id`.
    ///
    /// Also updates `last_entry_id` so that subsequent [`append_message`] calls
    /// chain from the target entry.
    pub async fn navigate_to(&mut self, entry_id: &str) -> Result<Vec<AgentMessage>> {
        let path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?
            .clone();

        let data = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        // Build two maps: id → entry  and  id → parent_id.
        let mut entry_map: HashMap<String, SessionEntry> = HashMap::new();
        let mut parent_of: HashMap<String, Option<String>> = HashMap::new();

        for (i, line) in data.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                continue;
            }
            let entry: SessionEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let id = entry.id().to_string();
            let parent_id = Self::entry_parent_id(&entry);
            parent_of.insert(id.clone(), parent_id);
            entry_map.insert(id, entry);
        }

        // Verify the target entry actually exists.
        if !entry_map.contains_key(entry_id) {
            return Err(anyhow::anyhow!(
                "Entry ID '{}' not found in session",
                entry_id
            ));
        }

        // Walk the ancestor chain from entry_id up to the root.
        let mut path_ids: Vec<String> = Vec::new();
        let mut cursor = Some(entry_id.to_string());
        while let Some(current) = cursor {
            path_ids.push(current.clone());
            cursor = parent_of.get(&current).and_then(|p| p.clone());
        }
        path_ids.reverse(); // root-first

        // Collect AgentMessages for Message entries along the path.
        let messages: Vec<AgentMessage> = path_ids
            .iter()
            .filter_map(|id| {
                entry_map.get(id).and_then(|entry| match entry {
                    SessionEntry::Message { message, .. } => Some(message.clone()),
                    _ => None,
                })
            })
            .collect();

        // Update the manager state so new entries continue from here.
        self.last_entry_id = Some(entry_id.to_string());

        Ok(messages)
    }

    /// Return all nodes that are branch points — entries with more than one child.
    ///
    /// Branch points are the positions in the tree where the conversation
    /// diverged; useful for presenting navigation choices to the user.
    pub async fn get_branch_points(&self) -> Result<Vec<TreeNode>> {
        let all_nodes = self.get_tree().await?;
        Ok(all_nodes
            .into_iter()
            .filter(|node| node.children.len() > 1)
            .collect())
    }

    // -----------------------------------------------------------------------
    // Branch summarization
    // -----------------------------------------------------------------------

    /// Collect the `AgentMessage`s on the ancestor path up to `entry_id` and
    /// return the `(system_prompt, user_prompt)` pair that an LLM should use
    /// to produce a branch-point summary.
    ///
    /// This method deliberately does **not** call the LLM itself; it only
    /// builds the prompt so that callers can decide which provider/model to
    /// use.  The returned prompts are produced by
    /// [`build_branch_summary_prompt`] and therefore follow the branch-summary
    /// structured format.
    ///
    /// # Errors
    /// Returns an error if no session is active or if `entry_id` does not
    /// exist in the current session file.
    pub async fn build_branch_summary_prompts(
        &self,
        entry_id: &str,
        _settings: &BranchSummarizationSettings,
    ) -> Result<(String, String)> {
        let messages = self.collect_messages_up_to(entry_id).await?;
        let llm_messages = to_llm_messages(&messages);
        let messages_text = serialize_conversation(&llm_messages);
        Ok(build_branch_summary_prompt(&messages_text))
    }

    /// Collect the conversation messages on the path from the root up to
    /// `entry_id` and return them serialized as a human-readable string
    /// suitable for passing to a summarization LLM.
    ///
    /// This is useful when you want the raw text rather than the prompt pair,
    /// e.g. for token estimation before deciding whether to summarize.
    ///
    /// # Errors
    /// Returns an error if no session is active or if `entry_id` does not
    /// exist in the current session file.
    pub async fn collect_messages_up_to(&self, entry_id: &str) -> Result<Vec<AgentMessage>> {
        let path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?;

        let data = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        // Build id → entry and id → parent_id maps.
        let mut entry_map: HashMap<String, SessionEntry> = HashMap::new();
        let mut parent_of: HashMap<String, Option<String>> = HashMap::new();

        for (i, line) in data.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                continue;
            }
            let entry: SessionEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let id = entry.id().to_string();
            let parent_id = Self::entry_parent_id(&entry);
            parent_of.insert(id.clone(), parent_id);
            entry_map.insert(id, entry);
        }

        if !entry_map.contains_key(entry_id) {
            return Err(anyhow::anyhow!(
                "Entry ID '{}' not found in session",
                entry_id
            ));
        }

        // Trace ancestor chain from entry_id to root.
        let mut path_ids: Vec<String> = Vec::new();
        let mut cursor = Some(entry_id.to_string());
        while let Some(current) = cursor {
            path_ids.push(current.clone());
            cursor = parent_of.get(&current).and_then(|p| p.clone());
        }
        path_ids.reverse(); // root-first

        // Collect AgentMessage payloads for Message entries on the path.
        let messages: Vec<AgentMessage> = path_ids
            .iter()
            .filter_map(|id| {
                entry_map.get(id).and_then(|entry| match entry {
                    SessionEntry::Message { message, .. } => Some(message.clone()),
                    _ => None,
                })
            })
            .collect();

        Ok(messages)
    }

    /// Serialize the messages up to `entry_id` and return their estimated
    /// token count.  Useful for deciding whether branch summarization is
    /// warranted before actually building the prompt.
    pub async fn estimate_branch_tokens(&self, entry_id: &str) -> Result<u64> {
        let messages = self.collect_messages_up_to(entry_id).await?;
        let llm_messages = to_llm_messages(&messages);
        let text = serialize_conversation(&llm_messages);
        Ok(estimate_tokens_str(&text))
    }

    /// Persist a pre-computed branch summary as a [`SessionEntry::BranchSummary`]
    /// in the active session file.
    ///
    /// Call this after receiving the LLM response for the prompts produced by
    /// [`build_branch_summary_prompts`].  The `tokens_before` value should be
    /// the token estimate for the messages that were summarized, obtainable via
    /// [`estimate_branch_tokens`].
    ///
    /// Returns the entry ID of the newly appended `BranchSummary` entry.
    pub async fn append_branch_summary(
        &mut self,
        branch_entry_id: &str,
        summary: String,
        tokens_before: u64,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let entry = SessionEntry::BranchSummary {
            id: id.clone(),
            branch_entry_id: branch_entry_id.to_string(),
            parent_id: self.last_entry_id.clone(),
            timestamp: Utc::now(),
            summary,
            tokens_before,
        };
        self.append_entry(&entry).await?;
        Ok(id)
    }

    // -----------------------------------------------------------------------
    // Session merging
    // -----------------------------------------------------------------------

    /// Merge another session into the current session.
    ///
    /// All entries from `source_path` are appended to the current session,
    /// with their parent IDs adjusted to maintain the tree structure.
    /// Entries are re-parented to chain from the current session's last entry.
    ///
    /// Returns the number of entries merged.
    pub async fn merge(&mut self, source_path: &Path) -> Result<usize> {
        let current_path = self
            .current_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active session"))?
            .clone();

        // Read source session
        let source_data = fs::read_to_string(source_path)
            .await
            .with_context(|| format!("Failed to read source session: {}", source_path.display()))?;

        let source_lines: Vec<&str> = source_data.lines().collect();
        if source_lines.is_empty() {
            return Ok(0);
        }

        // Parse source header to validate it's a session file
        let _: SessionHeader = serde_json::from_str(source_lines[0])
            .with_context(|| "Source file is not a valid session")?;

        // Build a map of old ID -> new ID for all entries we're merging
        let mut id_remap: HashMap<String, String> = HashMap::new();
        let mut entries_to_merge: Vec<SessionEntry> = Vec::new();

        // First pass: collect all entries and assign new IDs
        for line in &source_lines[1..] {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                let old_id = entry.id().to_string();
                let new_id = Uuid::new_v4().to_string();
                id_remap.insert(old_id, new_id.clone());
                entries_to_merge.push(entry);
            }
        }

        if entries_to_merge.is_empty() {
            return Ok(0);
        }

        // Get the current last entry ID to use as parent for the first merged entry
        let current_parent_id = self.last_entry_id.clone();

        // Second pass: remap parent IDs and write entries
        let mut count = 0;
        let mut previous_new_id: Option<String> = current_parent_id;

        for entry in entries_to_merge {
            let old_id = entry.id().to_string();
            let new_id = id_remap.get(&old_id).cloned().unwrap_or_else(|| Uuid::new_v4().to_string());

            // Remap the entry with new IDs
            let remapped_entry = self.remap_entry_ids(entry, &new_id, &previous_new_id, &id_remap);
            
            self.append_entry(&remapped_entry).await?;
            previous_new_id = Some(new_id);
            count += 1;
        }

        Ok(count)
    }

    /// Remap entry IDs for merging.
    fn remap_entry_ids(
        &self,
        entry: SessionEntry,
        new_id: &str,
        new_parent_id: &Option<String>,
        id_remap: &HashMap<String, String>,
    ) -> SessionEntry {
        use SessionEntry::*;

        match entry {
            Message { message, timestamp, .. } => Message {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                message,
            },
            Compaction { summary, first_kept_entry_id, tokens_before, timestamp, .. } => {
                // Remap the first_kept_entry_id if it was in the merged session
                let remapped_first_kept = id_remap.get(&first_kept_entry_id).cloned();
                Compaction {
                    id: new_id.to_string(),
                    parent_id: new_parent_id.clone(),
                    timestamp,
                    summary,
                    first_kept_entry_id: remapped_first_kept.unwrap_or(first_kept_entry_id),
                    tokens_before,
                }
            }
            ModelChange { model, provider, timestamp, .. } => ModelChange {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                model,
                provider,
            },
            ThinkingLevelChange { level, timestamp, .. } => ThinkingLevelChange {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                level,
            },
            Label { label, timestamp, .. } => Label {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                label,
            },
            BranchSummary { branch_entry_id, summary, tokens_before, timestamp, .. } => {
                // Remap the branch_entry_id if it was in the merged session
                let remapped_branch = id_remap.get(&branch_entry_id).cloned();
                BranchSummary {
                    id: new_id.to_string(),
                    branch_entry_id: remapped_branch.unwrap_or(branch_entry_id),
                    parent_id: new_parent_id.clone(),
                    timestamp,
                    summary,
                    tokens_before,
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Schema migrations
    // -----------------------------------------------------------------------

    /// Migrate a session file from an older schema version to the current version (3).
    ///
    /// Returns true if migration was performed, false if already at current version.
    pub async fn migrate_session(path: &Path) -> Result<bool> {
        let data = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        let lines: Vec<&str> = data.lines().collect();
        if lines.is_empty() {
            anyhow::bail!("Session file is empty");
        }

        // Parse header to check version
        let header: serde_json::Value = serde_json::from_str(lines[0])
            .with_context(|| "Invalid session header")?;

        let version = header.get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        if version >= 3 {
            return Ok(false); // Already at current version
        }

        // Perform migration
        let mut migrated_lines = Vec::new();
        
        // Migrate header
        let mut new_header = header.clone();
        if let Some(obj) = new_header.as_object_mut() {
            obj.insert("version".to_string(), serde_json::json!(3));
            // Ensure entry_type is set
            if !obj.contains_key("type") {
                obj.insert("type".to_string(), serde_json::json!("session"));
            }
        }
        migrated_lines.push(serde_json::to_string(&new_header)?);

        // Migrate entries based on version
        for line in &lines[1..] {
            if line.trim().is_empty() {
                migrated_lines.push(line.to_string());
                continue;
            }

            let mut entry: serde_json::Value = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => {
                    // Keep malformed lines as-is
                    migrated_lines.push(line.to_string());
                    continue;
                }
            };

            // Version-specific migrations
            if version < 2 {
                // v1 -> v2: Ensure all entries have an 'id' field
                if let Some(obj) = entry.as_object_mut() {
                    if !obj.contains_key("id") {
                        obj.insert("id".to_string(), serde_json::json!(Uuid::new_v4().to_string()));
                    }
                    // Ensure 'type' field exists
                    if !obj.contains_key("type") {
                        // Infer type from structure or default to message
                        let entry_type = if obj.contains_key("message") {
                            "message"
                        } else if obj.contains_key("summary") && obj.contains_key("first_kept_entry_id") {
                            "compaction"
                        } else if obj.contains_key("model") {
                            "model_change"
                        } else {
                            "message"
                        };
                        obj.insert("type".to_string(), serde_json::json!(entry_type));
                    }
                }
            }

            if version < 3 {
                // v2 -> v3: Add timestamp if missing
                if let Some(obj) = entry.as_object_mut() {
                    if !obj.contains_key("timestamp") {
                        obj.insert("timestamp".to_string(), serde_json::json!(Utc::now()));
                    }
                    // Ensure parent_id field exists (can be null)
                    if !obj.contains_key("parent_id") {
                        obj.insert("parent_id".to_string(), serde_json::Value::Null);
                    }
                }
            }

            migrated_lines.push(serde_json::to_string(&entry)?);
        }

        // Write migrated file atomically
        let temp_path = path.with_extension("tmp");
        let mut temp_file = fs::File::create(&temp_path).await?;
        for line in migrated_lines {
            temp_file.write_all(line.as_bytes()).await?;
            temp_file.write_all(b"\n").await?;
        }
        drop(temp_file);

        // Atomically replace original
        fs::rename(&temp_path, path).await?;

        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Extract (entry_type, parent_id, summary) from a `SessionEntry`.
    fn describe_entry(entry: &SessionEntry) -> (String, Option<String>, String) {
        match entry {
            SessionEntry::Message {
                parent_id, message, ..
            } => {
                let summary = match message {
                    AgentMessage::Llm(msg) => {
                        let text = msg.text_content();
                        let truncated = if text.chars().count() > 80 {
                            let t: String = text.chars().take(80).collect();
                            format!("{}...", t)
                        } else {
                            text
                        };
                        let role = if msg.is_user() {
                            "user"
                        } else if msg.is_assistant() {
                            "assistant"
                        } else {
                            "tool_result"
                        };
                        format!("[{}] {}", role, truncated)
                    }
                    AgentMessage::SystemContext { content, source } => {
                        format!("[system:{source}] {}", Self::truncate(content, 60))
                    }
                    AgentMessage::CompactionSummary { summary, .. } => {
                        format!("[compaction] {}", Self::truncate(summary, 60))
                    }
                    AgentMessage::Extension { type_name, .. } => {
                        format!("[extension:{type_name}]")
                    }
                };
                ("message".to_string(), parent_id.clone(), summary)
            }
            SessionEntry::Compaction {
                parent_id, summary, ..
            } => (
                "compaction".to_string(),
                parent_id.clone(),
                format!("[compaction] {}", Self::truncate(summary, 60)),
            ),
            SessionEntry::ModelChange {
                parent_id,
                model,
                provider,
                ..
            } => (
                "model_change".to_string(),
                parent_id.clone(),
                format!("[model] {provider}/{model}"),
            ),
            SessionEntry::ThinkingLevelChange {
                parent_id, level, ..
            } => (
                "thinking_level_change".to_string(),
                parent_id.clone(),
                format!("[thinking] level={level}"),
            ),
            SessionEntry::Label {
                parent_id, label, ..
            } => (
                "label".to_string(),
                parent_id.clone(),
                format!("[label] {label}"),
            ),
            SessionEntry::BranchSummary {
                parent_id,
                summary,
                branch_entry_id,
                ..
            } => (
                "branch_summary".to_string(),
                parent_id.clone(),
                format!(
                    "[branch_summary@{}] {}",
                    &branch_entry_id[..branch_entry_id.len().min(8)],
                    Self::truncate(summary, 50)
                ),
            ),
        }
    }

    /// Extract the `parent_id` from any `SessionEntry` variant.
    fn entry_parent_id(entry: &SessionEntry) -> Option<String> {
        match entry {
            SessionEntry::Message { parent_id, .. }
            | SessionEntry::Compaction { parent_id, .. }
            | SessionEntry::ModelChange { parent_id, .. }
            | SessionEntry::ThinkingLevelChange { parent_id, .. }
            | SessionEntry::Label { parent_id, .. }
            | SessionEntry::BranchSummary { parent_id, .. } => parent_id.clone(),
        }
    }

    /// Truncate a string to at most `max_chars` characters, appending `...` if cut.
    fn truncate(s: &str, max_chars: usize) -> String {
        if s.chars().count() > max_chars {
            let truncated: String = s.chars().take(max_chars).collect();
            format!("{}...", truncated)
        } else {
            s.to_string()
        }
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
        // Flush the file to disk before acquiring the lock so that any
        // concurrent reader sees a valid header immediately.
        drop(file);

        // Acquire an exclusive advisory lock on the newly-created session file.
        // This prevents another SessionManager (in a different process or the
        // same process) from opening the same file for writing while this
        // manager is active.  We drop any previously held lock first so that
        // re-using a manager for multiple successive sessions does not
        // deadlock on itself.
        self.active_lock = None; // release previous lock if any
        let lock = SessionLock::acquire_with_timeout(&path, Self::LOCK_TIMEOUT)
            .with_context(|| {
                format!(
                    "Could not acquire lock for new session file '{}'.",
                    path.display()
                )
            })?;

        self.current_session = Some(path);
        self.session_id = Some(id.clone());
        self.last_entry_id = None;
        self.active_lock = Some(lock);
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
        let forked_path = manager.fork(&id2, "/tmp/work").await.expect("fork session");

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

    // -----------------------------------------------------------------------
    // Tree navigation tests
    // -----------------------------------------------------------------------

    /// Verify that `get_tree` correctly builds parent→child relationships for a
    /// simple linear chain: root → m1 → m2 → m3.
    #[tokio::test]
    async fn get_tree_builds_linear_chain() {
        let dir = temp_dir("tree-linear");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        let id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("first")))
            .await
            .expect("first");
        let id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("second")))
            .await
            .expect("second");
        let id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("third")))
            .await
            .expect("third");

        let tree = manager.get_tree().await.expect("get_tree");

        assert_eq!(tree.len(), 3);

        let node1 = tree.iter().find(|n| n.entry_id == id1).expect("node1");
        let node2 = tree.iter().find(|n| n.entry_id == id2).expect("node2");
        let node3 = tree.iter().find(|n| n.entry_id == id3).expect("node3");

        // Parentage
        assert!(node1.parent_id.is_none(), "root has no parent");
        assert_eq!(node2.parent_id.as_deref(), Some(id1.as_str()));
        assert_eq!(node3.parent_id.as_deref(), Some(id2.as_str()));

        // Children
        assert_eq!(node1.children, vec![id2.clone()]);
        assert_eq!(node2.children, vec![id3.clone()]);
        assert!(node3.children.is_empty(), "leaf has no children");

        // Entry types
        assert_eq!(node1.entry_type, "message");
        assert_eq!(node2.entry_type, "message");
        assert_eq!(node3.entry_type, "message");

        // Summaries contain role and content fragment
        assert!(node1.summary.contains("first"), "summary includes text");

        fs::remove_dir_all(dir).ok();
    }

    /// `navigate_to` on a leaf entry should return every message along the path
    /// from root → leaf in the correct (root-first) order.
    #[tokio::test]
    async fn navigate_to_leaf_returns_full_path() {
        let dir = temp_dir("nav-leaf");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        let _id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("alpha")))
            .await
            .expect("alpha");
        let _id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("beta")))
            .await
            .expect("beta");
        let id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("gamma")))
            .await
            .expect("gamma");

        let messages = manager.navigate_to(&id3).await.expect("navigate_to");

        // All three messages should be returned, root-first.
        assert_eq!(messages.len(), 3);
        let texts: Vec<String> = messages
            .iter()
            .filter_map(|m| m.as_llm())
            .map(|m| m.text_content())
            .collect();
        assert_eq!(texts, vec!["alpha", "beta", "gamma"]);

        // After navigation, last_entry_id must point to the target so new
        // appended messages chain from there.
        manager
            .append_message(AgentMessage::from_llm(Message::user("delta")))
            .await
            .expect("delta");

        // Read back and verify parent of the new entry.
        let path = manager.session_path().unwrap().to_path_buf();
        let content = std::fs::read_to_string(&path).expect("read session");
        let last_line = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .last()
            .expect("last line");
        let last_entry: SessionEntry =
            serde_json::from_str(last_line).expect("parse last entry");
        match last_entry {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some(id3.as_str()));
            }
            _ => panic!("expected message entry"),
        }

        fs::remove_dir_all(dir).ok();
    }

    /// `navigate_to` on an intermediate (branch-point) entry should return only
    /// the messages on the path up to that point, not beyond.
    #[tokio::test]
    async fn navigate_to_branch_point_returns_partial_path() {
        let dir = temp_dir("nav-branch");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        let id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("root")))
            .await
            .expect("root");
        let id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("branch-point")))
            .await
            .expect("branch-point");
        // Simulate a branch: go back to id2 and append a diverging child.
        manager.branch(&id2);
        let _id3b = manager
            .append_message(AgentMessage::from_llm(Message::user("branch-b")))
            .await
            .expect("branch-b");

        // Now navigate_to id2 (the branch point itself).
        let messages = manager.navigate_to(&id2).await.expect("navigate_to id2");

        assert_eq!(messages.len(), 2);
        let texts: Vec<String> = messages
            .iter()
            .filter_map(|m| m.as_llm())
            .map(|m| m.text_content())
            .collect();
        assert_eq!(texts, vec!["root", "branch-point"]);

        // Verify last_entry_id is now id2.
        let _ = id1; // used above
        manager
            .append_message(AgentMessage::from_llm(Message::user("after-nav")))
            .await
            .expect("after-nav");
        let path = manager.session_path().unwrap().to_path_buf();
        let content = std::fs::read_to_string(&path).expect("read session");
        let last_line = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .last()
            .expect("last line");
        let last_entry: SessionEntry =
            serde_json::from_str(last_line).expect("parse last entry");
        match last_entry {
            SessionEntry::Message { parent_id, .. } => {
                assert_eq!(parent_id.as_deref(), Some(id2.as_str()));
            }
            _ => panic!("expected message entry"),
        }

        fs::remove_dir_all(dir).ok();
    }

    /// `get_branch_points` returns only entries that have two or more children.
    #[tokio::test]
    async fn get_branch_points_finds_forked_entries() {
        let dir = temp_dir("branch-points");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        // Build a tree:
        //   m1 → m2 → m3   (linear tail — no branch)
        //         ↓
        //         m4         (second child of m1 creates a branch at m1)
        let _id1_a = manager
            .append_message(AgentMessage::from_llm(Message::user("before-branch")))
            .await
            .expect("before-branch");
        let id_branch = manager
            .append_message(AgentMessage::from_llm(Message::user("branch-root")))
            .await
            .expect("branch-root");
        let _id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("arm-a")))
            .await
            .expect("arm-a");

        // Go back to id_branch and append a second child → branch point.
        manager.branch(&id_branch);
        let _id4 = manager
            .append_message(AgentMessage::from_llm(Message::user("arm-b")))
            .await
            .expect("arm-b");

        let branch_points = manager
            .get_branch_points()
            .await
            .expect("get_branch_points");

        // Exactly one branch point: id_branch.
        assert_eq!(branch_points.len(), 1);
        assert_eq!(branch_points[0].entry_id, id_branch);
        assert_eq!(branch_points[0].children.len(), 2);

        fs::remove_dir_all(dir).ok();
    }

    /// `navigate_to` returns an error when the entry ID does not exist.
    #[tokio::test]
    async fn navigate_to_errors_on_missing_entry() {
        let dir = temp_dir("nav-missing");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        manager
            .append_message(AgentMessage::from_llm(Message::user("hello")))
            .await
            .expect("append");

        let err = manager
            .navigate_to("does-not-exist")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("not found in session"));

        fs::remove_dir_all(dir).ok();
    }

    /// `get_tree` returns an error when no session is active.
    #[tokio::test]
    async fn get_tree_errors_without_active_session() {
        let dir = temp_dir("tree-no-session");
        let manager = SessionManager::new(dir.clone());
        let err = manager
            .get_tree()
            .await
            .expect_err("should fail without active session");
        assert!(err.to_string().contains("No active session"));

        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Branch summarization tests
    // -----------------------------------------------------------------------

    /// `collect_messages_up_to` must return the messages on the ancestor path
    /// from root to the target entry in root-first order, and only include
    /// `Message` entries (not e.g. `ModelChange`).
    #[tokio::test]
    async fn collect_messages_up_to_returns_ancestor_path() {
        use pi_agent_core::context::compaction::BranchSummarizationSettings;

        let dir = temp_dir("collect-messages");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        let _id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("first message")))
            .await
            .expect("first");
        let id2 = manager
            .append_message(AgentMessage::from_llm(Message::user("second message")))
            .await
            .expect("second");
        let _id3 = manager
            .append_message(AgentMessage::from_llm(Message::user("third message")))
            .await
            .expect("third");

        // Collect only up to id2 (not including id3)
        let messages = manager
            .collect_messages_up_to(&id2)
            .await
            .expect("collect_messages_up_to");

        assert_eq!(messages.len(), 2, "should return exactly 2 messages (root to id2)");

        let texts: Vec<String> = messages
            .iter()
            .filter_map(|m| m.as_llm())
            .map(|m| m.text_content())
            .collect();
        assert_eq!(texts[0], "first message");
        assert_eq!(texts[1], "second message");

        // The third message must NOT appear.
        assert!(
            !texts.contains(&"third message".to_string()),
            "id3 must not appear in path up to id2"
        );

        // Also verify that build_branch_summary_prompts returns well-formed prompts.
        let settings = BranchSummarizationSettings::default();
        let (sys, user) = manager
            .build_branch_summary_prompts(&id2, &settings)
            .await
            .expect("build_branch_summary_prompts");

        assert!(sys.contains("branch point"), "system prompt should be branch-specific");
        assert!(user.contains("second message"), "user prompt must embed the messages");
        assert!(user.contains("<conversation>"), "must have conversation tags");

        fs::remove_dir_all(dir).ok();
    }

    /// `collect_messages_up_to` on a branch arm must follow the arm's ancestor
    /// chain — not include messages from sibling branches.
    #[tokio::test]
    async fn collect_messages_up_to_follows_branch_arm() {
        let dir = temp_dir("collect-branch-arm");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        // Shared root message
        let id_root = manager
            .append_message(AgentMessage::from_llm(Message::user("shared root")))
            .await
            .expect("root");

        // Branch A: append two more messages
        let id_a1 = manager
            .append_message(AgentMessage::from_llm(Message::user("arm-A step 1")))
            .await
            .expect("arm-A step 1");
        let id_a2 = manager
            .append_message(AgentMessage::from_llm(Message::user("arm-A step 2")))
            .await
            .expect("arm-A step 2");

        // Branch B: go back to root and append a diverging message
        manager.branch(&id_root);
        let id_b1 = manager
            .append_message(AgentMessage::from_llm(Message::user("arm-B step 1")))
            .await
            .expect("arm-B step 1");

        // collect_messages_up_to(id_a2) → should give [root, arm-A step 1, arm-A step 2]
        let path_a = manager
            .collect_messages_up_to(&id_a2)
            .await
            .expect("collect arm-A path");
        let texts_a: Vec<String> = path_a
            .iter()
            .filter_map(|m| m.as_llm())
            .map(|m| m.text_content())
            .collect();
        assert_eq!(texts_a, vec!["shared root", "arm-A step 1", "arm-A step 2"]);

        // collect_messages_up_to(id_b1) → should give [root, arm-B step 1]
        let path_b = manager
            .collect_messages_up_to(&id_b1)
            .await
            .expect("collect arm-B path");
        let texts_b: Vec<String> = path_b
            .iter()
            .filter_map(|m| m.as_llm())
            .map(|m| m.text_content())
            .collect();
        assert_eq!(texts_b, vec!["shared root", "arm-B step 1"]);

        // Sanity: make sure the id variables are actually used.
        let _ = (id_root, id_a1, id_b1);

        fs::remove_dir_all(dir).ok();
    }

    /// `append_branch_summary` persists a `BranchSummary` entry that can be
    /// round-tripped through JSON and appears correctly in `get_tree`.
    #[tokio::test]
    async fn append_branch_summary_persists_entry() {
        let dir = temp_dir("branch-summary-persist");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");

        let id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("refactor auth module")))
            .await
            .expect("first message");

        let summary_text = "## Goal\nRefactor auth module\n\n## Files Modified\n- modified: src/auth.rs";
        let summary_id = manager
            .append_branch_summary(&id1, summary_text.to_string(), 512)
            .await
            .expect("append_branch_summary");

        // The returned ID must be a non-empty string.
        assert!(!summary_id.is_empty());

        // Read back from disk and verify the entry round-trips correctly.
        let path = manager.session_path().unwrap().to_path_buf();
        let content = std::fs::read_to_string(&path).expect("read session file");
        let last_line = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .last()
            .expect("last line");

        let entry: SessionEntry =
            serde_json::from_str(last_line).expect("branch summary must be valid JSON");
        match &entry {
            SessionEntry::BranchSummary {
                id,
                branch_entry_id,
                summary,
                tokens_before,
                ..
            } => {
                assert_eq!(id, &summary_id);
                assert_eq!(branch_entry_id, &id1);
                assert_eq!(summary, summary_text);
                assert_eq!(*tokens_before, 512u64);
            }
            other => panic!("expected BranchSummary, got {:?}", other),
        }

        // The entry must also appear in get_tree with the correct type.
        let tree = manager.get_tree().await.expect("get_tree");
        let node = tree
            .iter()
            .find(|n| n.entry_id == summary_id)
            .expect("branch summary node must appear in tree");
        assert_eq!(node.entry_type, "branch_summary");

        fs::remove_dir_all(dir).ok();
    }

    /// `collect_messages_up_to` errors when the target entry does not exist.
    #[tokio::test]
    async fn collect_messages_up_to_errors_on_missing_entry() {
        let dir = temp_dir("collect-missing");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        manager
            .append_message(AgentMessage::from_llm(Message::user("hello")))
            .await
            .expect("append");

        let err = manager
            .collect_messages_up_to("nonexistent-id")
            .await
            .expect_err("should fail for missing id");
        assert!(err.to_string().contains("not found in session"));

        fs::remove_dir_all(dir).ok();
    }

    /// `collect_messages_up_to` errors when no session is active.
    #[tokio::test]
    async fn collect_messages_up_to_errors_without_active_session() {
        let dir = temp_dir("collect-no-session");
        let manager = SessionManager::new(dir.clone());

        let err = manager
            .collect_messages_up_to("some-id")
            .await
            .expect_err("should fail without active session");
        assert!(err.to_string().contains("No active session"));

        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Session merging tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn merge_appends_source_entries_to_current_session() {
        let dir = temp_dir("merge-basic");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());
        
        // Create target session with 2 messages
        manager.create_session("/tmp/target").await.expect("target session created");
        let _t1 = manager.append_message(AgentMessage::from_llm(Message::user("target-msg-1"))).await.expect("t1");
        let t2 = manager.append_message(AgentMessage::from_llm(Message::user("target-msg-2"))).await.expect("t2");
        let target_path = manager.session_path().unwrap().to_path_buf();
        
        // Create source session file manually
        let source_path = dir.join("source.jsonl");
        let source_header = SessionHeader::new("source-id".to_string(), "/tmp/source".to_string());
        let s1 = SessionEntry::Message {
            id: "s1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-msg-1")),
        };
        let s2 = SessionEntry::Message {
            id: "s2".to_string(),
            parent_id: Some("s1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-msg-2")),
        };
        let source_content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&source_header).unwrap(),
            serde_json::to_string(&s1).unwrap(),
            serde_json::to_string(&s2).unwrap()
        );
        fs::write(&source_path, source_content).expect("write source");
        
        // Merge source into target
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 2, "should merge 2 entries");
        
        // Verify merged entries are chained from t2
        let lines = read_lines(&target_path).await;
        assert_eq!(lines.len(), 5, "header + 2 target + 2 merged = 5 lines");
        
        // Last entry should be a message with parent_id pointing to t2
        let last_entry = parse_entry(lines.last().unwrap());
        match last_entry {
            SessionEntry::Message { parent_id, .. } => {
                // The last merged entry's parent should be the previous entry in the merged chain
                assert!(parent_id.is_some(), "merged entry should have parent");
            }
            _ => panic!("expected message entry"),
        }
        
        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_errors_without_active_session() {
        let dir = temp_dir("merge-no-session");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());
        
        let source_path = dir.join("source.jsonl");
        fs::write(&source_path, "{}\n").expect("write source");
        
        let err = manager.merge(&source_path).await.expect_err("should fail without session");
        assert!(err.to_string().contains("No active session"));
        
        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_empty_source_returns_zero() {
        let dir = temp_dir("merge-empty");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());
        
        manager.create_session("/tmp/target").await.expect("target session created");
        let target_path = manager.session_path().unwrap().to_path_buf();
        
        // Create empty source
        let source_path = dir.join("source.jsonl");
        let source_header = SessionHeader::new("source-id".to_string(), "/tmp/source".to_string());
        fs::write(&source_path, format!("{}\n", serde_json::to_string(&source_header).unwrap())).expect("write source");
        
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 0, "should merge 0 entries from empty source");
        
        // Target should still only have header
        let lines = read_lines(&target_path).await;
        assert_eq!(lines.len(), 1, "only header line");
        
        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Schema migration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn migrate_session_upgrades_v1_to_v3() {
        let dir = temp_dir("migrate-v1");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v1-session.jsonl");
        
        // Create a v1-style session (minimal header, no version field)
        let v1_header = r#"{"type":"session","id":"v1-test","cwd":"/tmp","timestamp":"2024-01-01T00:00:00Z"}"#;
        let v1_entry = r#"{"type":"message","id":"m1","message":{"role":"user","content":"hello"}}"#;
        fs::write(&path, format!("{}\n{}\n", v1_header, v1_entry)).expect("write v1 session");
        
        // Migrate
        let migrated = SessionManager::migrate_session(&path).await.expect("migrate succeeded");
        assert!(migrated, "should have performed migration");
        
        // Verify upgrade
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("parse header");
        assert_eq!(header.get("version").unwrap().as_u64(), Some(3));
        
        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_session_returns_false_for_current_version() {
        let dir = temp_dir("migrate-current");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v3-session.jsonl");
        
        // Create a v3 session
        let header = SessionHeader::new("v3-test".to_string(), "/tmp".to_string());
        fs::write(&path, format!("{}\n", serde_json::to_string(&header).unwrap())).expect("write v3 session");
        
        // Try to migrate
        let migrated = SessionManager::migrate_session(&path).await.expect("check succeeded");
        assert!(!migrated, "should not migrate already-current version");
        
        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_session_adds_missing_timestamps() {
        let dir = temp_dir("migrate-timestamps");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v2-session.jsonl");
        
        // Create a v2-style session (has version 2, but some entries lack timestamps)
        let v2_header = r#"{"type":"session","version":2,"id":"v2-test","cwd":"/tmp","timestamp":"2024-01-01T00:00:00Z"}"#;
        let v2_entry_no_ts = r#"{"type":"message","id":"m1","parent_id":null,"message":{"role":"user","content":"hello"}}"#;
        fs::write(&path, format!("{}\n{}\n", v2_header, v2_entry_no_ts)).expect("write v2 session");
        
        // Migrate
        SessionManager::migrate_session(&path).await.expect("migrate succeeded");
        
        // Verify entry now has timestamp
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let entry: serde_json::Value = serde_json::from_str(lines[1]).expect("parse entry");
        assert!(entry.get("timestamp").is_some(), "entry should now have timestamp");
        
        fs::remove_dir_all(dir).ok();
    }
}
