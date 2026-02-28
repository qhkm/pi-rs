use std::path::PathBuf;

/// Per-channel state
pub struct ChannelState {
    pub channel_id: String,
    pub dir: PathBuf,
    pub memory: String,
}

impl ChannelState {
    pub fn new(channel_id: String, workspace: &std::path::Path) -> Self {
        let dir = workspace.join(&channel_id);
        Self {
            channel_id,
            dir,
            memory: String::new(),
        }
    }

    /// Path to the channel's log file
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("log.jsonl")
    }

    /// Path to the channel's memory file
    pub fn memory_path(&self) -> PathBuf {
        self.dir.join("MEMORY.md")
    }

    /// Path to the channel's scratch directory
    pub fn scratch_dir(&self) -> PathBuf {
        self.dir.join("scratch")
    }
}
