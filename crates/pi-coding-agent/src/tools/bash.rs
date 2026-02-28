use super::truncate::{smart_truncate, TruncationConfig};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
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
    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "bash".into(),
                message: "missing 'command'".into(),
            })?;
        let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                tool_name: "bash".into(),
                message: format!("Failed to spawn process: {e}"),
            })?;

        // Take handles before passing child to select branches.
        let mut stdout_handle = child.stdout.take();
        let mut stderr_handle = child.stderr.take();

        let timeout_dur = std::time::Duration::from_secs(timeout_secs);
        let mut abort_rx = ctx.abort.clone();

        // Race: normal completion/timeout vs abort signal.
        // abort_rx.changed() fires on any value change; we check *v == true afterward.
        let status = tokio::select! {
            result = tokio::time::timeout(timeout_dur, child.wait()) => {
                match result {
                    Ok(Ok(status)) => status,
                    Ok(Err(e)) => return Ok(ToolResult::error(format!("Failed to wait for process: {e}"))),
                    Err(_) => {
                        let _ = child.kill().await;
                        return Ok(ToolResult::error(format!("Command timed out after {timeout_secs}s")));
                    }
                }
            }
            result = abort_rx.changed() => {
                // changed() resolves when the value transitions; verify it became true.
                let _ = result;
                if *abort_rx.borrow() {
                    let _ = child.kill().await;
                    return Ok(ToolResult::error("Command aborted"));
                }
                // Value changed but not to true (shouldn't happen in practice).
                // Fall through: wait for process normally.
                child.wait().await.map_err(|e| pi_agent_core::AgentError::ToolExecution {
                    tool_name: "bash".into(),
                    message: format!("Failed to wait for process: {e}"),
                })?
            }
        };

        // Read stdout and stderr from the piped handles.
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        if let Some(ref mut h) = stdout_handle {
            let _ = h.read_to_end(&mut stdout_bytes).await;
        }
        if let Some(ref mut h) = stderr_handle {
            let _ = h.read_to_end(&mut stderr_bytes).await;
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("STDERR:\n");
            result.push_str(&stderr);
        }
        if result.is_empty() {
            result = "(no output)".to_string();
        }

        let cfg = TruncationConfig::default();
        if status.success() {
            Ok(ToolResult::success(smart_truncate(&result, &cfg)))
        } else {
            result.push_str(&format!("\nExit code: {}", status.code().unwrap_or(-1)));
            Ok(ToolResult::error(smart_truncate(&result, &cfg)))
        }
    }
}
