//! SSH client for remote pod management.

use anyhow::{Context, Result};
use tokio::process::Command;

/// SSH connection manager
pub struct SshClient {
    connection_string: String,
}

impl SshClient {
    /// Create a new SSH client
    pub fn new(connection_string: &str) -> Result<Self> {
        Ok(Self {
            connection_string: connection_string.to_string(),
        })
    }

    /// Execute a command on the remote host
    pub async fn exec(&self, command: &str) -> Result<String> {
        let output = Command::new("ssh")
            .args(&[
                "-o", "StrictHostKeyChecking=no",
                "-o", "ConnectTimeout=10",
                &self.connection_string,
                command,
            ])
            .output()
            .await
            .context("Failed to execute SSH command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("SSH command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Test the SSH connection
    pub async fn test_connection(&self) -> Result<()> {
        self.exec("echo 'connected'").await?;
        Ok(())
    }

    /// Detect GPUs on the remote host
    pub async fn detect_gpus(&self) -> Result<Vec<crate::config::Gpu>> {
        let output = self.exec("nvidia-smi --query-gpu=index,name,memory.total --format=csv,noheader").await
            .context("Failed to run nvidia-smi. Is CUDA installed?")?;

        let mut gpus = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 3 {
                gpus.push(crate::config::Gpu {
                    id: parts[0].trim().parse().unwrap_or(0),
                    name: parts[1].trim().to_string(),
                    memory: parts[2].trim().to_string(),
                });
            }
        }

        if gpus.is_empty() {
            anyhow::bail!("No GPUs detected on remote host");
        }

        Ok(gpus)
    }

    /// Start an interactive shell
    pub async fn interactive_shell(&self) -> Result<()> {
        let status = Command::new("ssh")
            .args(&[
                "-o", "StrictHostKeyChecking=no",
                &self.connection_string,
            ])
            .status()
            .await
            .context("Failed to start SSH shell")?;

        if !status.success() {
            anyhow::bail!("SSH shell exited with error");
        }

        Ok(())
    }

    /// Copy a file to the remote host
    pub async fn scp_upload(&self, local_path: &str, remote_path: &str) -> Result<()> {
        let status = Command::new("scp")
            .args(&[
                "-o", "StrictHostKeyChecking=no",
                local_path,
                &format!("{}:{}", self.connection_string, remote_path),
            ])
            .status()
            .await
            .context("Failed to SCP file")?;

        if !status.success() {
            anyhow::bail!("SCP upload failed");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_client_creation() {
        let client = SshClient::new("user@host").unwrap();
        assert_eq!(client.connection_string, "user@host");
    }
}
