//! Event processing for Slack bot.

use anyhow::Result;
use pi_agent_core::Agent;
use pi_ai::Content;

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

    /// Process a Slack event by forwarding it to the agent and returning the
    /// assistant's textual response.
    pub async fn process(&self, event: SlackEvent, ctx: SlackContext) -> Result<String> {
        // Get or create channel state
        let channel_state = self.state_manager.get_channel(&ctx.channel);

        // Add user message to conversation
        let user_message = pi_agent_core::messages::AgentMessage::from_llm(pi_ai::Message::user(
            event.text.clone(),
        ));
        channel_state.add_message(user_message)?;

        // Send the user's text to the agent and get the assistant response
        let assistant_msg = self
            .agent
            .prompt(&event.text)
            .await
            .map_err(|e| anyhow::anyhow!("Agent prompt failed: {e}"))?;

        // Extract text from the assistant response Content blocks
        let response: String = assistant_msg
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

        // Store the assistant response as a proper assistant message
        let response_message = pi_agent_core::messages::AgentMessage::from_llm(
            pi_ai::Message::Assistant(assistant_msg),
        );
        channel_state.add_message(response_message)?;

        Ok(response)
    }

    /// Handle file uploads
    pub async fn handle_files(&self, event: &SlackEvent, _ctx: &SlackContext) -> Result<String> {
        if event.files.is_empty() {
            return Ok(String::new());
        }

        let mut responses = Vec::new();
        for file in &event.files {
            responses.push(format!("File: {} ({})", file.name, file.url));
        }

        Ok(responses.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_processor_creation() {
        // Just test that we can create the state manager structure
        // Full agent integration tests would require a configured provider
        let _state_manager = StateManager::new();
    }
}
