use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A skill (custom CLI tool) that mom can create and use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub command: String,
    pub path: PathBuf,
}

/// Load skills from a directory
pub fn load_skills(dir: &std::path::Path) -> Vec<Skill> {
    // TODO: Scan directory for SKILL.md files and parse them
    Vec::new()
}
