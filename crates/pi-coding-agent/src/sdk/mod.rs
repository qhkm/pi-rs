//! SDK mode — programmatic API for embedding the coding agent in other Rust
//! programs without the CLI or TUI overhead.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use pi_coding_agent::sdk::{AgentSession, SessionOptions};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let session = AgentSession::create(SessionOptions {
//!         provider: "anthropic".to_string(),
//!         model: "claude-sonnet-4-5".to_string(),
//!         system_prompt: None,
//!         cwd: std::env::current_dir()?.display().to_string(),
//!         thinking_level: None,
//!         max_turns: 50,
//!     })?;
//!
//!     session.send_message("List all Rust files in the current directory.").await?;
//!
//!     for msg in session.messages().await {
//!         println!("{:?}", msg);
//!     }
//!     Ok(())
//! }
//! ```

use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::broadcast;

use pi_agent_core::{Agent, AgentConfig, AgentEvent, AgentState};
use pi_agent_core::context::budget::TokenBudget;
use pi_agent_core::context::compaction::CompactionSettings;
use pi_agent_core::messages::AgentMessage;
use pi_ai::ThinkingLevel;

use crate::tools::operations::LocalFileOps;
use crate::tools::FileOperations;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for creating an [`AgentSession`] programmatically.
///
/// All fields are plain data values so that callers don't need to import
/// pi-agent-core or pi-ai types directly.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// Provider name: `"anthropic"`, `"openai"`, or `"google"`.
    pub provider: String,
    /// Model ID accepted by the chosen provider, e.g. `"claude-sonnet-4-5"`.
    pub model: String,
    /// Override the system prompt.  When `None` the default coding-assistant
    /// prompt is used, augmented by any AGENTS.md / CLAUDE.md files found in
    /// `cwd`.
    pub system_prompt: Option<String>,
    /// Absolute path that the agent will treat as its working directory.
    pub cwd: String,
    /// Optional reasoning/thinking level forwarded to the provider.
    pub thinking_level: Option<ThinkingLevel>,
    /// Maximum number of agent turns before stopping (0 = unlimited).
    /// Defaults to 50.
    pub max_turns: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            system_prompt: None,
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            thinking_level: None,
            max_turns: 50,
        }
    }
}

/// An SDK handle for interacting with a coding agent programmatically.
///
/// Create one with [`AgentSession::create`], then call [`send_message`] to run
/// the agent on a prompt.  The underlying [`Agent`] is identical to the one
/// used by the CLI — the same seven built-in tools are registered and the same
/// context-loading logic applies.
///
/// # Thread safety
/// `AgentSession` is `Send + Sync` and may be shared across tasks via an
/// `Arc<AgentSession>`.  The wrapped [`Agent`] uses internal `RwLock`s / atomic
/// state so concurrent reads are safe; only one `send_message` call should be
/// outstanding at a time (the agent itself enforces this through its abort/state
/// machinery).
pub struct AgentSession {
    agent: Agent,
}

impl AgentSession {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new agent session with the given options.
    ///
    /// This function:
    /// 1. Registers all default LLM providers via [`pi_ai::register_defaults`].
    /// 2. Looks up the requested provider (returns an error if not available,
    ///    typically because the corresponding `*_API_KEY` env-var is not set).
    /// 3. Resolves the model from the global registry.
    /// 4. Builds the system prompt by loading AGENTS.md / CLAUDE.md context
    ///    files found in `options.cwd` (same logic as the CLI).
    /// 5. Constructs an [`Agent`] and registers the seven built-in tools:
    ///    `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`.
    ///
    /// # Errors
    /// Returns an error if the provider is unknown / unavailable, or if the
    /// model ID cannot be resolved.
    pub fn create(options: SessionOptions) -> Result<Self, anyhow::Error> {
        // Register all providers backed by env-var API keys.
        pi_ai::register_defaults();

        // Resolve provider.
        let provider = pi_ai::get_provider(&options.provider).ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{}' not available. \
                 Set the corresponding API key env-var \
                 (e.g. ANTHROPIC_API_KEY for 'anthropic').",
                options.provider
            )
        })?;

        // Resolve model.
        let model = pi_ai::find_model(&options.model)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found.", options.model))?;

        // Build system prompt (load context files from cwd).
        let cwd_path = std::path::Path::new(&options.cwd);
        let loaded_context = crate::context::resource_loader::load_context(cwd_path)
            .with_context(|| {
                format!("Failed to load context files from '{}'", options.cwd)
            })?;

        const DEFAULT_PROMPT: &str =
            "You are a helpful AI coding assistant. You have access to tools \
             for reading, writing, and editing files, running bash commands, \
             and searching code.";

        let system_prompt = crate::context::resource_loader::build_system_prompt(
            &loaded_context,
            options.system_prompt.as_deref(),
            DEFAULT_PROMPT,
        );

        // Build AgentConfig.
        let config = AgentConfig {
            provider,
            model,
            system_prompt: Some(system_prompt),
            max_turns: options.max_turns,
            token_budget: TokenBudget::default(),
            compaction: CompactionSettings::default(),
            thinking_level: options.thinking_level,
            cwd: options.cwd.clone(),
            api_key_override: None,
            api_key_resolver: None,
            thinking_budgets: None,
            session_id: None,
            event_log_path: None,
            streaming_tool_execution: false,
            thinking_budget_selector: None,
        };

        let agent = Agent::new(config);

        // Register the seven built-in tools (same as main.rs).
        // We need a synchronous constructor here, so spin up a one-shot
        // runtime just for tool registration — or use block_in_place if we
        // are already inside a Tokio runtime.
        //
        // Using `futures::executor::block_on` keeps this fn sync while still
        // driving the async register_tool calls.
        futures::executor::block_on(Self::register_builtin_tools(&agent))?;

        Ok(Self { agent })
    }

    /// Internal helper: register the seven built-in tools onto `agent`.
    async fn register_builtin_tools(agent: &Agent) -> Result<(), anyhow::Error> {
        let ops: Arc<dyn FileOperations> = Arc::new(LocalFileOps);

        agent
            .register_tool(Arc::new(crate::tools::read::ReadTool::new(ops.clone())))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::write::WriteTool::new(ops.clone())))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::edit::EditTool::new(ops.clone())))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::bash::BashTool::new()))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::grep::GrepTool::new()))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::find::FindTool::new()))
            .await;
        agent
            .register_tool(Arc::new(crate::tools::ls::LsTool::new()))
            .await;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Interaction
    // -----------------------------------------------------------------------

    /// Send a text message and wait for the agent to finish processing.
    ///
    /// This drives the full agent loop: LLM streaming → tool execution →
    /// repeat until the assistant produces a final response or max turns is
    /// reached.
    ///
    /// # Errors
    /// Propagates any [`pi_agent_core::AgentError`] (converted to
    /// [`anyhow::Error`]) that the agent encounters, including provider errors,
    /// max-turns exhaustion, and explicit aborts.
    pub async fn send_message(&self, message: &str) -> Result<(), anyhow::Error> {
        self.agent
            .prompt(message)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Send a pre-built [`pi_ai::Message`] and wait for the agent to finish.
    ///
    /// Useful for multimodal inputs where the caller needs to attach image
    /// content blocks alongside text.
    pub async fn send_message_raw(
        &self,
        message: pi_ai::Message,
    ) -> Result<(), anyhow::Error> {
        self.agent
            .prompt_message(message)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Control
    // -----------------------------------------------------------------------

    /// Abort the current operation.
    ///
    /// This sends an abort signal through the agent's internal watch channel.
    /// The running loop checks the signal between turns and tool calls; it may
    /// not terminate immediately.  After an abort the session can be reused —
    /// the abort flag is reset at the start of the next [`send_message`] call.
    pub fn abort(&self) {
        self.agent.abort();
    }

    // -----------------------------------------------------------------------
    // Observation
    // -----------------------------------------------------------------------

    /// Get the current agent state.
    pub async fn state(&self) -> AgentState {
        self.agent.state().await
    }

    /// Get all messages in the conversation (user, assistant, tool results).
    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.agent.messages().await
    }

    /// Subscribe to the real-time event stream.
    ///
    /// Returns a [`broadcast::Receiver`] that delivers [`AgentEvent`]s as the
    /// agent runs.  Multiple subscribers are supported; each gets an
    /// independent copy of every event.  Messages sent before the subscription
    /// is created are not replayed.
    ///
    /// # Usage
    /// ```rust,no_run
    /// # use pi_coding_agent::sdk::{AgentSession, SessionOptions};
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// # let session = AgentSession::create(SessionOptions::default())?;
    /// let mut rx = session.subscribe();
    /// tokio::spawn(async move {
    ///     while let Ok(event) = rx.recv().await {
    ///         println!("event: {:?}", event);
    ///     }
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.agent.subscribe()
    }

    /// Access the underlying [`Agent`] directly for advanced use-cases such as
    /// preloading messages, registering additional tools, or applying context
    /// transforms.
    ///
    /// Prefer the typed helper methods (`send_message`, `abort`, …) over this
    /// escape hatch where possible.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use pi_ai::{
        Context, LLMProvider, Model, ProviderCapabilities, StreamEvent, StreamOptions,
    };
    use pi_ai::models::registry::built_in_models;

    // ------------------------------------------------------------------
    // Minimal no-op provider / model helpers
    // ------------------------------------------------------------------

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

    /// Register a "test" provider backed by our no-op implementation so that
    /// `pi_ai::get_provider("test")` works without any env-var.
    fn register_test_provider() {
        // pi_ai::register_provider inserts into a global registry. Calling it
        // multiple times with the same name is idempotent (the last write wins
        // but both are identical).
        pi_ai::register_provider("test", Arc::new(NoopProvider));
    }

    /// Return the first built-in model ID available, for tests that need a
    /// concrete model string.
    fn any_built_in_model() -> String {
        built_in_models()
            .into_iter()
            .next()
            .expect("at least one built-in model must exist")
            .id
            .clone()
    }

    // ------------------------------------------------------------------
    // Test: create session with a valid (test) provider and model
    // ------------------------------------------------------------------

    #[test]
    fn create_session_with_valid_config_succeeds() {
        register_test_provider();
        let model_id = any_built_in_model();

        let result = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: model_id,
            system_prompt: Some("Be terse.".to_string()),
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 10,
        });

        assert!(
            result.is_ok(),
            "Expected Ok but got: {:?}",
            result.err()
        );
    }

    // ------------------------------------------------------------------
    // Test: create session with an unknown provider returns an error
    // ------------------------------------------------------------------

    #[test]
    fn create_session_with_invalid_provider_returns_error() {
        // Make sure defaults are registered so the registry is initialised.
        pi_ai::register_defaults();
        let model_id = any_built_in_model();

        let result = AgentSession::create(SessionOptions {
            provider: "nonexistent_provider_xyz".to_string(),
            model: model_id,
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 50,
        });

        assert!(result.is_err(), "Expected Err for unknown provider");

        // Extract the error without requiring AgentSession: Debug.
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("nonexistent_provider_xyz"),
            "Error should mention the provider name; got: {err_msg}"
        );
    }

    // ------------------------------------------------------------------
    // Test: create session with an unknown model returns an error
    // ------------------------------------------------------------------

    #[test]
    fn create_session_with_invalid_model_returns_error() {
        register_test_provider();

        let result = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: "nonexistent-model-abc-xyz-999".to_string(),
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 50,
        });

        assert!(result.is_err(), "Expected Err for unknown model");

        // Extract the error without requiring AgentSession: Debug.
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("nonexistent-model-abc-xyz-999"),
            "Error should mention the model name; got: {err_msg}"
        );
    }

    // ------------------------------------------------------------------
    // Test: subscribe returns a working receiver
    // ------------------------------------------------------------------

    #[test]
    fn subscribe_returns_broadcast_receiver() {
        register_test_provider();
        let model_id = any_built_in_model();

        let session = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: model_id,
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 5,
        })
        .expect("session creation should succeed");

        // Two independent receivers — just verifying the call doesn't panic.
        let _rx1 = session.subscribe();
        let _rx2 = session.subscribe();
    }

    // ------------------------------------------------------------------
    // Test: initial state is Idle
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn initial_state_is_idle() {
        register_test_provider();
        let model_id = any_built_in_model();

        let session = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: model_id,
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 5,
        })
        .expect("session creation should succeed");

        assert_eq!(session.state().await, AgentState::Idle);
    }

    // ------------------------------------------------------------------
    // Test: messages() is empty before any prompts
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn messages_empty_before_any_prompts() {
        register_test_provider();
        let model_id = any_built_in_model();

        let session = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: model_id,
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 5,
        })
        .expect("session creation should succeed");

        assert!(session.messages().await.is_empty());
    }

    // ------------------------------------------------------------------
    // Test: abort() does not panic when no operation is running
    // ------------------------------------------------------------------

    #[test]
    fn abort_when_idle_does_not_panic() {
        register_test_provider();
        let model_id = any_built_in_model();

        let session = AgentSession::create(SessionOptions {
            provider: "test".to_string(),
            model: model_id,
            system_prompt: None,
            cwd: std::env::temp_dir().display().to_string(),
            thinking_level: None,
            max_turns: 5,
        })
        .expect("session creation should succeed");

        // Should not panic.
        session.abort();
    }

    // ------------------------------------------------------------------
    // Test: SessionOptions::default() is usable
    // ------------------------------------------------------------------

    #[test]
    fn session_options_default_is_sound() {
        let opts = SessionOptions::default();
        assert_eq!(opts.provider, "anthropic");
        assert_eq!(opts.model, "claude-sonnet-4-5");
        assert_eq!(opts.max_turns, 50);
        assert!(opts.thinking_level.is_none());
        assert!(opts.system_prompt.is_none());
        // cwd should be a non-empty path string.
        assert!(!opts.cwd.is_empty());
    }
}
