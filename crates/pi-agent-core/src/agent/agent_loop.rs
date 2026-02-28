use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};
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
use crate::agent::state::{AgentConfig, AgentSharedState, AgentState};
use crate::context::budget::ContextUsage;
use crate::error::{AgentError, Result};
use crate::messages::{self, AgentMessage};
use crate::tools::traits::{ToolContext, ToolResult as AgentToolResult};

/// The Agent drives the core loop of: prompt → stream → tool execution → repeat.
pub struct Agent {
    pub config: AgentConfig,
    pub shared: Arc<AgentSharedState>,
    /// Conversation messages (stored here so Agent owns them directly)
    messages: tokio::sync::RwLock<Vec<AgentMessage>>,
    current_model: tokio::sync::RwLock<Model>,
    model_cycle: tokio::sync::Mutex<Option<ModelCycleState>>,
    event_tx: broadcast::Sender<AgentEvent>,
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
        Self {
            config,
            shared: Arc::new(AgentSharedState::new()),
            messages: tokio::sync::RwLock::new(Vec::new()),
            current_model: tokio::sync::RwLock::new(current_model),
            model_cycle: tokio::sync::Mutex::new(None),
            event_tx,
        }
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

                // Auto-compaction check
                if self.config.compaction.enabled {
                    let usage = self.context_usage().await;
                    let context_window = self.config.token_budget.context_window;
                    if should_compact(usage.total_tokens, context_window, &self.config.compaction) {
                        self.emit(AgentEvent::AutoCompaction);
                        match self.run_compaction().await {
                            Ok(result) => {
                                tracing::info!(
                                    tokens_before = result.tokens_before,
                                    tokens_after = result.tokens_after,
                                    "Auto-compacted context"
                                );
                            }
                            Err(e) => {
                                tracing::warn!("Auto-compaction failed: {e}");
                            }
                        }
                    }
                }

                // If no tool calls, we're done with this inner loop
                if !has_tool_calls {
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

    /// Build the LLM context from current state
    async fn build_context(&self) -> Context {
        let messages = self.messages.read().await;
        let llm_messages = messages::to_llm_messages(&messages);
        let tools_guard = self.shared.tools.read().await;
        let tool_defs = tools_guard.active_tool_definitions();

        let mut ctx = Context::new(llm_messages).with_tools(tool_defs);
        if let Some(ref prompt) = self.config.system_prompt {
            ctx = ctx.with_system(prompt.clone());
        }
        ctx
    }

    /// Stream a response from the LLM provider
    async fn stream_response(
        &self,
        context: &Context,
        message_id: &str,
    ) -> Result<AssistantMessage> {
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
        let options = SimpleStreamOptions {
            base: pi_ai::StreamOptions::default(),
            reasoning: self.config.thinking_level,
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

        tool.execute(arguments.clone(), &ctx).await
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
    async fn run_compaction(&self) -> Result<CompactionResult> {
        let previous_state = *self.shared.state.read().await;
        *self.shared.state.write().await = AgentState::Compacting;

        let result = self.run_compaction_inner().await;

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
    async fn run_compaction_inner(&self) -> Result<CompactionResult> {
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
        let (system_prompt, user_prompt) =
            build_compaction_prompt(&conversation_text, previous_summary.as_deref());

        // Call LLM for summarization using the non-streaming complete() method
        let summary_context =
            Context::new(vec![Message::user(&user_prompt)]).with_system(system_prompt);
        let options = StreamOptions {
            max_tokens: Some(8192),
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

    /// Emit an event to all subscribers
    fn emit(&self, event: AgentEvent) {
        // broadcast::send returns Err only if there are no receivers, which is fine
        let _ = self.event_tx.send(event);
    }

    /// Get estimated context usage
    pub async fn context_usage(&self) -> ContextUsage {
        let msgs = self.messages.read().await;
        let total_tokens: u64 = msgs.iter().map(|m| messages::estimate_tokens(m)).sum();
        let system_tokens = self
            .config
            .system_prompt
            .as_ref()
            .map(|s| (s.len() as u64) / 4)
            .unwrap_or(0);

        let available = self.config.token_budget.available_for_context();
        let usage_percent = if available > 0 {
            (total_tokens as f64 / available as f64) * 100.0
        } else {
            0.0
        };

        ContextUsage {
            total_tokens: total_tokens + system_tokens,
            system_tokens,
            message_tokens: total_tokens,
            tool_tokens: 0, // TODO: estimate tool definition tokens
            message_count: msgs.len(),
            usage_percent,
        }
    }
}
