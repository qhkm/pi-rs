pub mod channel;

use std::path::PathBuf;

pub use channel::ChannelState;

/// Global mom state
pub struct MomState {
    pub workspace: PathBuf,
    pub global_memory: String,
}

impl MomState {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            global_memory: String::new(),
        }
    }
}
