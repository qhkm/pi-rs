use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::warn;
use uuid::Uuid;

use pi_ai::{
    AssistantMessage, Content, Context, Message, Model, SimpleStreamOptions, StreamEvent,
    StreamOptions, ToolResultMessage,
};

use crate::context::compaction::{
    build_compaction_prompt, find_compaction_split, serialize_conversation, should_compact,
    CompactionResult,
};

use crate::agent::events::{AgentEndReason, AgentEvent};
use crate::agent::hooks::{
    resolve_hook_results, HookContext, HookEvent, HookOutcome, HookRegistry,
};
use crate::agent::state::{default_thinking_budgets, AgentConfig, AgentSharedState, AgentState};
use crate::context::budget::ContextUsage;
use crate::error::{AgentError, Result};
use crate::messages::{self, AgentMessage};
use crate::tools::traits::{ToolContext, ToolProgress, ToolResult as AgentToolResult};

/// A callable that receives the full LLM-visible message list (already
/// converted from `AgentMessage`) right before it is sent to the provider and
/// may mutate it in place.  The list is a *clone* of the stored conversation;
/// mutations here never affect the persisted history.
///
/// # Thread-safety
/// Transforms are stored behind an `Arc<RwLock<…>>` so they can be registered
/// from any thread.  The closure itself must be `Send + Sync`.
pub type ContextTransformFn = Box<dyn Fn(&mut Vec<Message>) + Send + Sync>;

/// The Agent drives the core loop of: prompt -> stream -> tool execution -> repeat.
pub struct Agent {
    pub config: AgentConfig,
    pub shared: Arc<AgentSharedState>,
    /// Conversation messages (stored here so Agent owns them directly)
    messages: tokio::sync::RwLock<Vec<AgentMessage>>,
    current_model: tokio::sync::RwLock<Model>,
    model_cycle: tokio::sync::Mutex<Option<ModelCycleState>>,
    event_tx: broadcast::Sender<AgentEvent>,
    /// Ordered list of context transforms applied to a *clone* of the messages
    /// immediately before each LLM call.  Transforms run in registration order.
    context_transforms: Arc<RwLock<Vec<ContextTransformFn>>>,
    /// Event log file handle (if persistence is enabled)
    event_log: Arc<RwLock<Option<std::fs::File>>>,
    /// Hook registry for extension lifecycle events.
    ///
    /// Extensions register handlers via [`HookRegistry::register`]; the agent
    /// loop dispatches events at key lifecycle points (before/after turn,
    /// before/after compaction, etc.).
    hook_registry: Arc<HookRegistry>,
}

#[derive(Debug, Clone)]
struct ModelCycleState {
    models: Vec<Model>,
    next_index: usize,
    started: bool,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Self {
        let (event_tx, _) = broadcast::channel(4096);
        let current_model = config.model.clone();
        let compaction_enabled = config.compaction.enabled;

        // Initialize event log if path is configured
        let event_log = if let Some(ref path) = config.event_log_path {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => Some(file),
                Err(e) => {
                    tracing::warn!("Failed to open event log: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            config,
            shared: Arc::new(AgentSharedState::new_with_compaction(compaction_enabled)),
            messages: tokio::sync::RwLock::new(Vec::new()),
            current_model: tokio::sync::RwLock::new(current_model),
            model_cycle: tokio::sync::Mutex::new(None),
            event_tx,
            context_transforms: Arc::new(RwLock::new(Vec::new())),
            event_log: Arc::new(RwLock::new(event_log)),
            hook_registry: Arc::new(HookRegistry::new()),
        }
    }

    /// Enable or disable auto-compaction at runtime.
    ///
    /// This updates the shared atomic flag that the agent loop checks after each
    /// LLM turn. The change takes effect on the next turn — it does not interrupt
    /// a compaction that is already in progress.
    pub fn set_auto_compaction(&self, enabled: bool) {
        use std::sync::atomic::Ordering;
        self.shared
            .auto_compaction_enabled
            .store(enabled, Ordering::SeqCst);
    }

    /// Returns whether auto-compaction is currently enabled.
    pub fn auto_compaction_enabled(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.shared.auto_compaction_enabled.load(Ordering::SeqCst)
    }

    /// Register a context transform that will be applied to the LLM-visible
    /// message list right before every provider call.
    ///
    /// Transforms are applied in registration order.  The closure receives a
    /// mutable reference to the *already-converted* `Vec<Message>` (i.e. the
    /// output of [`messages::to_llm_messages`]) and may add, remove, or rewrite
    /// any messages it likes.  The stored conversation is never mutated.
    ///
    /// # Example
    /// ```ignore
    /// agent.register_context_transform(Box::new(|msgs| {
    ///     // Prepend an invisible reminder before every LLM call.
    ///     msgs.insert(0, Message::user("Always respond in English."));
    /// }));
    /// ```
    pub async fn register_context_transform(&self, transform: ContextTransformFn) {
        self.context_transforms.write().await.push(transform);
    }

    /// Return a reference to the hook registry.
    ///
    /// Extensions can use this to register handlers for lifecycle events
    /// (e.g. `BeforeTurn`, `AfterCompact`).  The registry is `Arc`-wrapped
    /// so the returned reference can be cloned and held across async
    /// boundaries.
    pub fn hook_registry(&self) -> &Arc<HookRegistry> {
        &self.hook_registry
    }

    /// Subscribe to agent events
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Get current agent state
    pub async fn state(&self) -> AgentState {
        *self.shared.state.read().await
    }

    /// Get current messages
    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.messages.read().await.clone()
    }

    /// Preload conversation messages before issuing new prompts (e.g. session resume).
    pub async fn preload_messages(&self, messages: Vec<AgentMessage>) {
        self.messages.write().await.extend(messages);
    }

    /// Abort the current operation
    pub fn abort(&self) {
        let _ = self.shared.abort_tx.send(true);
    }

    /// Reset the abort signal (for reuse after abort)
    pub fn reset_abort(&self) {
        let _ = self.shared.abort_tx.send(false);
    }

    /// Register a tool
    pub async fn register_tool(&self, tool: Arc<dyn crate::tools::traits::AgentTool>) {
        self.shared.tools.write().await.register(tool);
    }

    /// Configure model cycling for user prompts.
    ///
    /// The first prompt uses `models[0]`, then subsequent prompts rotate through
    /// the remaining entries in order and wrap around.
    pub async fn configure_model_cycle(&self, models: Vec<Model>) {
        if models.is_empty() {
            return;
        }

        {
            let mut current = self.current_model.write().await;
            *current = models[0].clone();
        }

        let mut cycle = self.model_cycle.lock().await;
        *cycle = if models.len() > 1 {
            Some(ModelCycleState {
                models,
                next_index: 1,
                started: false,
            })
        } else {
            None
        };
    }

    /// Return the ID of the currently active model.
    ///
    /// When a model cycle has been configured, this reflects the model that will
    /// be used for the *next* LLM call.  Before any prompt has been issued (and
    /// before the cycle has started), it is always the model supplied at
    /// construction time (i.e. `models[0]`).
    pub async fn current_model_name(&self) -> String {
        self.current_model.read().await.id.clone()
    }

    /// Immediately advance the model cycle one step forward and return the
    /// new active model's ID.
    ///
    /// If no cycle has been configured (single-model mode) this is a no-op
    /// and returns `None`.
    ///
    /// The returned ID is the same value that [`current_model_name`] would
    /// return after this call.
    pub async fn cycle_model_next(&self) -> Option<String> {
        let mut cycle_guard = self.model_cycle.lock().await;
        let cycle = cycle_guard.as_mut()?;

        // Advance: current becomes models[next_index], next_index moves forward.
        let new_current = cycle.models[cycle.next_index].clone();
        cycle.next_index = (cycle.next_index + 1) % cycle.models.len();
        cycle.started = true;

        // Commit the new current model.
        *self.current_model.write().await = new_current.clone();

        Some(new_current.id)
    }

    /// Immediately step the model cycle one position backward and return the
    /// new active model's ID.
    ///
    /// If no cycle has been configured (single-model mode) this is a no-op
    /// and returns `None`.
    ///
    /// Wraps around: calling `cycle_model_prev` on the first model in the list
    /// moves to the last model.
    pub async fn cycle_model_prev(&self) -> Option<String> {
        let mut cycle_guard = self.model_cycle.lock().await;
        let cycle = cycle_guard.as_mut()?;

        let len = cycle.models.len();

        // The current model sits at index (next_index - 1 + len) % len once
        // the cycle has started.  Going backward means selecting the model
        // one slot before the current one, which is (next_index - 2 + len) % len.
        // We also need next_index to point one past the new current, so it
        // becomes (next_index - 1 + len) % len.
        cycle.next_index = (cycle.next_index + len - 1) % len;
        let prev_index = (cycle.next_index + len - 1) % len;
        let new_current = cycle.models[prev_index].clone();
        cycle.started = true;

        // Commit the new current model.
        *self.current_model.write().await = new_current.clone();

        Some(new_current.id)
    }

    /// Run the agent with a user prompt. This is the main entry point.
    /// Returns the final assistant message or an error.
    pub async fn prompt(&self, user_text: &str) -> Result<AssistantMessage> {
        self.prompt_message(Message::user(user_text)).await
    }

    /// Run the agent with a full user message (supports multimodal content).
    /// Returns the final assistant message or an error.
    pub async fn prompt_message(&self, user_message: Message) -> Result<AssistantMessage> {
        self.select_model_for_next_prompt().await;

        let agent_id = Uuid::new_v4().to_string();
        self.emit(AgentEvent::AgentStart {
            agent_id: agent_id.clone(),
        });
        self.reset_abort();

        // Add user message
        let user_msg = AgentMessage::from_llm(user_message);
        self.messages.write().await.push(user_msg);

        let result = self.run_loop(&agent_id).await;

        let reason = match &result {
            Ok(_) => AgentEndReason::Completed,
            Err(AgentError::Aborted) => AgentEndReason::Aborted,
            Err(AgentError::MaxTurns(_)) => AgentEndReason::MaxTurns,
            Err(AgentError::ContextOverflow { .. }) => AgentEndReason::ContextOverflow,
            Err(e) => AgentEndReason::Error(e.to_string()),
        };

        let final_state = match &reason {
            AgentEndReason::Aborted => AgentState::Aborted,
            _ => AgentState::Idle,
        };
        self.emit(AgentEvent::AgentEnd { agent_id, reason });
        *self.shared.state.write().await = final_state;
        result
    }

    async fn select_model_for_next_prompt(&self) {
        let maybe_next_model = {
            let mut cycle_guard = self.model_cycle.lock().await;
            let Some(cycle) = cycle_guard.as_mut() else {
                return;
            };

            if !cycle.started {
                cycle.started = true;
                None
            } else {
                let next = cycle.models[cycle.next_index].clone();
                cycle.next_index = (cycle.next_index + 1) % cycle.models.len();
                Some(next)
            }
        };

        if let Some(next_model) = maybe_next_model {
            *self.current_model.write().await = next_model;
        }
    }

    /// The core agent loop
    async fn run_loop(&self, agent_id: &str) -> Result<AssistantMessage> {
        tracing::debug!(agent_id = %agent_id, "Agent loop starting");
        let mut turn_index = 0usize;
        let mut last_message: Option<AssistantMessage> = None;

        'outer: loop {
            loop {
                // Check abort
                if *self.shared.abort_rx.borrow() {
                    return Err(AgentError::Aborted);
                }

                // Check max turns
                if self.config.max_turns > 0 && turn_index >= self.config.max_turns {
                    return last_message.ok_or(AgentError::MaxTurns(self.config.max_turns));
                }

                // Check for steering messages
                let steering = self.shared.queue.drain_steering().await;
                if !steering.is_empty() {
                    let mut msgs = self.messages.write().await;
                    for msg in steering {
                        msgs.push(msg);
                    }
                }

                // ── BeforeTurn hook ────────────────────────────────────────
                {
                    let ctx = HookContext {
                        event: HookEvent::BeforeTurn,
                        data: serde_json::json!({ "turn_index": turn_index }),
                    };
                    let results = self.hook_registry.dispatch(&ctx).await;
                    match resolve_hook_results(results) {
                        HookOutcome::Cancelled => {
                            tracing::info!(turn_index, "BeforeTurn hook cancelled the turn");
                            // Skip this turn entirely -- behave as if the model
                            // returned no tool calls so we exit the inner loop.
                            break;
                        }
                        HookOutcome::Modified(_data) => {
                            // Extensions may attach metadata; currently no
                            // mutable turn-level state to override.
                            tracing::debug!(
                                turn_index,
                                "BeforeTurn hook returned Modified (ignored for now)"
                            );
                        }
                        HookOutcome::Continue => {}
                    }
                }

                self.emit(AgentEvent::TurnStart { turn_index });
                *self.shared.state.write().await = AgentState::Streaming;

                // Build context
                let context = self.build_context().await;

                // Stream LLM response
                let message_id = Uuid::new_v4().to_string();
                self.emit(AgentEvent::MessageStart {
                    message_id: message_id.clone(),
                    role: "assistant".to_string(),
                });

                let assistant_msg = self.stream_response(&context, &message_id).await?;

                self.emit(AgentEvent::MessageEnd {
                    message_id: message_id.clone(),
                    usage: Some(assistant_msg.usage.clone()),
                });

                // Accumulate usage
                self.shared
                    .total_usage
                    .write()
                    .await
                    .add(&assistant_msg.usage);

                // Collect tool calls before moving assistant_msg into a Message
                let has_tool_calls = assistant_msg.has_tool_calls();
                let tool_calls: Vec<(String, String, serde_json::Value)> = assistant_msg
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => Some((id.clone(), name.clone(), arguments.clone())),
                        _ => None,
                    })
                    .collect();

                // Store assistant message
                self.messages
                    .write()
                    .await
                    .push(AgentMessage::from_llm(Message::Assistant(
                        assistant_msg.clone(),
                    )));

                last_message = Some(assistant_msg.clone());

                // Auto-compaction check -- read the runtime-mutable flag from shared state
                // rather than the static AgentConfig so that SetAutoCompaction RPC commands
                // take effect without rebuilding the agent.
                if self
                    .shared
                    .auto_compaction_enabled
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    let usage = self.context_usage().await;
                    let context_window = self.config.token_budget.context_window;
                    if should_compact(usage.total_tokens, context_window, &self.config.compaction) {
                        // ── BeforeCompact hook ─────────────────────────────
                        let compact_cancelled = {
                            let ctx = HookContext {
                                event: HookEvent::BeforeCompact,
                                data: serde_json::json!({
                                    "total_tokens": usage.total_tokens,
                                    "context_window": context_window,
                                }),
                            };
                            let results = self.hook_registry.dispatch(&ctx).await;
                            matches!(resolve_hook_results(results), HookOutcome::Cancelled)
                        };

                        if compact_cancelled {
                            tracing::info!("BeforeCompact hook cancelled auto-compaction");
                        } else {
                            self.emit(AgentEvent::AutoCompaction);
                            match self.run_compaction(None).await {
                                Ok(result) => {
                                    tracing::info!(
                                        tokens_before = result.tokens_before,
                                        tokens_after = result.tokens_after,
                                        "Auto-compacted context"
                                    );
                                    // ── AfterCompact hook ──────────────────
                                    let ctx = HookContext {
                                        event: HookEvent::AfterCompact,
                                        data: serde_json::json!({
                                            "tokens_before": result.tokens_before,
                                            "tokens_after": result.tokens_after,
                                            "messages_compacted": result.messages_compacted,
                                        }),
                                    };
                                    let _ = self.hook_registry.dispatch(&ctx).await;
                                }
                                Err(e) => {
                                    tracing::warn!("Auto-compaction failed: {e}");
                                }
                            }
                        }
                    }
                }

                // If no tool calls, we're done with this inner loop
                if !has_tool_calls {
                    // ── AfterTurn hook (no tool calls) ─────────────────────
                    {
                        let ctx = HookContext {
                            event: HookEvent::AfterTurn,
                            data: serde_json::json!({
                                "turn_index": turn_index,
                                "has_tool_calls": false,
                            }),
                        };
                        let _ = self.hook_registry.dispatch(&ctx).await;
                    }

                    self.emit(AgentEvent::TurnEnd {
                        turn_index,
                        message: Some(Message::Assistant(assistant_msg)),
                    });
                    break;
                }

                // Execute tool calls
                *self.shared.state.write().await = AgentState::ExecutingTools;
                let tool_results = self.execute_tools(&tool_calls).await;

                // Add tool results as messages
                {
                    let mut msgs = self.messages.write().await;
                    for result in &tool_results {
                        msgs.push(AgentMessage::from_llm(result.clone()));
                    }
                }

                // ── AfterTurn hook (with tool calls) ───────────────────────
                {
                    let ctx = HookContext {
                        event: HookEvent::AfterTurn,
                        data: serde_json::json!({
                            "turn_index": turn_index,
                            "has_tool_calls": true,
                            "tool_count": tool_calls.len(),
                        }),
                    };
                    let _ = self.hook_registry.dispatch(&ctx).await;
                }

                self.emit(AgentEvent::TurnEnd {
                    turn_index,
                    message: Some(Message::Assistant(assistant_msg)),
                });

                turn_index += 1;
            } // end inner loop

            // Process any follow-up messages; if present, re-enter the inner loop.
            let follow_ups = self.shared.queue.drain_follow_up().await;
            if follow_ups.is_empty() {
                break 'outer;
            }
            {
                let mut msgs = self.messages.write().await;
                for msg in follow_ups {
                    msgs.push(msg);
                }
            }
        } // end 'outer loop

        last_message.ok_or_else(|| AgentError::Other(anyhow::anyhow!("No response generated")))
    }

    /// Build the LLM context from current state.
    ///
    /// Produces a *clone* of the stored messages, converts them to the
    /// `Vec<Message>` format expected by providers, and then runs every
    /// registered [`ContextTransformFn`] over that clone in order.  The
    /// stored conversation is never modified.
    async fn build_context(&self) -> Context {
        let messages = self.messages.read().await;
        let mut llm_messages = messages::to_llm_messages(&messages);
        // Drop the read guard before acquiring the transforms lock to avoid
        // any potential ordering issues with other async tasks.
        drop(messages);

        // Apply registered context transforms to the cloned message list.
        {
            let transforms = self.context_transforms.read().await;
            for transform in transforms.iter() {
                transform(&mut llm_messages);
            }
        }

        let tools_guard = self.shared.tools.read().await;
        let tool_defs = tools_guard.active_tool_definitions();

        let mut ctx = Context::new(llm_messages).with_tools(tool_defs);
        if let Some(ref prompt) = self.config.system_prompt {
            ctx = ctx.with_system(prompt.clone());
        }
        ctx
    }

    /// Resolve the per-request API key from config.
    ///
    /// Priority (highest first):
    /// 1. `api_key_resolver` — invoked on every call; wins when it returns `Some`.
    /// 2. `api_key_override` — static key set at agent construction time.
    /// 3. `None` — the provider falls back to its own default (env var lookup).
    fn resolve_api_key(&self) -> Option<String> {
        if let Some(ref resolver) = self.config.api_key_resolver {
            let resolved = resolver();
            if resolved.is_some() {
                return resolved;
            }
        }
        self.config.api_key_override.clone()
    }

    /// Stream a response from the LLM provider
    async fn stream_response(
        &self,
        context: &Context,
        message_id: &str,
    ) -> Result<AssistantMessage> {
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);

        // Determine thinking level: dynamic selector takes precedence over static config
        let current_messages = self.messages.read().await;
        let dynamic_level = self
            .config
            .thinking_budget_selector
            .as_ref()
            .and_then(|selector| selector(&current_messages));
        drop(current_messages);

        // Use dynamic level if provided, otherwise fall back to static config
        let effective_thinking_level = dynamic_level.or(self.config.thinking_level);

        // Emit event if dynamic thinking level was selected
        if dynamic_level.is_some() {
            self.emit(AgentEvent::DynamicThinkingLevel {
                level: format!("{:?}", effective_thinking_level.unwrap()),
                reason: "Dynamic adjustment based on context".to_string(),
            });
        }

        // Resolve the per-request thinking budget from the config's budget table.
        // `resolved_thinking_budget()` returns None when no thinking level is set,
        // or Some(n) where n=0 means "provider maximum / no cap".
        let thinking_budget = self
            .config
            .thinking_budgets
            .as_ref()
            .and_then(|budgets| {
                effective_thinking_level.and_then(|level| budgets.get(&level).copied())
            })
            .or_else(|| {
                effective_thinking_level
                    .and_then(|level| default_thinking_budgets().get(&level).copied())
            });

        let options = SimpleStreamOptions {
            base: pi_ai::StreamOptions {
                api_key: self.resolve_api_key(),
                thinking_budget,
                session_id: self.config.session_id.clone(),
                ..Default::default()
            },
            reasoning: effective_thinking_level,
            thinking_budgets: None,
        };

        // Spawn the streaming task
        let provider = self.config.provider.clone();
        let model = self.current_model.read().await.clone();
        let context_clone = context.clone();
        let stream_handle = tokio::spawn(async move {
            provider
                .stream_simple(&model, &context_clone, &options, tx)
                .await
        });

        // Collect events
        let mut final_message: Option<AssistantMessage> = None;
        let msg_id = message_id.to_string();

        while let Some(event) = rx.recv().await {
            // Check abort
            if *self.shared.abort_rx.borrow() {
                stream_handle.abort();
                return Err(AgentError::Aborted);
            }

            match &event {
                StreamEvent::Done { message, .. } => {
                    final_message = Some(message.clone());
                }
                StreamEvent::Error { error, .. } => {
                    // Error carries an AssistantMessage with error_message set
                    if let Some(ref err_msg) = error.error_message {
                        let err_text = err_msg.clone();
                        // Drain remaining events then return error
                        while rx.recv().await.is_some() {}
                        stream_handle.abort();
                        return Err(AgentError::Other(anyhow::anyhow!("{}", err_text)));
                    }
                    final_message = Some(error.clone());
                }
                _ => {}
            }

            // Forward as AgentEvent
            self.emit(AgentEvent::MessageUpdate {
                message_id: msg_id.clone(),
                event,
            });
        }

        // Wait for the stream task to complete
        match stream_handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if final_message.is_none() {
                    return Err(AgentError::Ai(e));
                }
                // If we already have a final_message, log the provider error but don't fail
                warn!("Provider returned error after final message: {e}");
            }
            Err(e) => {
                if e.is_cancelled() {
                    return Err(AgentError::Aborted);
                }
                return Err(AgentError::Other(anyhow::anyhow!(
                    "Stream task panicked: {}",
                    e
                )));
            }
        }

        final_message.ok_or_else(|| AgentError::Other(anyhow::anyhow!("No response from LLM")))
    }

    /// Execute tool calls concurrently and return tool result messages in original order.
    ///
    /// Approval checks (which require user interaction) are still performed sequentially
    /// before spawning, because they may need to wait on a single-consumer approval channel.
    /// Once approved (or if no approval is required), tool execution runs concurrently via
    /// `futures::future::join_all`.
    async fn execute_tools(
        &self,
        tool_calls: &[(String, String, serde_json::Value)],
    ) -> Vec<Message> {
        use futures::future::join_all;

        // Emit ToolExecutionStart for every call and build a future per tool.
        // Approval is checked inside execute_single_tool which must remain sequential
        // relative to the approval channel; therefore we run approvals sequentially but
        // the actual tool *execution* (after approval) can be overlapped.
        //
        // Strategy: collect one future per call that does (start_event → execute → end_event)
        // and then join_all them. The approval wait inside execute_single_tool is async-safe
        // but serialises on the shared Mutex<Receiver>; tools that don't need approval run
        // freely in parallel.
        let futures: Vec<_> = tool_calls
            .iter()
            .map(|(call_id, tool_name, arguments)| {
                // Emit start before creating the future so callers see events immediately.
                self.emit(AgentEvent::ToolExecutionStart {
                    tool_name: tool_name.clone(),
                    call_id: call_id.clone(),
                    arguments: arguments.clone(),
                });

                let call_id = call_id.clone();
                let tool_name = tool_name.clone();
                let arguments = arguments.clone();

                async move {
                    let start = Instant::now();
                    let result = self
                        .execute_single_tool(&call_id, &tool_name, &arguments)
                        .await;
                    let duration_ms = start.elapsed().as_millis() as u64;

                    let (content, is_error) = match result {
                        Ok(tool_result) => (tool_result.content, tool_result.is_error),
                        Err(e) => (format!("Error: {e}"), true),
                    };

                    self.emit(AgentEvent::ToolExecutionEnd {
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        result: content.clone(),
                        duration_ms,
                        is_error,
                    });

                    Message::ToolResult(ToolResultMessage {
                        tool_call_id: call_id,
                        tool_name,
                        content: vec![Content::text(&content)],
                        details: None,
                        is_error,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    })
                }
            })
            .collect();

        // Run all tool executions concurrently; join_all preserves input order.
        join_all(futures).await
    }

    /// Execute a single tool, checking for approval if the tool requires it.
    ///
    /// When `tool.requires_approval()` is true:
    /// 1. A `ToolApprovalRequired` event is emitted so the caller can prompt the user.
    /// 2. The method waits (up to 5 minutes) on the approval channel for a matching `call_id`.
    /// 3. If the user denies or the timeout expires, execution is skipped and an error result
    ///    is returned so the agent can communicate the denial back to the LLM.
    async fn execute_single_tool(
        &self,
        call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<AgentToolResult> {
        let tools_guard = self.shared.tools.read().await;
        let tool = tools_guard
            .get(tool_name)
            .ok_or_else(|| AgentError::ToolNotFound(tool_name.to_string()))?
            .clone();
        drop(tools_guard); // Release lock before any await

        // --- Approval gate ---
        if tool.requires_approval() {
            self.emit(AgentEvent::ToolApprovalRequired {
                call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.clone(),
            });

            // Wait up to 5 minutes for an approval decision that matches our call_id.
            const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
            let approved = self.wait_for_approval(call_id, APPROVAL_TIMEOUT).await;

            self.emit(AgentEvent::ToolApprovalResult {
                call_id: call_id.to_string(),
                approved,
            });

            if !approved {
                return Ok(AgentToolResult::error("Tool execution denied by user"));
            }
        }

        let ctx =
            ToolContext::new(self.config.cwd.clone()).with_abort(self.shared.abort_rx.clone());

        // Use streaming execution if enabled
        if self.config.streaming_tool_execution {
            self.execute_tool_streaming(call_id, tool.as_ref(), arguments, &ctx)
                .await
        } else {
            tool.execute(arguments.clone(), &ctx).await
        }
    }

    /// Execute a tool with streaming progress updates.
    async fn execute_tool_streaming(
        &self,
        call_id: &str,
        tool: &dyn crate::tools::traits::AgentTool,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult> {
        let (progress_tx, mut progress_rx) = mpsc::channel::<ToolProgress>(16);

        // Spawn the streaming execution
        let args = arguments.clone();
        let tool_ref = tool.clone_boxed();
        let ctx_clone = ToolContext::new(ctx.cwd.clone()).with_abort(ctx.abort.clone());

        let execution_handle = tokio::spawn(async move {
            tool_ref
                .execute_streaming(args, &ctx_clone, progress_tx)
                .await
        });

        // Collect all progress updates until the channel closes
        while let Some(progress) = progress_rx.recv().await {
            match progress {
                ToolProgress::Started => {
                    // Already emitted ToolExecutionStart
                }
                ToolProgress::Update(msg) => {
                    self.emit(AgentEvent::ToolExecutionUpdate {
                        call_id: call_id.to_string(),
                        progress: msg,
                    });
                }
                ToolProgress::PartialOutput(output) => {
                    self.emit(AgentEvent::ToolExecutionUpdate {
                        call_id: call_id.to_string(),
                        progress: output,
                    });
                }
            }
        }

        // Now await the execution handle to get the final result
        let result = match execution_handle.await {
            Ok(Ok(tool_result)) => tool_result,
            Ok(Err(e)) => AgentToolResult::error(format!("Tool error: {e}")),
            Err(e) => AgentToolResult::error(format!("Execution failed: {e}")),
        };

        Ok(result)
    }

    /// Block until an approval decision for `call_id` arrives or the timeout elapses.
    ///
    /// Decisions for other call IDs received while waiting are re-queued by sending them
    /// back through the channel so they are not lost.
    async fn wait_for_approval(&self, call_id: &str, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        // Buffer for decisions that belong to other call IDs so we can put them back.
        let mut pending_others: Vec<(String, bool)> = Vec::new();

        let result = loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timed out — treat as denied.
                break false;
            }

            let mut rx = self.shared.approval_rx.lock().await;
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some((id, approved))) => {
                    if id == call_id {
                        break approved;
                    }
                    // Decision is for a different call — save it to re-queue later.
                    pending_others.push((id, approved));
                }
                Ok(None) => {
                    // Channel closed — treat as denied.
                    break false;
                }
                Err(_elapsed) => {
                    // Timeout — treat as denied.
                    break false;
                }
            }
        };

        // Re-queue decisions that arrived for other call IDs.
        for (id, approved) in pending_others {
            // Best-effort: if the channel is full we drop rather than block.
            let _ = self.shared.approval_tx.try_send((id, approved));
        }

        result
    }

    /// Send an approval decision for a pending tool call.
    ///
    /// `call_id` must match the `call_id` from the `ToolApprovalRequired` event.
    /// Returns `false` if the approval channel is closed or full.
    pub async fn approve_tool(&self, call_id: &str, approved: bool) -> bool {
        self.shared
            .approval_tx
            .send((call_id.to_string(), approved))
            .await
            .is_ok()
    }

    /// Run context compaction: summarize older messages via an LLM call and
    /// replace them with a single `CompactionSummary` message.
    ///
    /// This is safe to call externally (e.g. from an RPC handler) while the agent
    /// is idle. The caller is responsible for ensuring no concurrent prompt is running.
    pub async fn run_compaction(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult> {
        let previous_state = *self.shared.state.read().await;
        *self.shared.state.write().await = AgentState::Compacting;

        let result = self.run_compaction_inner(custom_instructions).await;

        // Restore previous state (or Idle on success)
        *self.shared.state.write().await = if result.is_ok() {
            previous_state
        } else {
            previous_state
        };

        result
    }

    /// Inner implementation of compaction, separated so state management
    /// can happen in the outer wrapper regardless of errors.
    async fn run_compaction_inner(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult> {
        let messages = self.messages.read().await;
        let llm_messages = messages::to_llm_messages(&messages);

        // Calculate per-message token counts for split-point determination
        let message_tokens: Vec<u64> = messages
            .iter()
            .map(|m| messages::estimate_tokens(m))
            .collect();
        let split_idx =
            find_compaction_split(&message_tokens, self.config.compaction.keep_recent_tokens);

        if split_idx == 0 {
            return Err(AgentError::Compaction(
                "Nothing to compact — all messages are within the keep-recent window".to_string(),
            ));
        }

        // Build the serialized conversation text from messages to compact
        let to_compact = &llm_messages[..split_idx];
        let conversation_text = serialize_conversation(to_compact);

        // Check for a previous compaction summary to do incremental update
        let previous_summary = messages.iter().find_map(|m| {
            if let AgentMessage::CompactionSummary { summary, .. } = m {
                Some(summary.clone())
            } else {
                None
            }
        });

        let tokens_before: u64 = message_tokens.iter().sum();
        drop(messages); // release read lock before the LLM call

        // Build compaction prompt
        let (system_prompt, user_prompt) = build_compaction_prompt(
            &conversation_text,
            previous_summary.as_deref(),
            custom_instructions,
        );

        // Call LLM for summarization using the non-streaming complete() method
        let summary_context =
            Context::new(vec![Message::user(&user_prompt)]).with_system(system_prompt);
        let options = StreamOptions {
            max_tokens: Some(8192),
            // Forward the per-request API key override so compaction calls
            // use the same credentials as normal streaming calls.
            api_key: self.resolve_api_key(),
            session_id: self.config.session_id.clone(),
            ..Default::default()
        };

        self.emit(AgentEvent::AutoCompactionStart {
            reason: format!(
                "Context at {} tokens, compacting {} messages",
                tokens_before, split_idx
            ),
        });

        let model = self.current_model.read().await.clone();
        let summary_msg = self
            .config
            .provider
            .complete(&model, &summary_context, &options)
            .await
            .map_err(|e| {
                self.emit(AgentEvent::AutoCompactionEnd {
                    success: false,
                    tokens_before,
                    tokens_after: None,
                    error: Some(e.to_string()),
                });
                AgentError::Compaction(format!("LLM call failed: {e}"))
            })?;

        // Extract text from the summary response
        let summary_text = summary_msg
            .content
            .iter()
            .filter_map(|c| {
                if let Content::Text { text, .. } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        if summary_text.is_empty() {
            self.emit(AgentEvent::AutoCompactionEnd {
                success: false,
                tokens_before,
                tokens_after: None,
                error: Some("Empty summary from LLM".to_string()),
            });
            return Err(AgentError::Compaction(
                "LLM returned empty summary".to_string(),
            ));
        }

        // Replace compacted messages with the summary
        let mut msgs = self.messages.write().await;
        let kept = msgs.split_off(split_idx);
        let summary_message = AgentMessage::CompactionSummary {
            summary: summary_text.clone(),
            tokens_before,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };
        msgs.clear();
        msgs.push(summary_message);
        msgs.extend(kept);

        let tokens_after: u64 = msgs.iter().map(|m| messages::estimate_tokens(m)).sum();
        drop(msgs);

        self.emit(AgentEvent::AutoCompactionEnd {
            success: true,
            tokens_before,
            tokens_after: Some(tokens_after),
            error: None,
        });

        Ok(CompactionResult {
            summary: summary_text,
            messages_compacted: split_idx,
            tokens_before,
            tokens_after,
            first_kept_id: None,
        })
    }

    /// Emit an event to all subscribers and optionally persist to log.
    fn emit(&self, event: AgentEvent) {
        // Persist to event log first (needs ownership or clone), then broadcast.
        // Only clone when the log is actually configured, avoiding the cost in
        // the common case where no event log is set.
        let has_log = self
            .event_log
            .try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false);

        if has_log {
            // We need the event for both the log and the broadcast channel, so
            // clone once here.
            let log_event = event.clone();
            // Use try_write to avoid blocking the caller. If the lock is
            // contended the event is dropped from the log (but still broadcast).
            // In practice contention is rare since emit() is the only writer.
            if let Ok(guard) = self.event_log.try_write() {
                if let Some(ref file) = *guard {
                    let entry = serde_json::json!({
                        "timestamp": chrono::Utc::now().timestamp_millis(),
                        "event": log_event,
                    });
                    if let Ok(line) = serde_json::to_string(&entry) {
                        use std::io::Write;
                        // `Write` is implemented for `&File`, so we can write
                        // through the shared reference.
                        let _ = (&*file).write_all(line.as_bytes());
                        let _ = (&*file).write_all(b"\n");
                    }
                }
            }
        }

        // broadcast::send returns Err only if there are no receivers, which is fine
        let _ = self.event_tx.send(event);
    }

    /// Get estimated context usage
    pub async fn context_usage(&self) -> ContextUsage {
        let msgs = self.messages.read().await;
        let message_tokens: u64 = msgs.iter().map(|m| messages::estimate_tokens(m)).sum();
        let system_tokens = self
            .config
            .system_prompt
            .as_ref()
            .map(|s| (s.len() as u64) / 4)
            .unwrap_or(0);

        // Estimate tool definition tokens from active tool schemas
        let tools_guard = self.shared.tools.read().await;
        let tool_tokens: u64 = tools_guard
            .active_tool_definitions()
            .iter()
            .map(|t| {
                (t.name.len() + t.description.len() + t.parameters.to_string().len()) as u64 / 4
            })
            .sum();
        drop(tools_guard);

        let total_tokens = message_tokens + system_tokens + tool_tokens;
        let available = self.config.token_budget.available_for_context();
        let usage_percent = if available > 0 {
            (total_tokens as f64 / available as f64) * 100.0
        } else {
            0.0
        };

        ContextUsage {
            total_tokens,
            system_tokens,
            message_tokens,
            tool_tokens,
            message_count: msgs.len(),
            usage_percent,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    // pi_ai re-exports everything from its crate root.
    use pi_ai::{Api, InputType, LLMProvider, ModelCost, Provider, ProviderCapabilities};

    use crate::agent::state::AgentConfig;
    use crate::context::budget::TokenBudget;
    use crate::context::compaction::CompactionSettings;

    // ── Stub provider — never actually called in these tests ──────────────────

    struct StubProvider;

    #[async_trait]
    impl LLMProvider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
            _tx: mpsc::Sender<StreamEvent>,
        ) -> pi_ai::error::Result<()> {
            unimplemented!("StubProvider::stream should not be called in unit tests")
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn stub_model() -> Model {
        Model {
            id: "stub-model".to_string(),
            name: "Stub Model".to_string(),
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            base_url: "https://example.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 4096,
            headers: None,
        }
    }

    fn make_agent() -> Agent {
        let config = AgentConfig {
            provider: Arc::new(StubProvider),
            model: stub_model(),
            system_prompt: None,
            max_turns: 0,
            token_budget: TokenBudget::new(200_000),
            compaction: CompactionSettings::default(),
            thinking_level: None,
            thinking_budgets: None,
            cwd: "/tmp".to_string(),
            api_key_override: None,
            api_key_resolver: None,
            session_id: None,
            event_log_path: None,
            streaming_tool_execution: false,
            thinking_budget_selector: None,
        };
        Agent::new(config)
    }

    // ── Test 1: transform is called and can append a message ─────────────────

    /// Verifies that a registered transform is actually invoked during
    /// `build_context` and that the injected message is visible in the
    /// returned `Context`.
    #[tokio::test]
    async fn transform_appends_message_to_context() {
        let agent = make_agent();

        // Seed the conversation with one user message.
        agent
            .messages
            .write()
            .await
            .push(AgentMessage::from_llm(Message::user("Hello")));

        // Register a transform that appends a sentinel message.
        agent
            .register_context_transform(Box::new(|msgs| {
                msgs.push(Message::user("__sentinel__"));
            }))
            .await;

        let ctx = agent.build_context().await;

        // The context should contain both the original message and the sentinel.
        assert_eq!(ctx.messages.len(), 2, "expected 2 messages after transform");
        assert_eq!(
            ctx.messages[1].text_content(),
            "__sentinel__",
            "transform did not append the expected sentinel message"
        );
    }

    // ── Test 2: stored conversation is unmodified after transform ─────────────

    /// Verifies that the transform operates on a *clone* and never mutates the
    /// agent's internal message store.
    #[tokio::test]
    async fn transform_does_not_modify_stored_messages() {
        let agent = make_agent();

        agent
            .messages
            .write()
            .await
            .push(AgentMessage::from_llm(Message::user("Original")));

        // A transform that replaces every message with something different.
        agent
            .register_context_transform(Box::new(|msgs| {
                msgs.clear();
                msgs.push(Message::user("Replaced"));
            }))
            .await;

        // Calling build_context applies the transform to a clone.
        let ctx = agent.build_context().await;
        assert_eq!(ctx.messages.len(), 1);
        assert_eq!(ctx.messages[0].text_content(), "Replaced");

        // The internal store must still hold the original, untouched message.
        let stored = agent.messages().await;
        assert_eq!(stored.len(), 1, "stored message count should be unchanged");
        assert_eq!(
            stored[0]
                .as_llm()
                .expect("expected Llm variant")
                .text_content(),
            "Original",
            "stored message content should be unchanged after transform"
        );
    }

    // ── Test 3: multiple transforms run in registration order ─────────────────

    /// Verifies that when several transforms are registered they all fire, and
    /// do so in the order they were registered (each seeing the output of the
    /// previous transform).
    #[tokio::test]
    async fn multiple_transforms_run_in_order() {
        let agent = make_agent();

        // Track invocation order via a shared mutex-guarded vec.
        let call_order: Arc<std::sync::Mutex<Vec<u32>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let order_a = Arc::clone(&call_order);
        agent
            .register_context_transform(Box::new(move |msgs| {
                order_a.lock().unwrap().push(1);
                msgs.push(Message::user("__transform_1__"));
            }))
            .await;

        let order_b = Arc::clone(&call_order);
        agent
            .register_context_transform(Box::new(move |msgs| {
                order_b.lock().unwrap().push(2);
                // Assert that transform 1 already ran by inspecting the tail.
                let last = msgs.last().map(|m| m.text_content()).unwrap_or_default();
                assert_eq!(
                    last, "__transform_1__",
                    "transform 2 should see transform 1's appended message"
                );
                msgs.push(Message::user("__transform_2__"));
            }))
            .await;

        let order_c = Arc::clone(&call_order);
        agent
            .register_context_transform(Box::new(move |msgs| {
                order_c.lock().unwrap().push(3);
                msgs.push(Message::user("__transform_3__"));
            }))
            .await;

        let ctx = agent.build_context().await;

        let order = call_order.lock().unwrap().clone();
        assert_eq!(
            order,
            vec![1, 2, 3],
            "transforms must fire in registration order"
        );

        let n = ctx.messages.len();
        assert!(n >= 3, "expected at least 3 messages");
        assert_eq!(ctx.messages[n - 3].text_content(), "__transform_1__");
        assert_eq!(ctx.messages[n - 2].text_content(), "__transform_2__");
        assert_eq!(ctx.messages[n - 1].text_content(), "__transform_3__");
    }

    // ── Test 4: transform call count via atomic counter ───────────────────────

    /// Verifies the exact number of times a transform is invoked across
    /// multiple `build_context` calls (once per call).
    #[tokio::test]
    async fn transform_is_called_once_per_build_context_invocation() {
        let agent = make_agent();

        let call_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&call_count);

        agent
            .register_context_transform(Box::new(move |_msgs| {
                counter.fetch_add(1, Ordering::SeqCst);
            }))
            .await;

        agent.build_context().await;
        agent.build_context().await;
        agent.build_context().await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "transform should be called exactly once per build_context invocation"
        );
    }

    // ── API key override tests ─────────────────────────────────────────────────

    /// A provider that records the `api_key` received in `StreamOptions` and
    /// immediately sends a terminal `Done` event so the agent loop can finish.
    struct KeyCapturingProvider {
        captured_key: Arc<std::sync::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl LLMProvider for KeyCapturingProvider {
        fn name(&self) -> &str {
            "key-capturing"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        async fn stream(
            &self,
            model: &Model,
            _context: &Context,
            options: &StreamOptions,
            tx: mpsc::Sender<StreamEvent>,
        ) -> pi_ai::error::Result<()> {
            // Record whichever api_key the agent injected.
            *self.captured_key.lock().unwrap() = options.api_key.clone();

            // Emit a minimal Done event so stream_response can return.
            use pi_ai::messages::types::{AssistantMessage, StopReason, Usage};
            let msg = AssistantMessage {
                content: vec![],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            let _ = tx
                .send(StreamEvent::Done {
                    reason: StopReason::Stop,
                    message: msg,
                })
                .await;
            Ok(())
        }
    }

    fn make_capturing_agent(
        captured_key: Arc<std::sync::Mutex<Option<String>>>,
        api_key_override: Option<String>,
        api_key_resolver: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
    ) -> Agent {
        let provider = Arc::new(KeyCapturingProvider { captured_key });
        let config = AgentConfig {
            provider,
            model: stub_model(),
            system_prompt: None,
            max_turns: 1,
            token_budget: TokenBudget::new(200_000),
            compaction: CompactionSettings::default(),
            thinking_level: None,
            thinking_budgets: None,
            cwd: "/tmp".to_string(),
            api_key_override,
            api_key_resolver,
            session_id: None,
            event_log_path: None,
            streaming_tool_execution: false,
            thinking_budget_selector: None,
        };
        Agent::new(config)
    }

    // ── Test 5: static api_key_override is forwarded to the provider ──────────

    /// Verifies that when `api_key_override` is set on `AgentConfig`, the value
    /// is propagated all the way to `StreamOptions::api_key` inside the provider.
    #[tokio::test]
    async fn api_key_override_forwarded_to_provider() {
        let captured = Arc::new(std::sync::Mutex::new(None::<String>));

        let agent = make_capturing_agent(
            captured.clone(),
            Some("sk-static-override".to_string()),
            None,
        );

        let _ = agent.prompt("hello").await;

        let key = captured.lock().unwrap().clone();
        assert_eq!(
            key.as_deref(),
            Some("sk-static-override"),
            "Provider should receive the static api_key_override value"
        );
    }

    // ── Test 6: dynamic resolver wins over static override ────────────────────

    /// Verifies that when both `api_key_resolver` and `api_key_override` are set,
    /// the resolver's return value takes priority because it is evaluated first
    /// in `resolve_api_key()`.
    #[tokio::test]
    async fn dynamic_resolver_wins_over_static_override() {
        let captured = Arc::new(std::sync::Mutex::new(None::<String>));

        let resolver: Box<dyn Fn() -> Option<String> + Send + Sync> =
            Box::new(|| Some("sk-dynamic-from-resolver".to_string()));

        let agent = make_capturing_agent(
            captured.clone(),
            Some("sk-static-should-not-be-used".to_string()),
            Some(resolver),
        );

        let _ = agent.prompt("hello").await;

        let key = captured.lock().unwrap().clone();
        assert_eq!(
            key.as_deref(),
            Some("sk-dynamic-from-resolver"),
            "Dynamic resolver should take priority over static api_key_override"
        );
    }

    // ── Model cycling tests ───────────────────────────────────────────────────

    /// Build a stub model with a custom id so we can distinguish models in tests.
    fn named_model(id: &str) -> Model {
        Model {
            id: id.to_string(),
            name: id.to_string(),
            ..stub_model()
        }
    }

    /// Build a three-element cycle [alpha, beta, gamma] on a fresh agent and
    /// return the agent plus the three models.
    async fn make_cycling_agent() -> (Agent, Vec<Model>) {
        let agent = make_agent();
        let models = vec![
            named_model("alpha"),
            named_model("beta"),
            named_model("gamma"),
        ];
        agent.configure_model_cycle(models.clone()).await;
        (agent, models)
    }

    // ── Test 7: cycle_model_next wraps around ─────────────────────────────────

    /// Verifies that repeatedly calling `cycle_model_next` advances through the
    /// full cycle and then wraps back to the beginning.
    #[tokio::test]
    async fn cycle_model_next_wraps_around() {
        let (agent, _) = make_cycling_agent().await;

        // Starting model is always models[0] after configure_model_cycle.
        assert_eq!(agent.current_model_name().await, "alpha");

        // Advance once → beta.
        let next = agent.cycle_model_next().await;
        assert_eq!(next.as_deref(), Some("beta"));
        assert_eq!(agent.current_model_name().await, "beta");

        // Advance again → gamma.
        let next = agent.cycle_model_next().await;
        assert_eq!(next.as_deref(), Some("gamma"));
        assert_eq!(agent.current_model_name().await, "gamma");

        // Advance past the end → wraps back to alpha.
        let next = agent.cycle_model_next().await;
        assert_eq!(next.as_deref(), Some("alpha"));
        assert_eq!(agent.current_model_name().await, "alpha");

        // One more full round-trip.
        let next = agent.cycle_model_next().await;
        assert_eq!(next.as_deref(), Some("beta"));
    }

    // ── Test 8: cycle_model_prev wraps around ─────────────────────────────────

    /// Verifies that `cycle_model_prev` steps backward through the cycle and
    /// wraps from the first model to the last.
    #[tokio::test]
    async fn cycle_model_prev_wraps_around() {
        let (agent, _) = make_cycling_agent().await;

        // Starting model is alpha (index 0).
        assert_eq!(agent.current_model_name().await, "alpha");

        // Going backward from index 0 should wrap to gamma (the last entry).
        let prev = agent.cycle_model_prev().await;
        assert_eq!(prev.as_deref(), Some("gamma"));
        assert_eq!(agent.current_model_name().await, "gamma");

        // One more backward step from gamma → beta.
        let prev = agent.cycle_model_prev().await;
        assert_eq!(prev.as_deref(), Some("beta"));
        assert_eq!(agent.current_model_name().await, "beta");

        // Back to alpha.
        let prev = agent.cycle_model_prev().await;
        assert_eq!(prev.as_deref(), Some("alpha"));
        assert_eq!(agent.current_model_name().await, "alpha");
    }

    // ── Test 9: single-model, cycle ops are no-ops ────────────────────────────

    /// Verifies that when `configure_model_cycle` is called with a single model
    /// (or not called at all), `cycle_model_next` and `cycle_model_prev` both
    /// return `None` and `current_model_name` continues to return the initial
    /// model unchanged.
    #[tokio::test]
    async fn single_model_cycle_ops_are_no_ops() {
        let agent = make_agent();

        // No cycle configured — both ops should be no-ops.
        assert_eq!(agent.current_model_name().await, "stub-model");

        let next = agent.cycle_model_next().await;
        assert!(
            next.is_none(),
            "cycle_model_next should return None when no cycle is configured"
        );
        assert_eq!(agent.current_model_name().await, "stub-model");

        let prev = agent.cycle_model_prev().await;
        assert!(
            prev.is_none(),
            "cycle_model_prev should return None when no cycle is configured"
        );
        assert_eq!(agent.current_model_name().await, "stub-model");

        // Now configure with a single model — cycle state is still None for a
        // one-element list (see configure_model_cycle implementation).
        agent
            .configure_model_cycle(vec![named_model("only-model")])
            .await;

        assert_eq!(agent.current_model_name().await, "only-model");

        let next = agent.cycle_model_next().await;
        assert!(
            next.is_none(),
            "cycle_model_next should return None for a single-model cycle"
        );
        assert_eq!(agent.current_model_name().await, "only-model");

        let prev = agent.cycle_model_prev().await;
        assert!(
            prev.is_none(),
            "cycle_model_prev should return None for a single-model cycle"
        );
        assert_eq!(agent.current_model_name().await, "only-model");
    }

    // ── Test 10: context_usage includes tool_tokens ──────────────────────────

    /// A minimal tool implementation used only for context_usage token estimation.
    struct DummyTool {
        tool_name: String,
        desc: String,
        schema: serde_json::Value,
    }

    #[async_trait]
    impl crate::tools::traits::AgentTool for DummyTool {
        fn name(&self) -> &str {
            &self.tool_name
        }
        fn description(&self) -> &str {
            &self.desc
        }
        fn parameters_schema(&self) -> serde_json::Value {
            self.schema.clone()
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &crate::tools::traits::ToolContext,
        ) -> crate::Result<crate::tools::traits::ToolResult> {
            unimplemented!()
        }
        fn clone_boxed(&self) -> Box<dyn crate::tools::traits::AgentTool> {
            Box::new(DummyTool {
                tool_name: self.tool_name.clone(),
                desc: self.desc.clone(),
                schema: self.schema.clone(),
            })
        }
    }

    /// Verifies that registering tools increases `tool_tokens` in `context_usage`.
    #[tokio::test]
    async fn context_usage_includes_tool_tokens() {
        use serde_json::json;

        let agent = make_agent();

        // Before registering any tool, tool_tokens should be 0.
        let usage = agent.context_usage().await;
        assert_eq!(usage.tool_tokens, 0, "no tools → zero tool_tokens");

        // Register a tool with known sizes.
        let name = "read_file"; // 9 bytes
        let desc = "Read a file from the filesystem"; // 31 bytes
        let schema = json!({"type": "object", "properties": {"path": {"type": "string"}}}); // serialised length varies
        let schema_len = schema.to_string().len();

        let tool = Arc::new(DummyTool {
            tool_name: name.to_string(),
            desc: desc.to_string(),
            schema: schema.clone(),
        });
        agent.register_tool(tool).await;

        let usage = agent.context_usage().await;
        let expected = (name.len() + desc.len() + schema_len) as u64 / 4;
        assert_eq!(
            usage.tool_tokens, expected,
            "tool_tokens should reflect registered tool sizes"
        );
        assert!(
            usage.total_tokens >= usage.tool_tokens,
            "total must include tool_tokens"
        );
    }
}
