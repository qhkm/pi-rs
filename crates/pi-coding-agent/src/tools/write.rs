use super::operations::{resolve_and_validate_path, FileOperations};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use std::sync::Arc;

pub struct WriteTool {
    ops: Arc<dyn FileOperations>,
}

impl WriteTool {
    pub fn new(ops: Arc<dyn FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl AgentTool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Create or overwrite a file with the given content. Creates parent directories if needed."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let path_str = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "write".into(),
                message: "missing 'path'".into(),
            }
        })?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "write".into(),
                message: "missing 'content'".into(),
            })?;

        let path = resolve_and_validate_path(&ctx.cwd, path_str).map_err(|msg| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "write".into(),
                message: msg,
            }
        })?;
        self.ops
            .write_file(&path, content.as_bytes())
            .await
            .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                tool_name: "write".into(),
                message: format!("{}: {}", path.display(), e),
            })?;

        let line_count = content.lines().count();
        Ok(ToolResult::success(format!(
            "Wrote {} lines to {}",
            line_count,
            path.display()
        )))
    }

    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(WriteTool {
            ops: self.ops.clone(),
        })
    }
}
