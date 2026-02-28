//! `pi-ai` — unified LLM API layer.
//!
//! This crate provides:
//! - **Message types** (`messages`) — provider-agnostic conversation primitives
//! - **Tool definitions** (`tools`) — JSON Schema-backed tool calling
//! - **Streaming** (`streaming`) — SSE parsing and event streams
//! - **Models** (`models`) — model registry with pricing
//! - **Providers** (`providers`) — Anthropic, OpenAI-compatible, and Google
//! - **Auth** (`auth`) — API key resolution from environment variables
//! - **Error types** (`error`) — unified error enum

pub mod auth;
pub mod error;
pub mod messages;
pub mod models;
pub mod providers;
pub mod streaming;
pub mod tools;

// ─── Top-level re-exports ─────────────────────────────────────────────────────

// Error types
pub use error::{PiAiError, Result};

// Core message types
pub use messages::types::{
    Api, AssistantMessage, Content, Message, Provider, StopReason, ThinkingBudgets, ThinkingLevel,
    ToolResultMessage, Usage, UsageCost, UserContent, UserMessage,
};

// Message helpers
pub use messages::transform::{transform_messages, TransformOptions};
pub use messages::{tool_result_message, user_message};

// Tool types
pub use tools::schema::{ToolCall, ToolDefinition, ToolResult};

// Streaming
pub use streaming::event_stream::{
    create_event_stream, AssistantMessageEventStream, EventStreamReceiver, EventStreamSender,
};
pub use streaming::events::StreamEvent;
pub use streaming::sse::{SseEvent, SseStream};

// Models
pub use models::registry::{built_in_models, find_model, InputType, Model, ModelCost};

// Providers
pub use providers::traits::{Context, LLMProvider, ProviderCapabilities, SimpleStreamOptions, StreamOptions};
pub use providers::{AnthropicProvider, GoogleProvider, OpenAICompat, OpenAIProvider};
pub use providers::registry::{
    clear_providers, get_provider, get_providers, register_defaults, register_provider,
    unregister_provider,
};

// Auth
pub use auth::api_key::{get_api_key, is_valid_api_key, require_api_key};
