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

/// Tool wrapper definition for intercepting and modifying tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolWrapperDef {
    /// Name of the tool to wrap (can be * for all tools)
    pub tool_name: String,
    /// Description of what this wrapper does
    pub description: String,
    /// The wrapper implementation type
    pub wrapper_type: WrapperType,
    /// Pre-execution hook script/binary path
    pub before_hook: Option<String>,
    /// Post-execution hook script/binary path  
    pub after_hook: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapperType {
    /// Shell script wrapper
    Shell,
    /// WASM module wrapper
    Wasm,
    /// Binary executable wrapper
    Binary,
    /// Inline JavaScript/TypeScript (for extensions with embedded runtime)
    Inline,
}

/// A loaded extension
pub struct Extension {
    pub manifest: ExtensionManifest,
    pub path: std::path::PathBuf,
}
