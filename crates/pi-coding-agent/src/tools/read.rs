use super::operations::{resolve_and_validate_path, FileOperations};
use super::truncate::{smart_truncate, TruncationConfig};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use std::sync::Arc;

pub struct ReadTool {
    ops: Arc<dyn FileOperations>,
}

impl ReadTool {
    pub fn new(ops: Arc<dyn FileOperations>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read the contents of a file. Supports optional line offset and limit for large files."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to cwd or absolute)" },
                "offset": { "type": "integer", "description": "1-indexed line number to start from" },
                "limit": { "type": "integer", "description": "Maximum number of lines to read" }
            },
            "required": ["path"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let path_str = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "read".into(),
                message: "missing 'path'".into(),
            }
        })?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let path = resolve_and_validate_path(&ctx.cwd, path_str).map_err(|msg| {
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "read".into(),
                message: msg,
            }
        })?;
        let data = self.ops.read_file(&path).await.map_err(|e| {
            pi_agent_core::AgentError::ToolExecution {
                tool_name: "read".into(),
                message: format!("{}: {}", path.display(), e),
            }
        })?;

        let content = String::from_utf8_lossy(&data);
        let lines: Vec<&str> = content.lines().collect();
        let start = (offset.saturating_sub(1)).min(lines.len());
        let end = if let Some(lim) = limit {
            (start + lim).min(lines.len())
        } else {
            lines.len()
        };

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            output.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
        }

        let cfg = TruncationConfig::default();
        let output = smart_truncate(&output, &cfg);

        Ok(ToolResult::success(output))
    }
    
    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(ReadTool { ops: self.ops.clone() })
    }
}
