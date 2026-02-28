use serde::{Serialize, Deserialize};

/// Known model configurations with recommended settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub context_window: u64,
    pub recommended_gpus: u32,
    pub memory_percent: u32,
    pub tool_calling_template: Option<String>,
}

/// Get config for a known model
pub fn get_model_config(model_id: &str) -> Option<ModelConfig> {
    match model_id {
        "Qwen/Qwen2.5-Coder-32B-Instruct" => Some(ModelConfig {
            model_id: model_id.to_string(),
            context_window: 32768,
            recommended_gpus: 1,
            memory_percent: 90,
            tool_calling_template: Some("hermes".to_string()),
        }),
        "Qwen/Qwen3-Coder-30B-A3B" => Some(ModelConfig {
            model_id: model_id.to_string(),
            context_window: 32768,
            recommended_gpus: 1,
            memory_percent: 90,
            tool_calling_template: Some("hermes".to_string()),
        }),
        _ => None,
    }
}
