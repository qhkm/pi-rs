//! vLLM manager for starting and stopping models.

use anyhow::{Context, Result};
use tokio::process::Command;

/// Manages vLLM instances on remote pods
pub struct VllmManager {
    ssh_target: String,
    vllm_version: String,
}

impl VllmManager {
    /// Create a new vLLM manager
    pub fn new(ssh_target: String, vllm_version: String) -> Self {
        Self {
            ssh_target,
            vllm_version,
        }
    }

    /// Find a free port on the remote host
    pub async fn find_free_port(&self, _ssh: &str) -> Result<u16> {
        // For simplicity, use a random port in the high range
        // In production, this would check if the port is actually free
        let output = Command::new("ssh")
            .args(&[
                "-o", "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                "python3 -c 'import socket; s=socket.socket(); s.bind((\"\", 0)); print(s.getsockname()[1]); s.close()'",
            ])
            .output()
            .await
            .context("Failed to find free port")?;

        let port_str = String::from_utf8_lossy(&output.stdout);
        port_str.trim().parse().context("Invalid port number")
    }

    /// Validate that a string is safe to embed in a shell command.
    /// Rejects shell meta-characters.
    fn validate_shell_arg(s: &str, label: &str) -> Result<()> {
        if s.chars().any(|c| {
            matches!(
                c,
                ';' | '|'
                    | '&'
                    | '$'
                    | '`'
                    | '\''
                    | '"'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '\n'
                    | '\r'
                    | ' '
            )
        }) {
            anyhow::bail!("{} contains invalid characters: {}", label, s);
        }
        Ok(())
    }

    /// Start a vLLM model
    pub async fn start_model(
        &self,
        model: &str,
        port: u16,
        gpus: &[u32],
        _memory: Option<&str>,
        context: Option<&str>,
    ) -> Result<u32> {
        // Sanitize inputs to prevent shell injection
        Self::validate_shell_arg(model, "model name")?;
        if let Some(ctx) = context {
            Self::validate_shell_arg(ctx, "context length")?;
        }

        let gpu_str = gpus
            .iter()
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let mut args = vec![
            "--model".to_string(),
            model.to_string(),
            "--port".to_string(),
            port.to_string(),
            "--tensor-parallel-size".to_string(),
            gpus.len().to_string(),
            "--gpu-memory-utilization".to_string(),
            "0.9".to_string(),
        ];

        if let Some(ctx) = context {
            args.push("--max-model-len".to_string());
            args.push(ctx.to_string());
        }

        let cmd = format!(
            "CUDA_VISIBLE_DEVICES={} nohup python3 -m vllm.entrypoints.openai.api_server {} > /tmp/vllm-{}.log 2>&1 & echo $!",
            gpu_str,
            args.join(" "),
            port
        );

        let output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &cmd,
            ])
            .output()
            .await
            .context("Failed to start vLLM")?;

        let pid_str = String::from_utf8_lossy(&output.stdout);
        let pid = pid_str.trim().parse().context("Failed to parse PID")?;

        // Wait a moment for the server to start
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Verify the process is running
        let check = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &format!(
                    "ps -p {} > /dev/null && echo 'running' || echo 'not running'",
                    pid
                ),
            ])
            .output()
            .await?;

        let status = String::from_utf8_lossy(&check.stdout);
        if !status.contains("running") {
            // Try to get error from log
            let log = self.get_logs(pid).await.unwrap_or_default();
            anyhow::bail!(
                "vLLM failed to start. PID {} not running.\nLog:\n{}",
                pid,
                log
            );
        }

        Ok(pid)
    }

    /// Stop a running vLLM instance
    pub async fn stop_model(&self, pid: u32) -> Result<()> {
        let output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &format!(
                    "kill {} 2>/dev/null || kill -9 {} 2>/dev/null || true",
                    pid, pid
                ),
            ])
            .output()
            .await
            .context("Failed to stop vLLM")?;

        // Wait for process to terminate
        for _ in 0..10 {
            let check = Command::new("ssh")
                .args(&[
                    "-o",
                    "StrictHostKeyChecking=accept-new",
                    &self.ssh_target,
                    &format!(
                        "ps -p {} > /dev/null 2>&1 && echo 'running' || echo 'stopped'",
                        pid
                    ),
                ])
                .output()
                .await?;

            let status = String::from_utf8_lossy(&check.stdout);
            if status.contains("stopped") {
                return Ok(());
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        anyhow::bail!("Failed to stop vLLM process {}", pid);
    }

    /// Get logs for a running model by its PID.
    ///
    /// Uses `lsof` to find the actual log file opened by the process, falling
    /// back to the port-based naming convention.
    pub async fn get_logs(&self, pid: u32) -> Result<String> {
        // Try to find the log file associated with this PID via /proc fd links,
        // falling back to the port-based naming convention
        let cmd = format!(
            "readlink -f /proc/{pid}/fd/1 2>/dev/null || ls -t /tmp/vllm-*.log 2>/dev/null | head -1"
        );
        let log_file_output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &cmd,
            ])
            .output()
            .await
            .context("Failed to find log file")?;

        let log_file = String::from_utf8_lossy(&log_file_output.stdout)
            .trim()
            .to_string();

        let cat_cmd = if log_file.is_empty() || log_file.contains("No such file") {
            "echo 'No logs found'".to_string()
        } else {
            format!(
                "tail -n 500 {}",
                log_file.replace(
                    |c: char| !c.is_ascii_alphanumeric()
                        && c != '/'
                        && c != '-'
                        && c != '_'
                        && c != '.',
                    ""
                )
            )
        };

        let output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &cat_cmd,
            ])
            .output()
            .await
            .context("Failed to get logs")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Check if a model is healthy
    pub async fn health_check(&self, port: u16) -> Result<bool> {
        let output = Command::new("ssh")
            .args(&[
                "-o",
                "StrictHostKeyChecking=accept-new",
                &self.ssh_target,
                &format!(
                    "curl -sf http://localhost:{}/health || echo 'unhealthy'",
                    port
                ),
            ])
            .output()
            .await?;

        let response = String::from_utf8_lossy(&output.stdout);
        Ok(!response.contains("unhealthy"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vllm_manager_creation() {
        let mgr = VllmManager::new("user@host".to_string(), "release".to_string());
        assert_eq!(mgr.vllm_version, "release");
    }
}
