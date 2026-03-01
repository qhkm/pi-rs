use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;

pub struct FindTool;

impl FindTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FindTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for FindTool {
    fn name(&self) -> &str {
        "find"
    }
    fn description(&self) -> &str {
        "Search for files and directories by name pattern. Uses glob matching."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.rs', 'src/**/test_*')" },
                "path": { "type": "string", "description": "Root directory to search from (default: cwd)" },
                "limit": { "type": "integer", "description": "Maximum results (default: 200)" }
            },
            "required": ["pattern"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "find".into(),
                message: "missing 'pattern'".into(),
            })?;
        let root = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.cwd);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize;

        let full_pattern = format!("{}/{}", root, pattern);
        let entries: Vec<String> = glob::glob(&full_pattern)
            .map_err(|e| pi_agent_core::AgentError::ToolValidation {
                tool_name: "find".into(),
                message: format!("invalid glob: {e}"),
            })?
            .filter_map(|r| r.ok())
            .take(limit)
            .map(|p| p.display().to_string())
            .collect();

        if entries.is_empty() {
            Ok(ToolResult::success("No files found matching pattern"))
        } else {
            let count = entries.len();
            Ok(ToolResult::success(format!(
                "{}\n({} files)",
                entries.join("\n"),
                count
            )))
        }
    }

    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(FindTool)
    }
}
