use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use pi_agent_core::context::compaction::{
    build_branch_summary_prompt, estimate_tokens_str, serialize_conversation,
    BranchSummarizationSettings,
};
use pi_agent_core::messages::{to_llm_messages, AgentMessage};
use std::collections::{HashMap, HashSet};
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
        let lock =
            SessionLock::acquire_with_timeout(path, Self::LOCK_TIMEOUT).with_context(|| {
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

        // Pass 2: build an id → children index and a parent lookup for cycle detection.
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut parent_map: HashMap<String, Option<String>> = HashMap::new();
        for (id, parent_id, _, _) in &raw {
            // Ensure every node has an entry (even if it ends up with no children).
            children_map.entry(id.clone()).or_default();
            parent_map.insert(id.clone(), parent_id.clone());
            if let Some(pid) = parent_id {
                children_map
                    .entry(pid.clone())
                    .or_default()
                    .push(id.clone());
            }
        }

        // Detect and report cycles
        let cycles = Self::detect_cycles(&parent_map);
        if !cycles.is_empty() {
            for cycle in &cycles {
                tracing::warn!("Cycle detected in session tree: {:?}", cycle);
            }
            // Remove cycle edges to break them
            for cycle in cycles {
                if cycle.len() >= 2 {
                    let last = cycle.last().unwrap();
                    let first = cycle.first().unwrap();
                    // Remove the parent link that creates the cycle
                    if let Some(children) = children_map.get_mut(last) {
                        children.retain(|c| c != first);
                    }
                }
            }
        }

        // Pass 2.5: cycle detection — walk from each node upward through
        // parent_id links with a visited set.  If the same ID appears twice
        // in any single walk, the tree contains a cycle.
        for (id, _, _, _) in &raw {
            let mut visited = HashSet::new();
            let mut cursor = Some(id.clone());
            while let Some(current) = cursor {
                if !visited.insert(current.clone()) {
                    anyhow::bail!(
                        "Cycle detected in session tree: entry '{}' appears twice when walking ancestors of '{}'",
                        current,
                        id
                    );
                }
                cursor = parent_map.get(&current).and_then(|p| p.clone());
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

    /// Detect cycles in the parent map using DFS.
    /// Returns a list of cycles found, where each cycle is a list of entry IDs.
    fn detect_cycles(parent_map: &HashMap<String, Option<String>>) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut in_stack = std::collections::HashSet::new();

        for node_id in parent_map.keys() {
            if !visited.contains(node_id) {
                Self::dfs_detect_cycle(
                    node_id,
                    parent_map,
                    &mut visited,
                    &mut in_stack,
                    &mut Vec::new(),
                    &mut cycles,
                );
            }
        }

        cycles
    }

    fn dfs_detect_cycle(
        node_id: &str,
        parent_map: &HashMap<String, Option<String>>,
        visited: &mut std::collections::HashSet<String>,
        in_stack: &mut std::collections::HashSet<String>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(node_id.to_string());
        in_stack.insert(node_id.to_string());
        path.push(node_id.to_string());

        // Follow the parent pointer (we traverse parent -> child, so check who has us as parent)
        for (child_id, parent_id) in parent_map.iter() {
            if let Some(pid) = parent_id {
                if pid == node_id {
                    if !visited.contains(child_id) {
                        Self::dfs_detect_cycle(
                            child_id, parent_map, visited, in_stack, path, cycles,
                        );
                    } else if in_stack.contains(child_id) {
                        // Found a cycle - extract it from path
                        if let Some(pos) = path.iter().position(|id| id == child_id) {
                            let cycle: Vec<String> = path[pos..].iter().cloned().collect();
                            cycles.push(cycle);
                        }
                    }
                }
            }
        }

        path.pop();
        in_stack.remove(node_id);
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
        let mut visited = HashSet::new();
        while let Some(current) = cursor {
            if !visited.insert(current.clone()) {
                anyhow::bail!(
                    "Cycle detected in session tree while navigating to '{}': repeated ancestor '{}'",
                    entry_id,
                    current
                );
            }
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
        // Validate that branch_entry_id doesn't create a cycle.
        // A cycle would occur if the branch_entry_id is the same as the new
        // entry's own ID (impossible since we generate it below) or if it
        // references an entry that already points back to itself through the
        // parent chain.  We check the simpler invariant: branch_entry_id must
        // not equal the current last_entry_id when the last_entry_id is also
        // going to be the parent_id, because that would mean the BranchSummary
        // both summarizes and is a child of the same node, and — more
        // importantly — we walk the ancestor chain from branch_entry_id
        // upward and verify it terminates (no visited node appears twice).
        if let Some(ref last_id) = self.last_entry_id {
            if last_id == branch_entry_id {
                // This is allowed (summarizing the immediate parent is the
                // common case).  The real cycle check is below.
            }
        }

        // Perform a cycle-detection walk: read the session entries and walk
        // from branch_entry_id up through parent_id links.  If we encounter
        // the same ID twice, there is a cycle.
        {
            let path = self
                .current_session
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No active session"))?;

            let data = fs::read_to_string(path)
                .await
                .with_context(|| format!("Failed to read session file: {}", path.display()))?;

            let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
            let mut found = false;
            for (i, line) in data.lines().enumerate() {
                if i == 0 || line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                    let id = entry.id().to_string();
                    let pid = Self::entry_parent_id(&entry);
                    if id == branch_entry_id {
                        found = true;
                    }
                    parent_of.insert(id, pid);
                }
            }

            if !found {
                anyhow::bail!("branch_entry_id '{}' not found in session", branch_entry_id);
            }

            // Walk upward from branch_entry_id and check for cycles.
            let mut visited = HashSet::new();
            let mut cursor = Some(branch_entry_id.to_string());
            while let Some(current) = cursor {
                if !visited.insert(current.clone()) {
                    anyhow::bail!(
                        "Cycle detected in session tree: entry '{}' appears twice in the ancestor chain of '{}'",
                        current,
                        branch_entry_id
                    );
                }
                cursor = parent_of.get(&current).and_then(|p| p.clone());
            }
        }

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
        let _current_path = self
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
            let new_id = id_remap
                .get(&old_id)
                .cloned()
                .unwrap_or_else(|| Uuid::new_v4().to_string());

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
            Message {
                message, timestamp, ..
            } => Message {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                message,
            },
            Compaction {
                summary,
                first_kept_entry_id,
                tokens_before,
                timestamp,
                ..
            } => {
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
            ModelChange {
                model,
                provider,
                timestamp,
                ..
            } => ModelChange {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                model,
                provider,
            },
            ThinkingLevelChange {
                level, timestamp, ..
            } => ThinkingLevelChange {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                level,
            },
            Label {
                label, timestamp, ..
            } => Label {
                id: new_id.to_string(),
                parent_id: new_parent_id.clone(),
                timestamp,
                label,
            },
            BranchSummary {
                branch_entry_id,
                summary,
                tokens_before,
                timestamp,
                ..
            } => {
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
    ///
    /// # Hardening
    ///
    /// * **Header repair**: if the header is valid JSON but missing the `version`
    ///   field, we treat it as version 1 and repair it rather than bailing.
    /// * **v0 detection**: entries that have no `type` field at all are treated as
    ///   `"message"` entries (the only entry kind in the original v0 schema).
    /// * **Timestamp preservation**: when adding a missing `timestamp` field,
    ///   the migration first tries to extract a date from existing fields
    ///   (`created_at`, `time`, `date`, or the header's own timestamp) before
    ///   falling back to `Utc::now()`.
    /// * **Unknown field preservation**: all fields present in the original JSON
    ///   are retained even if the current schema does not know about them.
    /// * **Malformed entry handling**: non-JSON lines are wrapped in a comment
    ///   object so they survive the round-trip without silently disappearing.
    pub async fn migrate_session(path: &Path) -> Result<bool> {
        let data = fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        let lines: Vec<&str> = data.lines().collect();
        if lines.is_empty() {
            anyhow::bail!("Session file is empty");
        }

        // ------------------------------------------------------------------
        // Parse header -- attempt repair if structurally valid JSON but missing
        // required fields (e.g. a corrupt or very old header).
        // ------------------------------------------------------------------
        let mut header: serde_json::Value = match serde_json::from_str(lines[0]) {
            Ok(v) => v,
            Err(_) => {
                // Try to extract JSON from surrounding text (e.g. "XXX{...}YYY")
                let repaired = if let Some(start) = lines[0].find('{') {
                    if let Some(end) = lines[0].rfind('}') {
                        let json_part = &lines[0][start..=end];
                        serde_json::from_str(json_part).ok()
                    } else {
                        None
                    }
                } else {
                    None
                };

                // If extraction failed, build a minimal header so the rest of
                // the file can still be migrated.
                repaired.unwrap_or_else(|| {
                    tracing::warn!("Could not repair header, creating minimal header");
                    serde_json::json!({
                        "type": "session",
                        "version": 0,
                        "id": Uuid::new_v4().to_string(),
                        "timestamp": Utc::now(),
                        "cwd": "."
                    })
                })
            }
        };

        let version = header.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        if version >= 3 {
            return Ok(false); // Already at current version
        }

        // Extract a fallback timestamp from the header for entries that lack one.
        let header_timestamp = header
            .get("timestamp")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(Utc::now()));

        // ------------------------------------------------------------------
        // Migrate header
        // ------------------------------------------------------------------
        let mut migrated_lines = Vec::new();

        if let Some(obj) = header.as_object_mut() {
            obj.insert("version".to_string(), serde_json::json!(3));
            // Ensure type is set
            if !obj.contains_key("type") {
                obj.insert("type".to_string(), serde_json::json!("session"));
            }
            // Ensure the header itself has required fields
            if !obj.contains_key("id") {
                obj.insert(
                    "id".to_string(),
                    serde_json::json!(Uuid::new_v4().to_string()),
                );
            }
            if !obj.contains_key("timestamp") {
                obj.insert("timestamp".to_string(), serde_json::json!(Utc::now()));
            }
            if !obj.contains_key("cwd") {
                obj.insert("cwd".to_string(), serde_json::json!("."));
            }
        }
        migrated_lines.push(serde_json::to_string(&header)?);

        // Track seen IDs to detect collisions during migration
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut id_remap: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // ------------------------------------------------------------------
        // Migrate entries
        // ------------------------------------------------------------------
        for (line_idx, line) in lines[1..].iter().enumerate() {
            if line.trim().is_empty() {
                migrated_lines.push(line.to_string());
                continue;
            }

            let mut entry: serde_json::Value = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("Skipping malformed entry at line {}: {}", line_idx + 2, e);
                    // Wrap malformed lines as a comment entry so they survive
                    // the round-trip without silently disappearing.
                    let comment = serde_json::json!({
                        "type": "message",
                        "id": Uuid::new_v4().to_string(),
                        "parent_id": null,
                        "timestamp": Utc::now(),
                        "_malformed": true,
                        "_malformed_original": line.to_string(),
                        "message": {
                            "kind": "system_context",
                            "content": format!("[migration] malformed entry preserved: {}", line),
                            "source": "migration"
                        }
                    });
                    migrated_lines.push(serde_json::to_string(&comment)?);
                    continue;
                }
            };

            // v0 detection: entries with no `type` field are treated as
            // messages (the only entry kind in the original schema).
            if version == 0 {
                if let Some(obj) = entry.as_object_mut() {
                    if !obj.contains_key("type") {
                        obj.insert("type".to_string(), serde_json::json!("message"));
                    }
                }
            }

            // Version-specific migrations.  All manipulations operate on the
            // raw serde_json::Value so unknown fields are preserved as-is.
            if version < 2 || entry.get("id").is_none() {
                // v0/v1 -> v2: Ensure all entries have an 'id' field
                if let Some(obj) = entry.as_object_mut() {
                    let old_id = obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    // Check for ID collision
                    if let Some(ref id) = old_id {
                        if seen_ids.contains(id) {
                            // Generate new ID and track remap
                            let new_id = Uuid::new_v4().to_string();
                            id_remap.insert(id.clone(), new_id.clone());
                            obj.insert("id".to_string(), serde_json::json!(new_id));
                            tracing::debug!("Remapped duplicate ID {} -> {}", id, new_id);
                        } else {
                            seen_ids.insert(id.clone());
                        }
                    } else {
                        // No ID - generate one
                        obj.insert(
                            "id".to_string(),
                            serde_json::json!(Uuid::new_v4().to_string()),
                        );
                    }

                    // Ensure 'type' field exists
                    if !obj.contains_key("type") {
                        // Infer type from structure or default to message
                        let entry_type = if obj.contains_key("message") {
                            "message"
                        } else if obj.contains_key("summary")
                            && obj.contains_key("first_kept_entry_id")
                        {
                            "compaction"
                        } else if obj.contains_key("model") {
                            "model_change"
                        } else if obj.contains_key("branch_entry_id") {
                            "branch_summary"
                        } else {
                            "message"
                        };
                        obj.insert("type".to_string(), serde_json::json!(entry_type));
                    }
                }
            }

            if version < 3 {
                // v2 -> v3: Add timestamp if missing.
                //
                // Try to preserve an existing timestamp-like field before
                // falling back to the header timestamp, then Utc::now().
                let extracted_ts = if entry.get("timestamp").is_none() {
                    Self::extract_timestamp_from_entry(&entry)
                } else {
                    None
                };

                if let Some(obj) = entry.as_object_mut() {
                    if !obj.contains_key("timestamp") {
                        // Priority: extracted from entry fields > header timestamp > now
                        let ts = if let Some(dt) = extracted_ts {
                            serde_json::json!(dt)
                        } else {
                            Self::extract_existing_timestamp(obj)
                                .unwrap_or_else(|| header_timestamp.clone())
                        };
                        obj.insert("timestamp".to_string(), ts);
                    }
                    // Ensure parent_id field exists (can be null)
                    if !obj.contains_key("parent_id") {
                        obj.insert("parent_id".to_string(), serde_json::Value::Null);
                    }

                    // Remap parent_id if it points to a remapped ID
                    if let Some(parent_id) = obj.get("parent_id").and_then(|v| v.as_str()) {
                        if let Some(new_id) = id_remap.get(parent_id) {
                            obj.insert("parent_id".to_string(), serde_json::json!(new_id));
                        }
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

    /// Try to extract a usable timestamp value from known alternative field
    /// names that older schema versions may have used.
    ///
    /// Returns `Some(value)` if a parseable timestamp was found, `None` otherwise.
    fn extract_existing_timestamp(
        obj: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<serde_json::Value> {
        for key in &["created_at", "time", "date", "ts"] {
            if let Some(val) = obj.get(*key) {
                // Accept a string that chrono can parse, or a numeric unix timestamp.
                match val {
                    serde_json::Value::String(s) => {
                        if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
                            return Some(val.clone());
                        }
                    }
                    serde_json::Value::Number(n) => {
                        if let Some(secs) = n.as_i64() {
                            if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                                return Some(serde_json::json!(dt));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Attempt to extract timestamp from entry content (operates on the full
    /// `serde_json::Value` rather than just the object map).
    fn extract_timestamp_from_entry(entry: &serde_json::Value) -> Option<DateTime<Utc>> {
        // Try to extract from message timestamp if available
        if let Some(msg) = entry.get("message") {
            // Check for nested timestamp in message
            if let Some(ts) = msg.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
                    return Some(parsed.with_timezone(&Utc));
                }
            }
        }

        // Check for created_at or other timestamp fields
        for field in &["created_at", "date", "time"] {
            if let Some(ts) = entry.get(field).and_then(|t| t.as_str()) {
                if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
                    return Some(parsed.with_timezone(&Utc));
                }
            }
        }

        None
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
        let lock =
            SessionLock::acquire_with_timeout(&path, Self::LOCK_TIMEOUT).with_context(|| {
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
        let last_entry: SessionEntry = serde_json::from_str(last_line).expect("parse last entry");
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
        let last_entry: SessionEntry = serde_json::from_str(last_line).expect("parse last entry");
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

    /// `navigate_to` must fail (not hang) when a cycle exists in the ancestor chain.
    #[tokio::test]
    async fn navigate_to_errors_on_cycle() {
        let dir = temp_dir("nav-cycle");
        fs::create_dir_all(&dir).expect("create temp dir");

        let header = SessionHeader::new("nav-cycle-test".to_string(), "/tmp".to_string());
        let m1 = SessionEntry::Message {
            id: "m1".to_string(),
            parent_id: Some("m2".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("one")),
        };
        let m2 = SessionEntry::Message {
            id: "m2".to_string(),
            parent_id: Some("m1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("two")),
        };

        let path = dir.join("nav-cycle.jsonl");
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&m1).unwrap(),
            serde_json::to_string(&m2).unwrap()
        );
        fs::write(&path, content).expect("write session");

        let mut manager = SessionManager::new(dir.clone());
        manager.load_session(&path).await.expect("load session");

        let err = manager
            .navigate_to("m1")
            .await
            .expect_err("navigate_to should fail on cycle");
        assert!(
            err.to_string().contains("Cycle detected"),
            "expected cycle error, got: {}",
            err
        );

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

        assert_eq!(
            messages.len(),
            2,
            "should return exactly 2 messages (root to id2)"
        );

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

        assert!(
            sys.contains("branch point"),
            "system prompt should be branch-specific"
        );
        assert!(
            user.contains("second message"),
            "user prompt must embed the messages"
        );
        assert!(
            user.contains("<conversation>"),
            "must have conversation tags"
        );

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
            .append_message(AgentMessage::from_llm(Message::user(
                "refactor auth module",
            )))
            .await
            .expect("first message");

        let summary_text =
            "## Goal\nRefactor auth module\n\n## Files Modified\n- modified: src/auth.rs";
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
        manager
            .create_session("/tmp/target")
            .await
            .expect("target session created");
        let _t1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-msg-1")))
            .await
            .expect("t1");
        let t2 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-msg-2")))
            .await
            .expect("t2");
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

        let err = manager
            .merge(&source_path)
            .await
            .expect_err("should fail without session");
        assert!(err.to_string().contains("No active session"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_empty_source_returns_zero() {
        let dir = temp_dir("merge-empty");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        manager
            .create_session("/tmp/target")
            .await
            .expect("target session created");
        let target_path = manager.session_path().unwrap().to_path_buf();

        // Create empty source
        let source_path = dir.join("source.jsonl");
        let source_header = SessionHeader::new("source-id".to_string(), "/tmp/source".to_string());
        fs::write(
            &source_path,
            format!("{}\n", serde_json::to_string(&source_header).unwrap()),
        )
        .expect("write source");

        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 0, "should merge 0 entries from empty source");

        // Target should still only have header
        let lines = read_lines(&target_path).await;
        assert_eq!(lines.len(), 1, "only header line");

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_branched_tree_remaps_all_ids() {
        let dir = temp_dir("merge-branched");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create target session with branches
        manager
            .create_session("/tmp/target")
            .await
            .expect("target session created");
        let t1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-1")))
            .await
            .expect("t1");
        let t2 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-2")))
            .await
            .expect("t2");

        // Create a branch from t1
        manager.branch(&t1);
        let t3 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-3-branched")))
            .await
            .expect("t3");

        // Switch back to main line
        manager.branch(&t2);
        let t4 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-4")))
            .await
            .expect("t4");

        let target_path = manager.session_path().unwrap().to_path_buf();

        // Create source session with its own branches
        let source_path = dir.join("source.jsonl");
        let source_header = SessionHeader::new("source-id".to_string(), "/tmp/source".to_string());
        let s1 = SessionEntry::Message {
            id: "s1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-1")),
        };
        let s2 = SessionEntry::Message {
            id: "s2".to_string(),
            parent_id: Some("s1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-2")),
        };
        // Branch in source: s3 from s1
        let s3 = SessionEntry::Message {
            id: "s3".to_string(),
            parent_id: Some("s1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-3-branched")),
        };
        // Continue main line s4 from s2
        let s4 = SessionEntry::Message {
            id: "s4".to_string(),
            parent_id: Some("s2".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-4")),
        };
        let source_content = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            serde_json::to_string(&source_header).unwrap(),
            serde_json::to_string(&s1).unwrap(),
            serde_json::to_string(&s2).unwrap(),
            serde_json::to_string(&s3).unwrap(),
            serde_json::to_string(&s4).unwrap()
        );
        fs::write(&source_path, source_content).expect("write source");

        // Merge source into target
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 4, "should merge 4 entries");

        // Verify tree integrity - no duplicate IDs
        let tree = manager.get_tree().await.expect("get tree");
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for node in &tree {
            assert!(
                ids.insert(&node.entry_id),
                "Duplicate ID found: {}",
                node.entry_id
            );
        }

        // Verify parent chains are intact
        for node in &tree {
            if let Some(ref parent_id) = node.parent_id {
                assert!(
                    ids.contains(parent_id.as_str()),
                    "Parent ID {} not found for entry {}",
                    parent_id,
                    node.entry_id
                );
            }
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_with_id_collisions_remaps_correctly() {
        let dir = temp_dir("merge-collisions");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create target session
        manager
            .create_session("/tmp/target")
            .await
            .expect("target session created");
        let _t1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-1")))
            .await
            .expect("t1");
        let target_path = manager.session_path().unwrap().to_path_buf();

        // Create source session with IDs that might collide format-wise
        let source_path = dir.join("source.jsonl");
        let source_header = SessionHeader::new("source-id".to_string(), "/tmp/source".to_string());

        // Use IDs that look like UUIDs (similar to what target uses)
        let colliding_id = "550e8400-e29b-41d4-a716-446655440000";
        let s1 = SessionEntry::Message {
            id: colliding_id.to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-1")),
        };
        let s2 = SessionEntry::Message {
            id: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_string(),
            parent_id: Some(colliding_id.to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-2")),
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

        // Verify no ID collisions in the merged session
        let tree = manager.get_tree().await.expect("get tree");
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for node in &tree {
            assert!(
                ids.insert(&node.entry_id),
                "Duplicate ID after merge: {}",
                node.entry_id
            );
        }

        // Verify the original collision IDs are NOT present (they should be remapped)
        assert!(
            !ids.contains(colliding_id),
            "Colliding ID should have been remapped"
        );

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_forked_session_preserves_fork_structure() {
        let dir = temp_dir("merge-forked");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create target session
        manager
            .create_session("/tmp/target")
            .await
            .expect("target session created");
        let _t1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-1")))
            .await
            .expect("t1");
        let target_path = manager.session_path().unwrap().to_path_buf();

        // Create forked source: fork from an entry, then add to both
        let mut source_manager = SessionManager::new(dir.join("source"));
        source_manager
            .create_session("/tmp/source")
            .await
            .expect("source session created");
        let s1 = source_manager
            .append_message(AgentMessage::from_llm(Message::user("source-1")))
            .await
            .expect("s1");
        let _s2 = source_manager
            .append_message(AgentMessage::from_llm(Message::user("source-2")))
            .await
            .expect("s2");

        // Fork from s1 and add entry to the fork
        source_manager.branch(&s1);
        let _s3 = source_manager
            .append_message(AgentMessage::from_llm(Message::user("source-3-fork")))
            .await
            .expect("s3");

        let source_path = source_manager.session_path().unwrap().to_path_buf();

        // Merge forked source into target
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 3, "should merge 3 entries");

        // Verify tree structure is preserved
        let tree = manager.get_tree().await.expect("get tree");

        // Find entries that have the fork structure
        let mut found_branched = false;
        for node in &tree {
            // Look for entries that have parent pointing to earlier entry (not the last one)
            if let Some(ref parent_id) = node.parent_id {
                // Check if this entry is a "branched" entry (parent is not the chronologically previous)
                if node.summary.contains("fork") || node.summary.contains("branched") {
                    found_branched = true;
                    // Verify parent exists
                    assert!(
                        tree.iter().any(|n| n.entry_id == *parent_id),
                        "Branched entry's parent should exist"
                    );
                }
            }
        }

        // The fork structure should be preserved (at least one branched entry)
        assert!(
            found_branched,
            "Fork structure should be preserved in merged session"
        );

        fs::remove_dir_all(dir).ok();
    }

    // Note: Concurrent merge testing would require multi-threaded test setup
    // and is better suited as an integration test. The merge function itself
    // is async and uses file append operations which should be atomic.

    // -----------------------------------------------------------------------
    // Schema migration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn migrate_session_upgrades_v1_to_v3() {
        let dir = temp_dir("migrate-v1");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v1-session.jsonl");

        // Create a v1-style session (minimal header, no version field)
        let v1_header =
            r#"{"type":"session","id":"v1-test","cwd":"/tmp","timestamp":"2024-01-01T00:00:00Z"}"#;
        let v1_entry =
            r#"{"type":"message","id":"m1","message":{"role":"user","content":"hello"}}"#;
        fs::write(&path, format!("{}\n{}\n", v1_header, v1_entry)).expect("write v1 session");

        // Migrate
        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migrate succeeded");
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
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&header).unwrap()),
        )
        .expect("write v3 session");

        // Try to migrate
        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("check succeeded");
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
        SessionManager::migrate_session(&path)
            .await
            .expect("migrate succeeded");

        // Verify entry now has timestamp
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let entry: serde_json::Value = serde_json::from_str(lines[1]).expect("parse entry");
        assert!(
            entry.get("timestamp").is_some(),
            "entry should now have timestamp"
        );

        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Merge test coverage (#1)
    // -----------------------------------------------------------------------

    /// Merge a source session that contains branch summaries (manually constructed)
    /// and verify that all IDs (including the `branch_entry_id` inside
    /// `BranchSummary`) are remapped to new UUIDs that don't collide with the
    /// target session.
    #[tokio::test]
    async fn merge_branched_tree_remaps_all_ids_manual_construction() {
        let dir = temp_dir("merge-branch-remap");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create target session with one message.
        manager
            .create_session("/tmp/target")
            .await
            .expect("target session");
        let _t1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-msg")))
            .await
            .expect("t1");
        let target_path = manager.session_path().unwrap().to_path_buf();

        // Build a source session file manually with a message and a branch summary.
        let source_path = dir.join("source-branch.jsonl");
        let source_header =
            SessionHeader::new("source-branch-id".to_string(), "/tmp/source".to_string());
        let src_msg = SessionEntry::Message {
            id: "src-m1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-msg")),
        };
        let src_summary = SessionEntry::BranchSummary {
            id: "src-bs1".to_string(),
            branch_entry_id: "src-m1".to_string(),
            parent_id: Some("src-m1".to_string()),
            timestamp: Utc::now(),
            summary: "Branch summary of source".to_string(),
            tokens_before: 100,
        };
        let source_content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&source_header).unwrap(),
            serde_json::to_string(&src_msg).unwrap(),
            serde_json::to_string(&src_summary).unwrap()
        );
        fs::write(&source_path, source_content).expect("write source");

        // Merge source into target.
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(
            merged_count, 2,
            "should merge 2 entries (msg + branch summary)"
        );

        // Read back target and collect all entry IDs.
        let lines = read_lines(&target_path).await;
        // header(1) + target msg(1) + merged msg(1) + merged branch summary(1) = 4
        assert_eq!(lines.len(), 4, "header + 1 target + 2 merged");

        let mut all_ids: Vec<String> = Vec::new();
        for line in &lines[1..] {
            let entry = parse_entry(line);
            all_ids.push(entry.id().to_string());
        }

        // Verify no old source IDs leaked through.
        assert!(
            !all_ids.contains(&"src-m1".to_string()),
            "old source message ID should be remapped"
        );
        assert!(
            !all_ids.contains(&"src-bs1".to_string()),
            "old source branch summary ID should be remapped"
        );

        // Verify all IDs are unique.
        let id_set: std::collections::HashSet<&String> = all_ids.iter().collect();
        assert_eq!(
            id_set.len(),
            all_ids.len(),
            "all entry IDs must be unique after merge"
        );

        // Verify the branch summary's branch_entry_id was remapped too.
        let last_entry = parse_entry(lines.last().unwrap());
        match last_entry {
            SessionEntry::BranchSummary {
                branch_entry_id, ..
            } => {
                assert_ne!(
                    branch_entry_id, "src-m1",
                    "branch_entry_id must be remapped"
                );
                // The remapped branch_entry_id should point to the remapped message ID.
                assert!(
                    all_ids.contains(&branch_entry_id),
                    "branch_entry_id should reference a valid remapped ID"
                );
            }
            other => panic!("expected BranchSummary, got {:?}", other),
        }

        fs::remove_dir_all(dir).ok();
    }

    /// Create source and target sessions with overlapping entry IDs and verify
    /// that after merge no duplicate IDs exist.
    #[tokio::test]
    async fn merge_id_collision_remaps_correctly() {
        let dir = temp_dir("merge-id-collision");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create target session manually with a known entry ID.
        let target_path = dir.join("target-collision.jsonl");
        let target_header =
            SessionHeader::new("target-collision-id".to_string(), "/tmp/target".to_string());
        let target_msg = SessionEntry::Message {
            id: "shared-id-1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("target-msg")),
        };
        let target_content = format!(
            "{}\n{}\n",
            serde_json::to_string(&target_header).unwrap(),
            serde_json::to_string(&target_msg).unwrap()
        );
        fs::write(&target_path, target_content).expect("write target");

        // Load the target session into the manager.
        manager
            .load_session(&target_path)
            .await
            .expect("load target");

        // Create source session with the SAME entry ID ("shared-id-1") and
        // another entry that references it.
        let source_path = dir.join("source-collision.jsonl");
        let source_header =
            SessionHeader::new("source-collision-id".to_string(), "/tmp/source".to_string());
        let source_msg1 = SessionEntry::Message {
            id: "shared-id-1".to_string(), // Same ID as target!
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-colliding-msg")),
        };
        let source_msg2 = SessionEntry::Message {
            id: "shared-id-2".to_string(),
            parent_id: Some("shared-id-1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("source-msg-2")),
        };
        let source_content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&source_header).unwrap(),
            serde_json::to_string(&source_msg1).unwrap(),
            serde_json::to_string(&source_msg2).unwrap()
        );
        fs::write(&source_path, source_content).expect("write source");

        // Merge source into target.
        let merged_count = manager.merge(&source_path).await.expect("merge succeeded");
        assert_eq!(merged_count, 2);

        // Read all entries and collect IDs.
        let lines = read_lines(&target_path).await;
        // header(1) + target msg(1) + merged(2) = 4
        assert_eq!(lines.len(), 4);

        let mut all_ids: Vec<String> = Vec::new();
        for line in &lines[1..] {
            let entry = parse_entry(line);
            all_ids.push(entry.id().to_string());
        }

        // The original target ID "shared-id-1" should still be present (not remapped).
        assert!(
            all_ids.contains(&"shared-id-1".to_string()),
            "original target entry ID should be preserved"
        );

        // The merged entries should have NEW IDs (not "shared-id-1" or "shared-id-2").
        let id_set: std::collections::HashSet<&String> = all_ids.iter().collect();
        assert_eq!(
            id_set.len(),
            all_ids.len(),
            "all entry IDs must be unique — no duplicates from collision"
        );

        fs::remove_dir_all(dir).ok();
    }

    /// Simulate two sessions forked from the same base, add entries to both,
    /// then merge one into the other and verify tree integrity.
    #[tokio::test]
    async fn merge_forked_session_preserves_integrity() {
        let dir = temp_dir("merge-forked");
        fs::create_dir_all(&dir).expect("create temp dir");
        let mut manager = SessionManager::new(dir.clone());

        // Create a "base" session with some shared messages.
        manager
            .create_session("/tmp/work")
            .await
            .expect("base session");
        let base_id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("shared-base-msg")))
            .await
            .expect("base msg");

        // Fork at base_id1 to create a "fork" session.
        let fork_path = manager
            .fork(&base_id1, "/tmp/work")
            .await
            .expect("fork session");

        // The manager is now pointing to the fork. Add entries there.
        let fork_msg1 = manager
            .append_message(AgentMessage::from_llm(Message::user("fork-msg-1")))
            .await
            .expect("fork msg 1");
        let _fork_msg2 = manager
            .append_message(AgentMessage::from_llm(Message::user("fork-msg-2")))
            .await
            .expect("fork msg 2");

        // Now create a fresh "target" session (the one we'll merge into).
        manager
            .create_session("/tmp/work")
            .await
            .expect("target session");
        let target_id1 = manager
            .append_message(AgentMessage::from_llm(Message::user("target-msg-1")))
            .await
            .expect("target msg 1");
        let target_path = manager.session_path().unwrap().to_path_buf();

        // Merge the fork session into the target.
        let merged_count = manager
            .merge(&fork_path)
            .await
            .expect("merge forked session");
        // The fork file has: header + base_id1 msg + fork_msg1 + fork_msg2 = 3 entries
        assert_eq!(merged_count, 3, "should merge all 3 fork entries");

        // Verify the target session tree is valid.
        let tree = manager.get_tree().await.expect("get_tree after merge");

        // 1 target msg + 3 merged = 4 total entries.
        assert_eq!(tree.len(), 4, "target should have 4 entries after merge");

        // Collect all IDs in the tree.
        let tree_ids: std::collections::HashSet<String> =
            tree.iter().map(|n| n.entry_id.clone()).collect();
        assert_eq!(tree_ids.len(), 4, "all 4 IDs must be unique");

        // The original target message should still be present.
        assert!(
            tree_ids.contains(&target_id1),
            "target msg should be in tree"
        );

        // The fork's original IDs should NOT be present (they were remapped).
        assert!(
            !tree_ids.contains(&base_id1),
            "original fork base_id should be remapped"
        );
        assert!(
            !tree_ids.contains(&fork_msg1),
            "original fork_msg1 should be remapped"
        );

        // Every non-root node should have a valid parent_id that references
        // another node in the tree (integrity check).
        for node in &tree {
            if let Some(ref pid) = node.parent_id {
                assert!(
                    tree_ids.contains(pid),
                    "parent_id '{}' of node '{}' must reference an existing node",
                    pid,
                    node.entry_id
                );
            }
        }

        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Circular branch reference handling (#5)
    // -----------------------------------------------------------------------

    /// `get_tree` must detect and report a cycle if the session file contains
    /// a circular parent_id chain (e.g. A → B → A).
    #[tokio::test]
    async fn get_tree_detects_cycle() {
        let dir = temp_dir("tree-cycle");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("cyclic.jsonl");

        // Manually construct a session file with a cycle: m1 → m2 → m1
        let header = SessionHeader::new("cycle-test".to_string(), "/tmp".to_string());
        let m1 = SessionEntry::Message {
            id: "m1".to_string(),
            parent_id: Some("m2".to_string()), // cycle: m1's parent is m2
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("msg-1")),
        };
        let m2 = SessionEntry::Message {
            id: "m2".to_string(),
            parent_id: Some("m1".to_string()), // cycle: m2's parent is m1
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("msg-2")),
        };
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&m1).unwrap(),
            serde_json::to_string(&m2).unwrap()
        );
        fs::write(&path, content).expect("write cyclic session");

        let mut manager = SessionManager::new(dir.clone());
        manager
            .load_session(&path)
            .await
            .expect("load cyclic session");

        let err = manager
            .get_tree()
            .await
            .expect_err("get_tree should fail on cyclic session");
        assert!(
            err.to_string().contains("Cycle detected"),
            "error should mention cycle detection, got: {}",
            err
        );

        fs::remove_dir_all(dir).ok();
    }

    /// `append_branch_summary` must reject a `branch_entry_id` that does not
    /// exist in the session.
    #[tokio::test]
    async fn append_branch_summary_rejects_missing_branch_entry_id() {
        let dir = temp_dir("branch-summary-missing");
        let mut manager = SessionManager::new(dir.clone());
        manager
            .create_session("/tmp/work")
            .await
            .expect("session created");
        manager
            .append_message(AgentMessage::from_llm(Message::user("hello")))
            .await
            .expect("append msg");

        let err = manager
            .append_branch_summary("nonexistent-id", "summary".to_string(), 100)
            .await
            .expect_err("should fail for missing branch_entry_id");
        assert!(
            err.to_string().contains("not found in session"),
            "error should mention not found, got: {}",
            err
        );

        fs::remove_dir_all(dir).ok();
    }

    /// `append_branch_summary` must still succeed for a valid, non-cyclic
    /// branch_entry_id (regression test: the cycle check must not break the
    /// happy path).
    #[tokio::test]
    async fn append_branch_summary_succeeds_for_valid_entry() {
        let dir = temp_dir("branch-summary-valid");
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

        // Summarize the first message from the perspective of the second.
        let summary_id = manager
            .append_branch_summary(&id1, "Summary of first".to_string(), 256)
            .await
            .expect("append_branch_summary should succeed");
        assert!(!summary_id.is_empty());

        // Also summarize the second message (immediate parent — common case).
        let summary_id2 = manager
            .append_branch_summary(&id2, "Summary of second".to_string(), 512)
            .await
            .expect("append_branch_summary for immediate parent should succeed");
        assert!(!summary_id2.is_empty());

        // The tree should still be valid (no false-positive cycle detection).
        let tree = manager.get_tree().await.expect("get_tree");
        // id1, id2, summary_id, summary_id2 = 4 entries
        assert_eq!(tree.len(), 4);

        fs::remove_dir_all(dir).ok();
    }

    // -----------------------------------------------------------------------
    // Schema migration hardening tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn migrate_v0_entries_get_type_field() {
        let dir = temp_dir("migrate-v0");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v0-session.jsonl");

        // v0 header: no version field, no type field
        let content = r#"{"id":"s1","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
{"id":"e1","message":{"type":"system_context","content":"hello","source":"user"}}
"#;
        fs::write(&path, content).expect("write v0 file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");
        assert!(migrated, "should report migration was performed");

        let data = fs::read_to_string(&path).expect("read migrated file");
        let lines: Vec<&str> = data.lines().filter(|l| !l.trim().is_empty()).collect();

        // Header should now have version 3 and type "session"
        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("valid header json");
        assert_eq!(header["version"], 3);
        assert_eq!(header["type"], "session");

        // Entry should now have type "message" (v0 default)
        let entry: serde_json::Value = serde_json::from_str(lines[1]).expect("valid entry json");
        assert_eq!(entry["type"], "message");
        // Must also have parent_id and timestamp
        assert!(entry.get("parent_id").is_some());
        assert!(entry.get("timestamp").is_some());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_preserves_unknown_fields() {
        let dir = temp_dir("migrate-unknown-fields");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("extra-fields.jsonl");

        // v2 session with an entry that has an extra "custom_data" field
        let content = r#"{"type":"session","version":2,"id":"s1","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
{"type":"message","id":"e1","message":{"type":"system_context","content":"hi","source":"user"},"custom_data":"preserve_me"}
"#;
        fs::write(&path, content).expect("write file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");
        assert!(migrated);

        let data = fs::read_to_string(&path).expect("read migrated");
        let entry_line = data.lines().nth(1).expect("entry line");
        let entry: serde_json::Value = serde_json::from_str(entry_line).expect("valid json");

        // The custom_data field must be preserved
        assert_eq!(entry["custom_data"], "preserve_me");
        // Migration must have added timestamp and parent_id
        assert!(entry.get("timestamp").is_some());
        assert!(entry.get("parent_id").is_some());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_repairs_corrupt_header() {
        let dir = temp_dir("migrate-corrupt-header");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("corrupt-header.jsonl");

        // Header is not valid JSON at all
        let content = "not-json-at-all\n{\"type\":\"message\",\"id\":\"e1\",\"message\":{\"type\":\"system_context\",\"content\":\"hello\",\"source\":\"user\"}}\n";
        fs::write(&path, content).expect("write file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed despite corrupt header");
        assert!(migrated);

        let data = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = data.lines().filter(|l| !l.trim().is_empty()).collect();

        // A repaired header should be present
        let header: serde_json::Value =
            serde_json::from_str(lines[0]).expect("header should be valid json");
        assert_eq!(header["version"], 3);
        assert_eq!(header["type"], "session");
        assert!(header.get("id").is_some());
        assert!(header.get("cwd").is_some());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_wraps_malformed_entries() {
        let dir = temp_dir("migrate-malformed");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("malformed.jsonl");

        let content = r#"{"type":"session","version":1,"id":"s1","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
this-is-not-json
{"type":"message","id":"e1","message":{"type":"system_context","content":"valid","source":"user"}}
"#;
        fs::write(&path, content).expect("write file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");
        assert!(migrated);

        let data = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = data.lines().filter(|l| !l.trim().is_empty()).collect();

        assert_eq!(lines.len(), 3, "header + wrapped malformed + valid entry");

        // The malformed line should be wrapped in a comment-like entry
        let wrapped: serde_json::Value =
            serde_json::from_str(lines[1]).expect("wrapped entry should be valid json");
        assert_eq!(wrapped["type"], "message");
        assert_eq!(wrapped["message"]["kind"], "system_context");
        assert!(wrapped["_malformed_original"]
            .as_str()
            .unwrap()
            .contains("this-is-not-json"));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_extracts_created_at_timestamp() {
        let dir = temp_dir("migrate-timestamp");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("ts.jsonl");

        // v2 entry has no "timestamp" but has "created_at" with an RFC3339 date
        let content = r#"{"type":"session","version":2,"id":"s1","timestamp":"2025-06-15T12:00:00Z","cwd":"/tmp"}
{"type":"message","id":"e1","created_at":"2025-03-10T08:30:00Z","message":{"type":"system_context","content":"hi","source":"user"}}
"#;
        fs::write(&path, content).expect("write file");

        SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");

        let data = fs::read_to_string(&path).expect("read migrated");
        let entry_line = data.lines().nth(1).expect("entry line");
        let entry: serde_json::Value = serde_json::from_str(entry_line).expect("valid json");

        // The timestamp should have been extracted from created_at
        let ts = entry["timestamp"].as_str().expect("timestamp string");
        assert!(
            ts.contains("2025-03-10"),
            "timestamp should come from created_at, got: {}",
            ts
        );

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_falls_back_to_header_timestamp() {
        let dir = temp_dir("migrate-header-ts");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("header-ts.jsonl");

        // v2 entry has no timestamp and no alternative fields — should get
        // the header's timestamp.
        let content = r#"{"type":"session","version":2,"id":"s1","timestamp":"2025-06-15T12:00:00Z","cwd":"/tmp"}
{"type":"message","id":"e1","message":{"type":"system_context","content":"hi","source":"user"}}
"#;
        fs::write(&path, content).expect("write file");

        SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");

        let data = fs::read_to_string(&path).expect("read migrated");
        let entry_line = data.lines().nth(1).expect("entry line");
        let entry: serde_json::Value = serde_json::from_str(entry_line).expect("valid json");

        // The timestamp should have been taken from the header
        let ts = entry["timestamp"].as_str().expect("timestamp string");
        assert!(
            ts.contains("2025-06-15"),
            "timestamp should fall back to header, got: {}",
            ts
        );

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_v3_is_noop() {
        let dir = temp_dir("migrate-v3-noop");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("v3.jsonl");

        let header = SessionHeader::new("s1".to_string(), "/tmp".to_string());
        let content = format!("{}\n", serde_json::to_string(&header).expect("json"));
        fs::write(&path, &content).expect("write file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("noop migration");
        assert!(!migrated, "v3 file should not be migrated");

        // Content should be unchanged
        let after = fs::read_to_string(&path).expect("read after");
        assert_eq!(content, after);

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_header_missing_version_treated_as_v0() {
        let dir = temp_dir("migrate-no-version");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("no-ver.jsonl");

        // Header is valid JSON but has no version field
        let content = r#"{"type":"session","id":"s1","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
{"id":"e1","message":{"type":"system_context","content":"test","source":"user"}}
"#;
        fs::write(&path, content).expect("write file");

        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migration should succeed");
        assert!(migrated);

        let data = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = data.lines().filter(|l| !l.trim().is_empty()).collect();

        let header: serde_json::Value = serde_json::from_str(lines[0]).expect("valid header json");
        assert_eq!(header["version"], 3);

        // Entry should have gotten type "message" (v0 default) + id + parent_id + timestamp
        let entry: serde_json::Value = serde_json::from_str(lines[1]).expect("valid entry json");
        assert_eq!(entry["type"], "message");
        assert!(entry.get("id").is_some());
        assert!(entry.get("parent_id").is_some());
        assert!(entry.get("timestamp").is_some());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn extract_existing_timestamp_parses_alternatives() {
        // Test the helper directly
        let mut map = serde_json::Map::new();
        map.insert(
            "created_at".to_string(),
            serde_json::json!("2025-05-20T10:00:00Z"),
        );
        let ts = SessionManager::extract_existing_timestamp(&map);
        assert!(ts.is_some());
        assert!(ts.unwrap().as_str().unwrap().contains("2025-05-20"));

        // Numeric timestamp
        let mut map2 = serde_json::Map::new();
        map2.insert("ts".to_string(), serde_json::json!(1700000000));
        let ts2 = SessionManager::extract_existing_timestamp(&map2);
        assert!(ts2.is_some());

        // No matching fields
        let mut map3 = serde_json::Map::new();
        map3.insert("foo".to_string(), serde_json::json!("bar"));
        let ts3 = SessionManager::extract_existing_timestamp(&map3);
        assert!(ts3.is_none());

        // Invalid date string
        let mut map4 = serde_json::Map::new();
        map4.insert("created_at".to_string(), serde_json::json!("not-a-date"));
        let ts4 = SessionManager::extract_existing_timestamp(&map4);
        assert!(ts4.is_none());
    }

    // -----------------------------------------------------------------------
    // Additional cycle and merge tests from parallel implementation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn navigate_to_handles_valid_tree() {
        let dir = temp_dir("navigate-valid");
        fs::create_dir_all(&dir).expect("create temp dir");

        // Create session with valid (non-cyclic) tree: a -> b -> c
        let header = SessionHeader::new("nav-test".to_string(), "/tmp".to_string());
        let a = SessionEntry::Message {
            id: "a".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Root A")),
        };
        let b = SessionEntry::Message {
            id: "b".to_string(),
            parent_id: Some("a".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Child B")),
        };
        let c = SessionEntry::Message {
            id: "c".to_string(),
            parent_id: Some("b".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Child C")),
        };

        let path = dir.join("valid.jsonl");
        let content = format!(
            "{}\n{}\n{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            serde_json::to_string(&c).unwrap()
        );
        fs::write(&path, content).expect("write session");

        let mut manager = SessionManager::new(dir.clone());
        manager.load_session(&path).await.expect("load session");

        // Navigate should work on valid tree
        let messages = manager.navigate_to("c").await.expect("navigate to c");
        assert_eq!(messages.len(), 3, "Should have 3 messages in path");

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn merge_preserves_valid_parent_chains() {
        let dir = temp_dir("merge-parent-chains");
        fs::create_dir_all(&dir).expect("create temp dir");

        // Create target session
        let mut target_manager = SessionManager::new(dir.clone());
        target_manager
            .create_session("/tmp/target")
            .await
            .expect("create target");
        let _t1 = target_manager
            .append_message(AgentMessage::from_llm(Message::user("Target 1")))
            .await
            .expect("t1");

        // Create source with a simple chain
        let source_path = dir.join("source.jsonl");
        let header = SessionHeader::new("source".to_string(), "/tmp".to_string());
        let s1 = SessionEntry::Message {
            id: "s1".to_string(),
            parent_id: None,
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Source 1")),
        };
        let s2 = SessionEntry::Message {
            id: "s2".to_string(),
            parent_id: Some("s1".to_string()),
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Source 2")),
        };
        fs::write(
            &source_path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&header).unwrap(),
                serde_json::to_string(&s1).unwrap(),
                serde_json::to_string(&s2).unwrap()
            ),
        )
        .expect("write source");

        // Merge
        let merged = target_manager.merge(&source_path).await.expect("merge");
        assert_eq!(merged, 2);

        // Verify tree is still valid
        let tree = target_manager.get_tree().await.expect("get tree");
        let all_ids: std::collections::HashSet<_> = tree.iter().map(|n| &n.entry_id).collect();

        // No duplicates after merge
        assert_eq!(all_ids.len(), tree.len());

        // Valid parent chains
        for node in &tree {
            if let Some(ref parent_id) = node.parent_id {
                assert!(all_ids.contains(parent_id));
            }
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn get_tree_handles_orphan_parent() {
        let dir = temp_dir("orphan-parent");
        fs::create_dir_all(&dir).expect("create temp dir");

        // Create session with entry pointing to non-existent parent
        let header = SessionHeader::new("orphan".to_string(), "/tmp".to_string());
        let orphan = SessionEntry::Message {
            id: "orphan".to_string(),
            parent_id: Some("nonexistent".to_string()), // Parent doesn't exist
            timestamp: Utc::now(),
            message: AgentMessage::from_llm(Message::user("Orphan entry")),
        };

        let path = dir.join("orphan.jsonl");
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&orphan).unwrap()
        );
        fs::write(&path, content).expect("write session");

        let mut manager = SessionManager::new(dir.clone());
        manager.load_session(&path).await.expect("load session");

        // get_tree should handle orphan entries
        let tree = manager.get_tree().await.expect("get tree");
        assert_eq!(tree.len(), 1);

        // The orphan entry should have no children
        assert!(tree[0].children.is_empty());

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_session_handles_id_collisions_remap() {
        let dir = temp_dir("migrate-collision-remap");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("collision.jsonl");

        // Create a v1 session with duplicate IDs
        let v1_header = r#"{"type":"session","version":1,"id":"test","cwd":"/tmp"}"#;
        let entry1 = r#"{"type":"message","id":"dup","message":{"role":"user","content":"first"}}"#;
        let entry2 =
            r#"{"type":"message","id":"dup","message":{"role":"user","content":"second"}}"#;
        fs::write(&path, format!("{}\n{}\n{}\n", v1_header, entry1, entry2))
            .expect("write session");

        // Migrate
        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migrate succeeded");
        assert!(migrated, "should have performed migration");

        // Verify no duplicate IDs in migrated file
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (i, line) in lines.iter().enumerate().skip(1) {
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(id) = entry.get("id").and_then(|v| v.as_str()) {
                    assert!(
                        ids.insert(id.to_string()),
                        "Duplicate ID found at line {}: {}",
                        i + 1,
                        id
                    );
                }
            }
        }

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_session_handles_header_corruption_with_embedded_json() {
        let dir = temp_dir("migrate-corrupt-embedded");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("corrupt-session.jsonl");

        // Create a session with corrupted header (extra text before/after JSON)
        let corrupt_header = r#"XXX{"type":"session","version":1,"id":"test","cwd":"/tmp","timestamp":"2024-01-01T00:00:00Z"}YYY"#;
        let entry = r#"{"type":"message","id":"m1","message":{"role":"user","content":"hello"}}"#;
        fs::write(&path, format!("{}\n{}\n", corrupt_header, entry))
            .expect("write corrupt session");

        // Migrate should repair and succeed
        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migrate succeeded");
        assert!(migrated, "should have performed migration after repair");

        // Verify the file is now valid
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let header: serde_json::Value =
            serde_json::from_str(lines[0]).expect("parse repaired header");
        assert_eq!(header.get("version").unwrap().as_u64(), Some(3));

        fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn migrate_session_marks_malformed_entries_with_flag() {
        let dir = temp_dir("migrate-malformed-flag");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("malformed-session.jsonl");

        // Create a session with a malformed entry
        let header = r#"{"type":"session","version":2,"id":"test","cwd":"/tmp"}"#;
        let valid_entry = r#"{"type":"message","id":"m1","parent_id":null,"timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"valid"}}"#;
        let malformed_entry = r#"{this is not valid json"#;
        fs::write(
            &path,
            format!("{}\n{}\n{}\n", header, valid_entry, malformed_entry),
        )
        .expect("write session");

        // Migrate
        let migrated = SessionManager::migrate_session(&path)
            .await
            .expect("migrate succeeded");
        assert!(migrated, "should have performed migration");

        // Verify malformed entry is marked
        let content = fs::read_to_string(&path).expect("read migrated");
        let lines: Vec<&str> = content.lines().collect();
        let malformed_parsed: serde_json::Value =
            serde_json::from_str(lines[2]).expect("parse marked entry");
        assert!(
            malformed_parsed.get("_malformed").is_some(),
            "Malformed entry should be marked with _malformed flag"
        );
        assert!(
            malformed_parsed.get("_malformed_original").is_some(),
            "Malformed entry should preserve original content"
        );

        fs::remove_dir_all(dir).ok();
    }
}
