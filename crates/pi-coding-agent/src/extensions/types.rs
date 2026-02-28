use serde::{Deserialize, Serialize};

/// Extension manifest loaded from a directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tools: Vec<ExtensionToolDef>,
    #[serde(default)]
    pub commands: Vec<ExtensionCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub executor: ExecutorType,
    pub command: Option<String>,
    pub binary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorType {
    #[default]
    Shell,
    Binary,
    Wasm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionCommand {
    pub name: String,
    pub description: String,
}

/// A loaded extension
pub struct Extension {
    pub manifest: ExtensionManifest,
    pub path: std::path::PathBuf,
}
