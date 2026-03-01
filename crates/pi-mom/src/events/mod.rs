//! Event processing for Slack bot.

use anyhow::Result;
use pi_agent_core::Agent;

use crate::slack::{SlackContext, SlackEvent};
use crate::state::StateManager;

/// Process incoming Slack events
pub struct EventProcessor {
    state_manager: StateManager,
    agent: Agent,
}

impl EventProcessor {
    /// Create a new event processor
    pub fn new(agent: Agent) -> Self {
        Self {
            state_manager: StateManager::new(),
            agent,
        }
    }

    /// Process a Slack event
    pub async fn process(&self, event: SlackEvent, ctx: SlackContext) -> Result<String> {
        // Get or create channel state
        let channel_state = self.state_manager.get_channel(&ctx.channel);

        // Add user message to conversation
        let user_message = pi_agent_core::messages::AgentMessage::from_llm(
            pi_ai::Message::user(event.text.clone())
        );
        channel_state.add_message(user_message)?;

        // Get conversation history
        let history = channel_state.get_messages();

        // TODO: Process with agent and get response
        // For now, return a placeholder
        let response = format!("Received: {}", event.text);

        // Add assistant response to conversation
        // Note: Assistant messages require full AssistantMessage structure
        // This is simplified for the example - in production would use proper message creation
        let assistant_message = pi_agent_core::messages::AgentMessage::from_llm(
            pi_ai::Message::user(format!("[assistant] {}", response))
        );
        channel_state.add_message(assistant_message)?;

        Ok(response)
    }

    /// Handle file uploads
    pub async fn handle_files(&self, event: &SlackEvent, ctx: &SlackContext) -> Result<String> {
        if event.files.is_empty() {
            return Ok(String::new());
        }

        let mut responses = Vec::new();
        for file in &event.files {
            responses.push(format!("📎 File: {} ({})", file.name, file.url));
        }

        Ok(responses.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slack::{SlackEvent, SlackEventType};

    #[test]
    fn test_event_processor_creation() {
        // Just test that we can create the processor structure
        // Full agent integration tests would require a configured provider
        let _state_manager = StateManager::new();
    }
}
