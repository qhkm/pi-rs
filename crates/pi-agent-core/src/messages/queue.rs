use super::AgentMessage;
use std::collections::VecDeque;
use tokio::sync::Mutex;

/// Priority for queued messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    /// Steering: interrupt the current turn, inject immediately
    Steering,
    /// FollowUp: queue after the current agent run completes
    FollowUp,
}

/// A queued message with priority
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub message: AgentMessage,
    pub priority: MessagePriority,
}

/// Thread-safe message queue for the agent.
/// Supports steering (interrupt mid-run) and follow-up (after completion) patterns.
pub struct MessageQueue {
    steering: Mutex<VecDeque<AgentMessage>>,
    follow_up: Mutex<VecDeque<AgentMessage>>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self {
            steering: Mutex::new(VecDeque::new()),
            follow_up: Mutex::new(VecDeque::new()),
        }
    }

    /// Push a steering message (interrupts current turn)
    pub async fn push_steering(&self, message: AgentMessage) {
        self.steering.lock().await.push_back(message);
    }

    /// Push a follow-up message (processed after current run)
    pub async fn push_follow_up(&self, message: AgentMessage) {
        self.follow_up.lock().await.push_back(message);
    }

    /// Drain all pending steering messages
    pub async fn drain_steering(&self) -> Vec<AgentMessage> {
        let mut queue = self.steering.lock().await;
        queue.drain(..).collect()
    }

    /// Drain all pending follow-up messages
    pub async fn drain_follow_up(&self) -> Vec<AgentMessage> {
        let mut queue = self.follow_up.lock().await;
        queue.drain(..).collect()
    }

    /// Check if there are pending steering messages
    pub async fn has_steering(&self) -> bool {
        !self.steering.lock().await.is_empty()
    }

    /// Check if there are pending follow-up messages
    pub async fn has_follow_up(&self) -> bool {
        !self.follow_up.lock().await.is_empty()
    }
}

impl Default for MessageQueue {
    fn default() -> Self {
        Self::new()
    }
}
