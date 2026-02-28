use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    pub id: u32,
    pub name: String,
    pub memory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningModel {
    pub model: String,
    pub port: u16,
    pub gpus: Vec<u32>,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pod {
    pub ssh: String,
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub models: HashMap<String, RunningModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_path: Option<String>,
    #[serde(default = "default_vllm_version")]
    pub vllm_version: String,
}

fn default_vllm_version() -> String {
    "release".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub pods: HashMap<String, Pod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn config_path() -> PathBuf {
        dirs_next::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".pi")
            .join("pods")
            .join("config.json")
    }

    pub fn active_pod(&self) -> Option<(&str, &Pod)> {
        self.active
            .as_deref()
            .and_then(|name| self.pods.get(name).map(|pod| (name, pod)))
    }
}
