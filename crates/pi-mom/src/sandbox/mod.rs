use serde::{Deserialize, Serialize};

/// Sandbox execution environment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SandboxConfig {
    Host,
    Docker { name: String },
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig::Host
    }
}

/// Execute a command in the sandbox
pub async fn exec_in_sandbox(
    config: &SandboxConfig,
    command: &str,
    cwd: &str,
) -> anyhow::Result<String> {
    match config {
        SandboxConfig::Host => {
            let output = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output()
                .await?;
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        SandboxConfig::Docker { name } => {
            let output = tokio::process::Command::new("docker")
                .args(["exec", "-w", cwd, name, "bash", "-c", command])
                .output()
                .await?;
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
    }
}
