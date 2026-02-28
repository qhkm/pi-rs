use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use tokio::process::Command;

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str {
        "Execute a bash command and return its output. The command runs in the current working directory."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default: 120)" }
            },
            "required": ["command"]
        })
    }
    fn requires_approval(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let command = args.get("command").and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "bash".into(), message: "missing 'command'".into()
            })?;
        let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("bash")
                .arg("-c")
                .arg(command)
                .current_dir(&ctx.cwd)
                .output()
        ).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() { result.push('\n'); }
                    result.push_str("STDERR:\n");
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = "(no output)".to_string();
                }
                let is_error = !out.status.success();
                if is_error {
                    result.push_str(&format!("\nExit code: {}", out.status.code().unwrap_or(-1)));
                }
                if is_error {
                    Ok(ToolResult::error(result))
                } else {
                    Ok(ToolResult::success(result))
                }
            }
            Ok(Err(e)) => Ok(ToolResult::error(format!("Failed to execute: {e}"))),
            Err(_) => Ok(ToolResult::error(format!("Command timed out after {timeout_secs}s"))),
        }
    }
}
