use anyhow::Result;

/// Execute a command on a remote pod via SSH
pub async fn ssh_exec(ssh_cmd: &str, command: &str) -> Result<String> {
    let output = tokio::process::Command::new("ssh")
        .args(ssh_cmd.split_whitespace())
        .arg(command)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("SSH command failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
