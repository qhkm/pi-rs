use serde::{Deserialize, Serialize};

/// Artifact types for the web UI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Artifact {
    Html { title: String, content: String },
    Svg { title: String, content: String },
    Markdown { title: String, content: String },
}
