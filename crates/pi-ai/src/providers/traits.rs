use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::messages::types::{
    AssistantMessage, Message, StopReason, ThinkingBudgets, ThinkingLevel,
};
use crate::models::registry::Model;
use crate::streaming::events::StreamEvent;
use crate::tools::schema::ToolDefinition;

// ─── Options ──────────────────────────────────────────────────────────────────

/// Base streaming/completion options common across all providers.
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// Sampling temperature (0.0 – 1.0, or up to 2.0 for some providers).
    pub temperature: Option<f64>,
    /// Maximum tokens to generate.
    pub max_tokens: Option<u64>,
    /// API key override (falls back to env var if `None`).
    pub api_key: Option<String>,
    /// Optional session / request ID for tracing.
    pub session_id: Option<String>,
    /// Extra HTTP headers to include in the request.
    pub headers: Option<HashMap<String, String>>,
    /// Maximum delay between retries in milliseconds.
    pub max_retry_delay_ms: Option<u64>,
}

/// Options that include the high-level thinking abstraction.
#[derive(Debug, Clone, Default)]
pub struct SimpleStreamOptions {
    pub base: StreamOptions,
    /// Desired thinking level (maps to provider-specific budget tokens).
    pub reasoning: Option<ThinkingLevel>,
    /// Custom budget overrides per thinking level.
    pub thinking_budgets: Option<ThinkingBudgets>,
}

// ─── Context ──────────────────────────────────────────────────────────────────

/// The full conversational context passed to a provider.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
}

impl Context {
    pub fn new(messages: Vec<Message>) -> Self {
        Context {
            system_prompt: None,
            messages,
            tools: vec![],
        }
    }

    pub fn with_system(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

// ─── Provider capabilities ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
    pub thinking: bool,
    pub vision: bool,
}

// ─── LLMProvider trait ────────────────────────────────────────────────────────

#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Human-readable name of this provider.
    fn name(&self) -> &str;

    /// What this provider supports.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Stream a response, pushing events to `tx`.
    ///
    /// Implementations must send exactly one terminal event
    /// (`StreamEvent::Done` or `StreamEvent::Error`) as the last event.
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()>;

    /// Stream with high-level thinking-level abstraction.
    ///
    /// Default implementation converts `SimpleStreamOptions` to `StreamOptions`
    /// and calls [`Self::stream`].  Override if the provider handles thinking
    /// natively (e.g. Anthropic extended thinking).
    async fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        self.stream(model, context, &options.base, tx).await
    }

    /// Non-streaming completion — collects the stream into a final message.
    ///
    /// Runs the producer (`stream`) and the consumer (draining `rx`) as two
    /// concurrent futures via `tokio::join!` so neither side blocks the other.
    /// A bounded channel with a generous capacity is used so small responses
    /// don't need to yield at all; large responses are still fine because
    /// the consumer runs concurrently.
    ///
    /// Override this if the provider has a cheaper non-streaming endpoint.
    async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Result<AssistantMessage> {
        use tokio::sync::mpsc;

        // Generous capacity so typical responses never apply backpressure.
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(1024);

        // Concurrently drive the stream producer and drain the consumer.
        // tokio::join! polls both futures on the same task, so when stream()
        // blocks waiting for channel space, the drain future runs and frees
        // capacity, preventing deadlock.
        let (stream_result, final_message) =
            tokio::join!(self.stream(model, context, options, tx), async {
                let mut msg: Option<AssistantMessage> = None;
                while let Some(event) = rx.recv().await {
                    match event {
                        StreamEvent::Done { message, .. }
                        | StreamEvent::Error { error: message, .. } => {
                            msg = Some(message);
                        }
                        _ => {}
                    }
                }
                msg
            });

        stream_result?;
        final_message.ok_or(crate::error::PiAiError::StreamClosed)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Resolve the API key: use `options.api_key` if set, otherwise fall back to
/// the environment-variable lookup.
pub fn resolve_api_key(provider_name: &str, options: &StreamOptions) -> Option<String> {
    if let Some(k) = &options.api_key {
        return Some(k.clone());
    }
    crate::auth::api_key::get_api_key(provider_name)
}

/// Build an initial `AssistantMessage` stub used as the `partial` payload in
/// stream events.
pub fn make_partial(model: &Model) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage: crate::messages::types::Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}
