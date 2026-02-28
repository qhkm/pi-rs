use super::SlackEvent;

/// Process an incoming Slack event
pub async fn handle_event(event: &SlackEvent) {
    tracing::info!(
        "Received {} from {} in {}",
        match event.event_type {
            super::SlackEventType::Mention => "mention",
            super::SlackEventType::DirectMessage => "DM",
        },
        event.user,
        event.channel
    );
}
