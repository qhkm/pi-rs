use serde::{Serialize, Deserialize};

/// Token budget configuration for context management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Maximum context window size (from model)
    pub context_window: u64,
    /// Reserve this many tokens for the response
    pub reserve_for_response: u64,
    /// Reserve this many tokens as buffer before compaction triggers
    pub reserve_for_compaction: u64,
    /// Keep at least this many tokens of recent messages during compaction
    pub keep_recent_tokens: u64,
}

impl TokenBudget {
    pub fn new(context_window: u64) -> Self {
        Self {
            context_window,
            reserve_for_response: 4096,
            reserve_for_compaction: 16384,
            keep_recent_tokens: 20000,
        }
    }

    /// Maximum tokens available for context (messages + system prompt)
    pub fn available_for_context(&self) -> u64 {
        self.context_window.saturating_sub(self.reserve_for_response)
    }

    /// Whether compaction should trigger given current token usage
    pub fn should_compact(&self, current_tokens: u64) -> bool {
        current_tokens + self.reserve_for_compaction >= self.available_for_context()
    }

    /// How many tokens over budget we are (0 if within budget)
    pub fn overflow(&self, current_tokens: u64) -> u64 {
        current_tokens.saturating_sub(self.available_for_context())
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new(128_000) // Default to 128K context
    }
}

/// Current context usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextUsage {
    /// Total estimated tokens in context
    pub total_tokens: u64,
    /// Tokens used by system prompt
    pub system_tokens: u64,
    /// Tokens used by messages
    pub message_tokens: u64,
    /// Tokens used by tool definitions
    pub tool_tokens: u64,
    /// Number of messages in context
    pub message_count: usize,
    /// Percentage of context window used
    pub usage_percent: f64,
}
