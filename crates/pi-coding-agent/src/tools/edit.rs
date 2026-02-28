use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use super::operations::FileOperations;

pub struct EditTool {
    ops: Arc<dyn FileOperations>,
}

impl EditTool {
    pub fn new(ops: Arc<dyn FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str { "edit" }
    fn description(&self) -> &str {
        "Perform an exact string replacement in a file. The old_text must match exactly one location in the file."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "old_text": { "type": "string", "description": "Exact text to find and replace" },
                "new_text": { "type": "string", "description": "Replacement text" }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let path_str = args.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(), message: "missing 'path'".into()
            })?;
        let old_text = args.get("old_text").and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(), message: "missing 'old_text'".into()
            })?;
        let new_text = args.get("new_text").and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "edit".into(), message: "missing 'new_text'".into()
            })?;

        let path = resolve_path(&ctx.cwd, path_str);
        let data = self.ops.read_file(&path).await.map_err(|e|
            pi_agent_core::AgentError::ToolExecution {
                tool_name: "edit".into(), message: format!("read {}: {}", path.display(), e)
            })?;
        let content = String::from_utf8_lossy(&data).to_string();

        let matches: Vec<_> = content.match_indices(old_text).collect();
        if matches.is_empty() {
            return Ok(ToolResult::error(format!(
                "old_text not found in {}. Make sure it matches exactly.", path.display()
            )));
        }
        if matches.len() > 1 {
            return Ok(ToolResult::error(format!(
                "old_text matches {} locations in {}. Provide more context to make it unique.",
                matches.len(), path.display()
            )));
        }

        let new_content = content.replacen(old_text, new_text, 1);
        self.ops.write_file(&path, new_content.as_bytes()).await.map_err(|e|
            pi_agent_core::AgentError::ToolExecution {
                tool_name: "edit".into(), message: format!("write {}: {}", path.display(), e)
            })?;

        // Find the line number of the change
        let line_num = content[..matches[0].0].lines().count() + 1;
        Ok(ToolResult::success(format!("Edited {} at line {}", path.display(), line_num)))
    }
}

fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { PathBuf::from(cwd).join(p) }
}
