use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use pi_ai::ThinkingLevel;

/// Session metadata for listing/filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
    pub message_count: usize,
    pub thinking_level: ThinkingLevel,
    pub preview: String,
    pub usage: SessionUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// Settings key-value store
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub default_provider: String,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,
}

/// Provider API key storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderKey {
    pub provider: String,
    pub api_key: String,
}

/// Custom provider configuration (Ollama, LM Studio, vLLM)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Vec<String>,
}
