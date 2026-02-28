use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc};
use tracing::warn;
use uuid::Uuid;

use pi_ai::{
    AssistantMessage, Content, Context, Message,
    SimpleStreamOptions, StreamEvent, ToolResultMessage,
};

use crate::agent::events::{AgentEndReason, AgentEvent};
use crate::agent::state::{AgentConfig, AgentSharedState, AgentState};
use crate::context::budget::ContextUsage;
use crate::messages::{self, AgentMessage};
use crate::tools::traits::{ToolContext, ToolResult as AgentToolResult};
use crate::error::{AgentError, Result};

/// The Agent drives the core loop of: prompt → stream → tool execution → repeat.
pub struct Agent {
    pub config: AgentConfig,
    pub shared: Arc<AgentSharedState>,
    /// Conversation messages (stored here so Agent owns them directly)
    messages: tokio::sync::RwLock<Vec<AgentMessage>>,
    event_tx: broadcast::Sender<AgentEvent>,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            config,
            shared: Arc::new(AgentSharedState::new()),
            messages: tokio::sync::RwLock::new(Vec::new()),
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

    /// Run the agent with a user prompt. This is the main entry point.
    /// Returns the final assistant message or an error.
    pub async fn prompt(&self, user_text: &str) -> Result<AssistantMessage> {
        let agent_id = Uuid::new_v4().to_string();
        self.emit(AgentEvent::AgentStart { agent_id: agent_id.clone() });
        self.reset_abort();

        // Add user message
        let user_msg = AgentMessage::from_llm(Message::user(user_text));
        self.messages.write().await.push(user_msg);

        let result = self.run_loop(&agent_id).await;

        let reason = match &result {
            Ok(_) => AgentEndReason::Completed,
            Err(AgentError::Aborted) => AgentEndReason::Aborted,
            Err(AgentError::MaxTurns(_)) => AgentEndReason::MaxTurns,
            Err(AgentError::ContextOverflow { .. }) => AgentEndReason::ContextOverflow,
            Err(e) => AgentEndReason::Error(e.to_string()),
        };

        self.emit(AgentEvent::AgentEnd { agent_id, reason });
        *self.shared.state.write().await = AgentState::Idle;
        result
    }

    /// The core agent loop
    async fn run_loop(&self, _agent_id: &str) -> Result<AssistantMessage> {
        let mut turn_index = 0usize;
        let mut last_message: Option<AssistantMessage> = None;

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
            self.shared.total_usage.write().await.add(&assistant_msg.usage);

            // Collect tool calls before moving assistant_msg into a Message
            let has_tool_calls = assistant_msg.has_tool_calls();
            let tool_calls: Vec<(String, String, serde_json::Value)> = assistant_msg
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::ToolCall { id, name, arguments, .. } => {
                        Some((id.clone(), name.clone(), arguments.clone()))
                    }
                    _ => None,
                })
                .collect();

            // Store assistant message
            self.messages.write().await.push(
                AgentMessage::from_llm(Message::Assistant(assistant_msg.clone()))
            );

            last_message = Some(assistant_msg.clone());

            // If no tool calls, we're done with this loop
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
        }

        // Process any follow-up messages
        let follow_ups = self.shared.queue.drain_follow_up().await;
        if !follow_ups.is_empty() {
            let mut msgs = self.messages.write().await;
            for msg in follow_ups {
                msgs.push(msg);
            }
        }

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
        let model = self.config.model.clone();
        let context_clone = context.clone();
        let stream_handle = tokio::spawn(async move {
            provider.stream_simple(&model, &context_clone, &options, tx).await
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
                return Err(AgentError::Other(anyhow::anyhow!("Stream task panicked: {}", e)));
            }
        }

        final_message.ok_or_else(|| AgentError::Other(anyhow::anyhow!("No response from LLM")))
    }

    /// Execute tool calls and return tool result messages
    async fn execute_tools(
        &self,
        tool_calls: &[(String, String, serde_json::Value)],
    ) -> Vec<Message> {
        let mut results = Vec::new();

        for (call_id, tool_name, arguments) in tool_calls {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_name: tool_name.clone(),
                call_id: call_id.clone(),
                arguments: arguments.clone(),
            });

            let start = Instant::now();
            let result = self.execute_single_tool(call_id, tool_name, arguments).await;
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

            // Create tool result message
            let tool_result_msg = Message::ToolResult(ToolResultMessage {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                content: vec![Content::text(&content)],
                details: None,
                is_error,
                timestamp: chrono::Utc::now().timestamp_millis(),
            });

            results.push(tool_result_msg);
        }

        results
    }

    /// Execute a single tool
    async fn execute_single_tool(
        &self,
        _call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<AgentToolResult> {
        let tools_guard = self.shared.tools.read().await;
        let tool = tools_guard
            .get(tool_name)
            .ok_or_else(|| AgentError::ToolNotFound(tool_name.to_string()))?
            .clone();
        drop(tools_guard); // Release lock before executing

        let ctx = ToolContext::new(self.config.cwd.clone())
            .with_abort(self.shared.abort_rx.clone());

        tool.execute(arguments.clone(), &ctx).await
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
