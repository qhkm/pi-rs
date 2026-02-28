use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};

use pi_ai::{LLMProvider, Model, ThinkingLevel};

use crate::context::budget::TokenBudget;
use crate::context::compaction::CompactionSettings;
use crate::messages::queue::MessageQueue;
use crate::tools::registry::ToolRegistry;

/// Configuration for creating an Agent
pub struct AgentConfig {
    /// The LLM provider to use
    pub provider: Arc<dyn LLMProvider>,
    /// The model to use
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
    /// Current working directory
    pub cwd: String,
}

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
}

impl AgentSharedState {
    pub fn new() -> Self {
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
        }
    }
}

impl Default for AgentSharedState {
    fn default() -> Self {
        Self::new()
    }
}
