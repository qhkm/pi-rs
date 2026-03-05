use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};

use pi_ai::{LLMProvider, Model, ThinkingLevel};

use crate::context::budget::TokenBudget;
use crate::context::compaction::CompactionSettings;
use crate::messages::queue::MessageQueue;
use crate::tools::registry::ToolRegistry;

// ─── Default thinking budgets ─────────────────────────────────────────────────

/// Default token budgets for each `ThinkingLevel`.
///
/// | Level   | Default tokens  |
/// |---------|-----------------|
/// | Minimal | 1 024           |
/// | Low     | 4 096           |
/// | Medium  | 10 240          |
/// | High    | 32 768          |
/// | XHigh   | 0 (no limit)    |
///
/// A value of `0` for `XHigh` signals to the provider that no explicit cap
/// should be imposed (i.e., use the provider's own maximum).
pub fn default_thinking_budgets() -> HashMap<ThinkingLevel, u64> {
    let mut m = HashMap::new();
    m.insert(ThinkingLevel::Minimal, 1_024);
    m.insert(ThinkingLevel::Low, 4_096);
    m.insert(ThinkingLevel::Medium, 10_240);
    m.insert(ThinkingLevel::High, 32_768);
    m.insert(ThinkingLevel::XHigh, 0); // 0 = provider maximum / no explicit cap
    m
}

// ─── AgentConfig ──────────────────────────────────────────────────────────────

/// Configuration for creating an Agent
pub struct AgentConfig {
    /// The LLM provider API identifier to use (e.g., "openai-completions", "anthropic-messages")
    /// Provider is looked up from the global registry on each request, allowing runtime changes.
    pub provider_api: Option<String>,
    /// The model to use (placeholder used if provider not configured)
    pub model: Model,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Maximum turns before stopping (0 = unlimited)
    pub max_turns: usize,
    /// Token budget configuration
    pub token_budget: TokenBudget,
    /// Compaction settings
    pub compaction: CompactionSettings,
    /// Thinking/reasoning level
    pub thinking_level: Option<ThinkingLevel>,
    /// Per-level thinking budgets (token counts).
    ///
    /// Keys are `ThinkingLevel` variants; values are the maximum number of
    /// tokens the provider may use for internal reasoning at that level.
    ///
    /// When `None`, the defaults from `default_thinking_budgets()` are used.
    /// Setting `Some(map)` replaces the entire default table — use
    /// `default_thinking_budgets()` as a starting point and modify as needed.
    ///
    /// A budget of `0` for `XHigh` means "provider maximum / no explicit cap".
    pub thinking_budgets: Option<HashMap<ThinkingLevel, u64>>,
    /// Current working directory
    pub cwd: String,
    /// Static API key override for every request made by this agent.
    ///
    /// When set, this value is forwarded as `StreamOptions::api_key` so the
    /// provider uses it instead of its own default (env-var) key.
    /// Takes lower precedence than `api_key_resolver` when both are set.
    pub api_key_override: Option<String>,
    /// Dynamic API key resolver called before each LLM request.
    ///
    /// The closure is invoked on every streaming call and its return value,
    /// if `Some`, is used as the per-request API key.  This wins over
    /// `api_key_override` when both are present, allowing callers to rotate
    /// or tenant-scope keys at runtime without rebuilding the agent.
    pub api_key_resolver: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
    /// Session ID for cache reuse across requests.
    ///
    /// When set, this is passed to providers that support prompt caching
    /// (e.g., Anthropic) to enable cache hits across multiple turns.
    pub session_id: Option<String>,
    /// Event persistence configuration.
    ///
    /// When set, agent events are written to this path for replay/debugging.
    pub event_log_path: Option<std::path::PathBuf>,
    /// Enable streaming tool execution with progress events.
    ///
    /// When true, tools that support `execute_streaming()` will be called
    /// with progress updates emitted as `ToolExecutionUpdate` events.
    pub streaming_tool_execution: bool,
    /// Dynamic thinking budget selector for per-turn thinking level adjustment.
    ///
    /// When set, this closure is called before each LLM request to determine
    /// the thinking level based on the current context (messages, tools, etc.).
    /// This allows adaptive reasoning based on task complexity.
    pub thinking_budget_selector: Option<
        Box<dyn Fn(&[crate::messages::AgentMessage]) -> Option<ThinkingLevel> + Send + Sync>,
    >,
}

impl AgentConfig {
    /// Look up the token budget for the currently configured `thinking_level`.
    ///
    /// Returns `None` if no thinking level is set.
    /// Returns `Some(0)` when the level is `XHigh` and no explicit budget is
    /// configured (meaning: let the provider use its own maximum).
    pub fn resolved_thinking_budget(&self) -> Option<u64> {
        let level = self.thinking_level?;

        // Prefer the custom map; fall back to defaults.
        let budget = self
            .thinking_budgets
            .as_ref()
            .and_then(|m| m.get(&level).copied())
            .unwrap_or_else(|| *default_thinking_budgets().get(&level).unwrap_or(&0));

        Some(budget)
    }

    /// Get the provider from the global registry.
    /// Looks up by provider_api identifier, allowing runtime provider changes.
    pub fn get_provider(&self) -> Option<Arc<dyn LLMProvider>> {
        self.provider_api
            .as_ref()
            .and_then(|api| pi_ai::get_provider(api))
    }
}

// ─── AgentState ───────────────────────────────────────────────────────────────

/// Current state of the agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is idle, waiting for input
    Idle,
    /// Agent is streaming a response from the LLM
    Streaming,
    /// Agent is executing tools
    ExecutingTools,
    /// Agent is performing compaction
    Compacting,
    /// Agent has been aborted
    Aborted,
}

// ─── AgentSharedState ─────────────────────────────────────────────────────────

/// Shared mutable state for the agent
pub struct AgentSharedState {
    /// Current agent state
    pub state: RwLock<AgentState>,
    /// Tool registry
    pub tools: RwLock<ToolRegistry>,
    /// Message queue for steering/follow-up
    pub queue: MessageQueue,
    /// Abort signal sender
    pub abort_tx: watch::Sender<bool>,
    /// Abort signal receiver (cloneable)
    pub abort_rx: watch::Receiver<bool>,
    /// Cumulative usage
    pub total_usage: RwLock<pi_ai::Usage>,
    /// Approval channel sender — external callers send (call_id, approved) decisions here
    pub approval_tx: mpsc::Sender<(String, bool)>,
    /// Approval channel receiver — the agent loop reads from this to unblock pending approvals
    pub approval_rx: tokio::sync::Mutex<mpsc::Receiver<(String, bool)>>,
    /// Runtime toggle for auto-compaction (overrides AgentConfig::compaction.enabled at runtime).
    ///
    /// Initialized from `AgentConfig::compaction.enabled` at construction time.
    /// Use `Agent::set_auto_compaction` to change this at runtime without rebuilding the agent.
    pub auto_compaction_enabled: Arc<AtomicBool>,
}

impl AgentSharedState {
    pub fn new_with_compaction(compaction_enabled: bool) -> Self {
        let (abort_tx, abort_rx) = watch::channel(false);
        let (approval_tx, approval_rx) = mpsc::channel(16);
        Self {
            state: RwLock::new(AgentState::Idle),
            tools: RwLock::new(ToolRegistry::new()),
            queue: MessageQueue::new(),
            abort_tx,
            abort_rx,
            total_usage: RwLock::new(pi_ai::Usage::default()),
            approval_tx,
            approval_rx: tokio::sync::Mutex::new(approval_rx),
            auto_compaction_enabled: Arc::new(AtomicBool::new(compaction_enabled)),
        }
    }

    pub fn new() -> Self {
        Self::new_with_compaction(true)
    }
}

impl Default for AgentSharedState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal stub so we can construct AgentConfig without a real provider.
    use async_trait::async_trait;
    use pi_ai::models::registry::built_in_models;
    use pi_ai::{Context, LLMProvider, Model, ProviderCapabilities, StreamEvent, StreamOptions};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    struct NoopProvider;

    #[async_trait]
    impl LLMProvider for NoopProvider {
        fn name(&self) -> &str {
            "noop"
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }
        async fn stream(
            &self,
            _model: &Model,
            _ctx: &Context,
            _opts: &StreamOptions,
            _tx: mpsc::Sender<StreamEvent>,
        ) -> pi_ai::Result<()> {
            Ok(())
        }
    }

    fn make_config(
        level: Option<ThinkingLevel>,
        budgets: Option<HashMap<ThinkingLevel, u64>>,
    ) -> AgentConfig {
        let model = built_in_models()
            .into_iter()
            .next()
            .expect("at least one built-in model")
            .clone();
        AgentConfig {
            provider: Arc::new(NoopProvider),
            model,
            system_prompt: None,
            max_turns: 0,
            token_budget: crate::context::budget::TokenBudget::default(),
            compaction: crate::context::compaction::CompactionSettings::default(),
            thinking_level: level,
            thinking_budgets: budgets,
            cwd: ".".to_string(),
            api_key_override: None,
            api_key_resolver: None,
            session_id: None,
            event_log_path: None,
            streaming_tool_execution: false,
            thinking_budget_selector: None,
        }
    }

    /// No thinking level → resolved budget is None.
    #[test]
    fn no_thinking_level_returns_none() {
        let cfg = make_config(None, None);
        assert_eq!(cfg.resolved_thinking_budget(), None);
    }

    /// Default budgets are applied when no custom map is provided.
    #[test]
    fn default_budgets_are_used_when_no_custom_map() {
        let cases = [
            (ThinkingLevel::Minimal, 1_024u64),
            (ThinkingLevel::Low, 4_096),
            (ThinkingLevel::Medium, 10_240),
            (ThinkingLevel::High, 32_768),
            (ThinkingLevel::XHigh, 0), // 0 = provider max
        ];

        for (level, expected) in cases {
            let cfg = make_config(Some(level), None);
            assert_eq!(
                cfg.resolved_thinking_budget(),
                Some(expected),
                "level={level:?}"
            );
        }
    }

    /// A custom map overrides individual entries; unset entries fall back to defaults.
    #[test]
    fn custom_budget_overrides_defaults() {
        let mut custom = HashMap::new();
        custom.insert(ThinkingLevel::Medium, 20_000u64);

        // Medium should use 20 000; Minimal should still use the default 1 024.
        let cfg_medium = make_config(Some(ThinkingLevel::Medium), Some(custom.clone()));
        assert_eq!(cfg_medium.resolved_thinking_budget(), Some(20_000));

        let cfg_minimal = make_config(Some(ThinkingLevel::Minimal), Some(custom));
        assert_eq!(cfg_minimal.resolved_thinking_budget(), Some(1_024));
    }

    /// `default_thinking_budgets()` contains all five ThinkingLevel variants.
    #[test]
    fn default_budgets_map_covers_all_levels() {
        let defaults = default_thinking_budgets();
        for level in ThinkingLevel::all() {
            assert!(
                defaults.contains_key(level),
                "missing default for {level:?}"
            );
        }
    }
}
