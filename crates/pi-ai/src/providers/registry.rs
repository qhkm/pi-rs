use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use crate::providers::traits::LLMProvider;

// ─── Global registry ──────────────────────────────────────────────────────────

static REGISTRY: LazyLock<RwLock<HashMap<String, Arc<dyn LLMProvider>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// ─── API ──────────────────────────────────────────────────────────────────────

/// Register a provider under the given API identifier string.
///
/// The `api` key should match the serialised form of `Api` (e.g.
/// `"anthropic-messages"`, `"openai-completions"`).
pub fn register_provider(api: &str, provider: Arc<dyn LLMProvider>) {
    let mut guard = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    guard.insert(api.to_string(), provider);
}

/// Look up a provider by its API identifier.
pub fn get_provider(api: &str) -> Option<Arc<dyn LLMProvider>> {
    let guard = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    guard.get(api).cloned()
}

/// Return all registered providers as `(api_key, provider)` pairs.
pub fn get_providers() -> Vec<(String, Arc<dyn LLMProvider>)> {
    let guard = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Remove the provider registered under `api`, if any.
pub fn unregister_provider(api: &str) {
    let mut guard = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    guard.remove(api);
}

/// Remove all registered providers (useful in tests).
pub fn clear_providers() {
    let mut guard = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    guard.clear();
}

/// Register the built-in providers with their default configuration.
///
/// API keys are read from environment variables.  Call this once at
/// application startup after setting up env vars.
///
/// Returns a list of human-readable warnings for every provider whose
/// required environment variable was not set.  An empty vec means all
/// built-in providers were registered successfully.  Callers should log or
/// display these warnings so users know which providers are unavailable and
/// what they need to configure.
pub fn register_defaults() -> Vec<String> {
    use crate::auth::api_key::get_api_key;
    use crate::messages::types::Api;
    use crate::providers::anthropic::AnthropicProvider;
    use crate::providers::azure::AzureOpenAIProvider;
    use crate::providers::bedrock::BedrockProvider;
    use crate::providers::google::GoogleProvider;
    use crate::providers::openai::OpenAIProvider;
    use crate::providers::vertex::VertexProvider;

    let mut warnings = Vec::new();

    // Anthropic
    if let Some(key) = get_api_key("anthropic") {
        register_provider(
            &Api::AnthropicMessages.to_string(),
            Arc::new(AnthropicProvider::new(key, None)),
        );
    } else {
        warnings.push(
            "Anthropic provider not registered: set ANTHROPIC_API_KEY to enable it.".to_string(),
        );
    }

    // OpenAI
    if let Some(key) = get_api_key("openai") {
        register_provider(
            &Api::OpenAICompletions.to_string(),
            Arc::new(OpenAIProvider::new(key, None, Default::default())),
        );
    } else {
        warnings
            .push("OpenAI provider not registered: set OPENAI_API_KEY to enable it.".to_string());
    }

    // Google
    if let Some(key) = get_api_key("google") {
        register_provider(
            &Api::GoogleGenerativeAI.to_string(),
            Arc::new(GoogleProvider::new(key, None)),
        );
    } else {
        warnings
            .push("Google provider not registered: set GOOGLE_API_KEY to enable it.".to_string());
    }

    // Azure OpenAI
    if let Some(provider) = AzureOpenAIProvider::from_env() {
        register_provider("azure-openai", Arc::new(provider));
    } else {
        warnings.push(
            "Azure OpenAI provider not registered: set AZURE_OPENAI_API_KEY, \
             AZURE_OPENAI_ENDPOINT, and AZURE_OPENAI_DEPLOYMENT to enable it."
                .to_string(),
        );
    }

    // Amazon Bedrock
    if let Some(provider) = BedrockProvider::from_env() {
        register_provider("amazon-bedrock", Arc::new(provider));
    } else {
        warnings.push(
            "Amazon Bedrock provider not registered: set AWS_ACCESS_KEY_ID, \
             AWS_SECRET_ACCESS_KEY, and AWS_REGION to enable it."
                .to_string(),
        );
    }

    // Google Vertex AI
    if let Some(provider) = VertexProvider::from_env() {
        register_provider("google-vertex", Arc::new(provider));
    } else {
        warnings.push(
            "Google Vertex AI provider not registered: set GOOGLE_CLOUD_PROJECT \
             and ensure gcloud ADC is configured to enable it."
                .to_string(),
        );
    }

    warnings
}
