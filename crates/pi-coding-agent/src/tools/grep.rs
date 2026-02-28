use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde_json::Value;
use regex::Regex;

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &str { "grep" }
    fn description(&self) -> &str {
        "Search for a pattern in files. Supports regex patterns and glob file filters."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex or literal pattern to search" },
                "path": { "type": "string", "description": "Directory or file to search (default: cwd)" },
                "glob": { "type": "string", "description": "Glob pattern to filter files (e.g. '*.rs')" },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive search" },
                "limit": { "type": "integer", "description": "Maximum matches to return (default: 100)" }
            },
            "required": ["pattern"]
        })
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let pattern = args.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| pi_agent_core::AgentError::ToolValidation {
                tool_name: "grep".into(), message: "missing 'pattern'".into()
            })?;
        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(&ctx.cwd);
        let ignore_case = args.get("ignore_case").and_then(|v| v.as_bool()).unwrap_or(false);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
        let glob_pattern = args.get("glob").and_then(|v| v.as_str());

        let regex_pattern = if ignore_case {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        // Validate the regex pattern before proceeding
        Regex::new(&regex_pattern).map_err(|e|
            pi_agent_core::AgentError::ToolValidation {
                tool_name: "grep".into(), message: format!("invalid pattern: {e}")
            })?;

        // Use grep shell command for speed
        let mut cmd_args = vec!["-rn".to_string()];
        if ignore_case { cmd_args.push("-i".to_string()); }
        cmd_args.push("--".to_string());
        cmd_args.push(pattern.to_string());
        if let Some(glob_pat) = glob_pattern {
            cmd_args.push("--include".to_string());
            cmd_args.push(glob_pat.to_string());
        }
        cmd_args.push(search_path.to_string());

        let output = tokio::process::Command::new("grep")
            .args(&cmd_args)
            .current_dir(&ctx.cwd)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let lines: Vec<&str> = stdout.lines().take(limit).collect();
                if lines.is_empty() {
                    Ok(ToolResult::success("No matches found"))
                } else {
                    let total_lines = stdout.lines().count();
                    let truncated = if total_lines > limit {
                        format!("\n... (truncated at {} matches)", limit)
                    } else {
                        String::new()
                    };
                    Ok(ToolResult::success(format!("{}{}", lines.join("\n"), truncated)))
                }
            }
            Err(e) => Ok(ToolResult::error(format!("grep failed: {e}"))),
        }
    }
}
