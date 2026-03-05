use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedAuth {
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

fn auth_path() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    home.join(".pi").join("agent").join("auth.json")
}

pub fn load_persisted_auth() -> Option<PersistedAuth> {
    let path = auth_path();
    let raw = fs::read_to_string(path).ok()?;
    let parsed: PersistedAuth = serde_json::from_str(&raw).ok()?;
    if parsed.api_key.trim().is_empty() {
        return None;
    }
    Some(parsed)
}

pub fn save_persisted_auth(api_key: &str, provider: Option<&str>) -> Result<()> {
    let path = auth_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = PersistedAuth {
        api_key: api_key.to_string(),
        provider: provider.map(str::to_string),
    };
    let json = serde_json::to_string_pretty(&payload)?;
    fs::write(&path, json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(path, perms);
    }

    Ok(())
}

pub fn clear_persisted_auth() -> Result<()> {
    let path = auth_path();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}
