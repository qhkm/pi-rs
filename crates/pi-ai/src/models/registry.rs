use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::messages::types::{Api, Provider, Usage, UsageCost};
use crate::models::cost::calculate_cost;

// ─── Input types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    Text,
    Image,
}

// ─── Model cost (per-million token rates in USD) ──────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

// ─── Model definition ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: Provider,
    pub base_url: String,
    pub reasoning: bool,
    pub input_types: Vec<InputType>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
}

impl Model {
    pub fn calculate_cost(&self, usage: &Usage) -> UsageCost {
        calculate_cost(&self.cost, usage)
    }

    /// Annotate a `Usage` with cost information.
    pub fn annotate_usage(&self, mut usage: Usage) -> Usage {
        usage.cost = self.calculate_cost(&usage);
        usage
    }

    pub fn supports_images(&self) -> bool {
        self.input_types.contains(&InputType::Image)
    }

    pub fn supports_reasoning(&self) -> bool {
        self.reasoning
    }
}

// ─── Built-in model catalogue ─────────────────────────────────────────────────

/// Returns a list of well-known models with their pricing information.
///
/// Pricing is in USD per million tokens and is sourced from public provider
/// pricing pages (as of 2025-Q2).  Always verify against the provider's
/// current pricing for billing-critical applications.
pub fn built_in_models() -> Vec<Model> {
    vec![
        // ── Anthropic ──────────────────────────────────────────────────────
        Model {
            id: "claude-opus-4-5".to_string(),
            name: "Claude Opus 4.5".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 },
            context_window: 200_000,
            max_tokens: 32_000,
            headers: None,
        },
        Model {
            id: "claude-sonnet-4-5".to_string(),
            name: "Claude Sonnet 4.5".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
            context_window: 200_000,
            max_tokens: 16_000,
            headers: None,
        },
        Model {
            id: "claude-haiku-3-5".to_string(),
            name: "Claude Haiku 3.5".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.0 },
            context_window: 200_000,
            max_tokens: 8_096,
            headers: None,
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            name: "Claude Opus 4.6".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 18.75 },
            context_window: 200_000,
            max_tokens: 32_000,
            headers: None,
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            name: "Claude Sonnet 4.6".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
            context_window: 200_000,
            max_tokens: 16_000,
            headers: None,
        },
        // ── OpenAI ────────────────────────────────────────────────────────
        Model {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 2.5, output: 10.0, cache_read: 1.25, cache_write: 0.0 },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
        },
        Model {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o mini".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.15, output: 0.6, cache_read: 0.075, cache_write: 0.0 },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
        },
        Model {
            id: "gpt-4.1".to_string(),
            name: "GPT-4.1".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 2.0, output: 8.0, cache_read: 0.5, cache_write: 0.0 },
            context_window: 1_047_576,
            max_tokens: 32_768,
            headers: None,
        },
        Model {
            id: "gpt-4.1-mini".to_string(),
            name: "GPT-4.1 mini".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.4, output: 1.6, cache_read: 0.1, cache_write: 0.0 },
            context_window: 1_047_576,
            max_tokens: 32_768,
            headers: None,
        },
        Model {
            id: "o3".to_string(),
            name: "OpenAI o3".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 10.0, output: 40.0, cache_read: 2.5, cache_write: 0.0 },
            context_window: 200_000,
            max_tokens: 100_000,
            headers: None,
        },
        Model {
            id: "o4-mini".to_string(),
            name: "OpenAI o4-mini".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenAI,
            base_url: "https://api.openai.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 1.1, output: 4.4, cache_read: 0.275, cache_write: 0.0 },
            context_window: 200_000,
            max_tokens: 100_000,
            headers: None,
        },
        // ── Google ────────────────────────────────────────────────────────
        Model {
            id: "gemini-2.5-pro-preview-05-06".to_string(),
            name: "Gemini 2.5 Pro".to_string(),
            api: Api::GoogleGenerativeAI,
            provider: Provider::Google,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 1.25, output: 10.0, cache_read: 0.31, cache_write: 0.0 },
            context_window: 1_000_000,
            max_tokens: 65_536,
            headers: None,
        },
        Model {
            id: "gemini-2.5-flash-preview-05-20".to_string(),
            name: "Gemini 2.5 Flash".to_string(),
            api: Api::GoogleGenerativeAI,
            provider: Provider::Google,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.15, output: 0.6, cache_read: 0.0375, cache_write: 0.0 },
            context_window: 1_000_000,
            max_tokens: 65_536,
            headers: None,
        },
        Model {
            id: "gemini-2.0-flash".to_string(),
            name: "Gemini 2.0 Flash".to_string(),
            api: Api::GoogleGenerativeAI,
            provider: Provider::Google,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.1, output: 0.4, cache_read: 0.025, cache_write: 0.0 },
            context_window: 1_000_000,
            max_tokens: 8_192,
            headers: None,
        },
        // ── Groq ──────────────────────────────────────────────────────────
        Model {
            id: "llama-3.3-70b-versatile".to_string(),
            name: "Llama 3.3 70B Versatile (Groq)".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::Groq,
            base_url: "https://api.groq.com/openai".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost { input: 0.59, output: 0.79, cache_read: 0.0, cache_write: 0.0 },
            context_window: 128_000,
            max_tokens: 32_768,
            headers: None,
        },
        Model {
            id: "moonshotai/kimi-k2-instruct".to_string(),
            name: "Kimi K2 (Groq)".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::Groq,
            base_url: "https://api.groq.com/openai".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost { input: 1.0, output: 3.0, cache_read: 0.0, cache_write: 0.0 },
            context_window: 131_072,
            max_tokens: 16_384,
            headers: None,
        },
        // ── xAI ───────────────────────────────────────────────────────────
        Model {
            id: "grok-3".to_string(),
            name: "Grok 3".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::XAI,
            base_url: "https://api.x.ai".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 3.0, output: 15.0, cache_read: 0.0, cache_write: 0.0 },
            context_window: 131_072,
            max_tokens: 16_384,
            headers: None,
        },
        Model {
            id: "grok-3-mini".to_string(),
            name: "Grok 3 Mini".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::XAI,
            base_url: "https://api.x.ai".to_string(),
            reasoning: true,
            input_types: vec![InputType::Text],
            cost: ModelCost { input: 0.3, output: 0.5, cache_read: 0.0, cache_write: 0.0 },
            context_window: 131_072,
            max_tokens: 16_384,
            headers: None,
        },
        // ── Mistral ───────────────────────────────────────────────────────
        Model {
            id: "mistral-large-latest".to_string(),
            name: "Mistral Large".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::Mistral,
            base_url: "https://api.mistral.ai".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost { input: 2.0, output: 6.0, cache_read: 0.0, cache_write: 0.0 },
            context_window: 128_000,
            max_tokens: 8_192,
            headers: None,
        },
        // ── OpenRouter ────────────────────────────────────────────────────
        Model {
            id: "meta-llama/llama-4-maverick".to_string(),
            name: "Llama 4 Maverick (OpenRouter)".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::OpenRouter,
            base_url: "https://openrouter.ai/api".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text, InputType::Image],
            cost: ModelCost { input: 0.18, output: 0.6, cache_read: 0.0, cache_write: 0.0 },
            context_window: 524_288,
            max_tokens: 16_384,
            headers: Some({
                let mut h = HashMap::new();
                h.insert("HTTP-Referer".to_string(), "https://github.com/pi-mono-rs".to_string());
                h
            }),
        },
        // ── Cerebras ──────────────────────────────────────────────────────
        Model {
            id: "llama-4-scout-17b-16e-instruct".to_string(),
            name: "Llama 4 Scout (Cerebras)".to_string(),
            api: Api::OpenAICompletions,
            provider: Provider::Cerebras,
            base_url: "https://api.cerebras.ai".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost { input: 0.1, output: 0.1, cache_read: 0.0, cache_write: 0.0 },
            context_window: 128_000,
            max_tokens: 8_192,
            headers: None,
        },
    ]
}

/// Look up a built-in model by its ID.
pub fn find_model(id: &str) -> Option<Model> {
    built_in_models().into_iter().find(|m| m.id == id)
}

/// Returns all built-in models for a given provider.
pub fn models_for_provider(provider: &Provider) -> Vec<Model> {
    built_in_models().into_iter().filter(|m| &m.provider == provider).collect()
}

/// Returns all built-in models that use a given API type.
pub fn models_for_api(api: &Api) -> Vec<Model> {
    built_in_models().into_iter().filter(|m| &m.api == api).collect()
}
