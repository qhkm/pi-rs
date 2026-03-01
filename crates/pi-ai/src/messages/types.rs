use chrono::Utc;
use serde::{Deserialize, Serialize};

// ─── API identifier ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Api {
    OpenAICompletions,
    OpenAIResponses,
    AzureOpenAIResponses,
    AnthropicMessages,
    BedrockConverseStream,
    GoogleGenerativeAI,
    GoogleVertex,
    /// Forward-declared: native Mistral API (currently routed via OpenAI-compatible endpoint).
    MistralNative,
    /// Forward-declared: native Groq API (currently routed via OpenAI-compatible endpoint).
    GroqNative,
    /// Forward-declared: native xAI API (currently routed via OpenAI-compatible endpoint).
    XAINative,
    /// Forward-declared: native Cerebras API (currently routed via OpenAI-compatible endpoint).
    CerebrasNative,
    /// Forward-declared: native OpenRouter API (currently routed via OpenAI-compatible endpoint).
    OpenRouterNative,
    /// Forward-declared: native MiniMax API (currently routed via OpenAI-compatible endpoint).
    MiniMaxNative,
    /// Forward-declared: native HuggingFace API (currently routed via OpenAI-compatible endpoint).
    HuggingFaceNative,
    #[serde(untagged)]
    Custom(String),
}

impl std::fmt::Display for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Api::OpenAICompletions => write!(f, "openai-completions"),
            Api::OpenAIResponses => write!(f, "openai-responses"),
            Api::AzureOpenAIResponses => write!(f, "azure-open-ai-responses"),
            Api::AnthropicMessages => write!(f, "anthropic-messages"),
            Api::BedrockConverseStream => write!(f, "bedrock-converse-stream"),
            Api::GoogleGenerativeAI => write!(f, "google-generative-ai"),
            Api::GoogleVertex => write!(f, "google-vertex"),
            Api::MistralNative => write!(f, "mistral-native"),
            Api::GroqNative => write!(f, "groq-native"),
            Api::XAINative => write!(f, "xai-native"),
            Api::CerebrasNative => write!(f, "cerebras-native"),
            Api::OpenRouterNative => write!(f, "openrouter-native"),
            Api::MiniMaxNative => write!(f, "minimax-native"),
            Api::HuggingFaceNative => write!(f, "huggingface-native"),
            Api::Custom(s) => write!(f, "{s}"),
        }
    }
}

// ─── Provider identifier ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Anthropic,
    OpenAI,
    Google,
    GoogleVertex,
    AmazonBedrock,
    AzureOpenAI,
    #[serde(rename = "x-ai")]
    XAI,
    Groq,
    Cerebras,
    OpenRouter,
    Mistral,
    HuggingFace,
    MiniMax,
    #[serde(untagged)]
    Custom(String),
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Anthropic => write!(f, "anthropic"),
            Provider::OpenAI => write!(f, "open-ai"),
            Provider::Google => write!(f, "google"),
            Provider::GoogleVertex => write!(f, "google-vertex"),
            Provider::AmazonBedrock => write!(f, "amazon-bedrock"),
            Provider::AzureOpenAI => write!(f, "azure-open-ai"),
            Provider::XAI => write!(f, "x-ai"),
            Provider::Groq => write!(f, "groq"),
            Provider::Cerebras => write!(f, "cerebras"),
            Provider::OpenRouter => write!(f, "open-router"),
            Provider::Mistral => write!(f, "mistral"),
            Provider::HuggingFace => write!(f, "hugging-face"),
            Provider::MiniMax => write!(f, "minimax"),
            Provider::Custom(s) => write!(f, "{s}"),
        }
    }
}

// ─── Content blocks ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Content {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },
    Image {
        data: String,
        mime_type: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text {
            text: text.into(),
            text_signature: None,
        }
    }

    pub fn thinking(thinking: impl Into<String>) -> Self {
        Content::Thinking {
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: false,
        }
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Content::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }

    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Content::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            Content::Thinking { thinking, .. } => Some(thinking.as_str()),
            _ => None,
        }
    }

    pub fn is_tool_call(&self) -> bool {
        matches!(self, Content::ToolCall { .. })
    }

    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Content::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
}

// ─── Usage tracking ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: UsageCost,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total_tokens += other.total_tokens;
        self.cost.input += other.cost.input;
        self.cost.output += other.cost.output;
        self.cost.cache_read += other.cost.cache_read;
        self.cost.cache_write += other.cost.cache_write;
        self.cost.total += other.cost.total;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

// ─── Stop reasons ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Stop => write!(f, "stop"),
            StopReason::Length => write!(f, "length"),
            StopReason::ToolUse => write!(f, "toolUse"),
            StopReason::Error => write!(f, "error"),
            StopReason::Aborted => write!(f, "aborted"),
        }
    }
}

// ─── Thinking levels ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ThinkingLevel {
    /// Returns a token budget appropriate for this thinking level.
    ///
    /// The lookup order is:
    /// 1. `budgets.xhigh` / `budgets.high` / etc. if explicitly set
    /// 2. Hard-coded defaults that match the canonical budget table
    ///
    /// For `XHigh`, a value of `0` means "no limit" (provider maximum).
    pub fn to_budget_tokens(self, budgets: &ThinkingBudgets) -> u32 {
        match self {
            ThinkingLevel::Minimal => budgets.minimal.unwrap_or(1_024),
            ThinkingLevel::Low => budgets.low.unwrap_or(4_096),
            ThinkingLevel::Medium => budgets.medium.unwrap_or(10_240),
            ThinkingLevel::High => budgets.high.unwrap_or(32_768),
            ThinkingLevel::XHigh => budgets.xhigh.unwrap_or(0),
        }
    }

    /// Returns all variants in ascending budget order.
    pub fn all() -> &'static [ThinkingLevel] {
        &[
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::XHigh,
        ]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    pub minimal: Option<u32>,
    pub low: Option<u32>,
    pub medium: Option<u32>,
    pub high: Option<u32>,
    /// XHigh budget. `Some(0)` or `None` means "provider maximum / no limit".
    pub xhigh: Option<u32>,
}

// ─── Messages ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: UserContent,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<Content>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    pub api: Api,
    pub provider: Provider,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

impl AssistantMessage {
    pub fn new_partial(api: Api, provider: Provider, model: impl Into<String>) -> Self {
        AssistantMessage {
            content: vec![],
            api,
            provider,
            model: model.into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.as_text())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn thinking(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.as_thinking())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> Vec<&Content> {
        self.content.iter().filter(|c| c.is_tool_call()).collect()
    }

    pub fn has_tool_calls(&self) -> bool {
        self.content.iter().any(|c| c.is_tool_call())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
    pub timestamp: i64,
}

// ─── Unified message enum ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: Utc::now().timestamp_millis(),
        })
    }

    pub fn user_with_images(blocks: Vec<Content>) -> Self {
        Message::User(UserMessage {
            content: UserContent::Blocks(blocks),
            timestamp: Utc::now().timestamp_millis(),
        })
    }

    pub fn text_content(&self) -> String {
        match self {
            Message::User(m) => match &m.content {
                UserContent::Text(t) => t.clone(),
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|c| c.as_text())
                    .collect::<Vec<_>>()
                    .join(""),
            },
            Message::Assistant(m) => m
                .content
                .iter()
                .filter_map(|c| c.as_text())
                .collect::<Vec<_>>()
                .join(""),
            Message::ToolResult(m) => m
                .content
                .iter()
                .filter_map(|c| c.as_text())
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn tool_calls(&self) -> Vec<&Content> {
        match self {
            Message::Assistant(m) => m.tool_calls(),
            _ => vec![],
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            Message::User(m) => m.timestamp,
            Message::Assistant(m) => m.timestamp,
            Message::ToolResult(m) => m.timestamp,
        }
    }

    pub fn is_user(&self) -> bool {
        matches!(self, Message::User(_))
    }

    pub fn is_assistant(&self) -> bool {
        matches!(self, Message::Assistant(_))
    }

    pub fn is_tool_result(&self) -> bool {
        matches!(self, Message::ToolResult(_))
    }

    pub fn as_user(&self) -> Option<&UserMessage> {
        match self {
            Message::User(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_assistant(&self) -> Option<&AssistantMessage> {
        match self {
            Message::Assistant(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_tool_result(&self) -> Option<&ToolResultMessage> {
        match self {
            Message::ToolResult(m) => Some(m),
            _ => None,
        }
    }
}
