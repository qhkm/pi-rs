use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use std::path::PathBuf;

pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }
    fn description(&self) -> &str {
        "List the contents of a directory."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path (default: cwd)" }
            },
            "required": []
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.cwd);
        let path = if PathBuf::from(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            PathBuf::from(&ctx.cwd).join(path_str)
        };

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await.map_err(|e| {
            pi_agent_core::AgentError::ToolExecution {
                tool_name: "ls".into(),
                message: format!("{}: {}", path.display(), e),
            }
        })?;

        while let Some(entry) =
            dir.next_entry()
                .await
                .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                    tool_name: "ls".into(),
                    message: e.to_string(),
                })?
        {
            let meta = entry.metadata().await.ok();
            let name = entry.file_name().to_string_lossy().to_string();
            let suffix = if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

            if suffix == "/" {
                entries.push(format!("{}{}", name, suffix));
            } else {
                entries.push(format!("{:<40} {:>8}", name, format_size(size)));
            }
        }

        entries.sort();
        if entries.is_empty() {
            Ok(ToolResult::success("(empty directory)"))
        } else {
            Ok(ToolResult::success(entries.join("\n")))
        }
    }

    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(LsTool)
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
