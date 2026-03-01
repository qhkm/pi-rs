//! Slack Socket Mode WebSocket client.
//!
//! Implements real-time event handling via Slack's Socket Mode API.

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{SlackContext, SlackEvent, SlackEventType};

/// Slack Socket Mode connection manager
pub struct SocketModeClient {
    app_token: String,
    bot_token: String,
    event_handler: Option<Arc<dyn EventHandler>>,
}

/// Trait for handling Slack events
#[async_trait::async_trait]
pub trait EventHandler: Send + Sync {
    async fn on_event(&self, event: SlackEvent, ctx: SlackContext);
    async fn on_connect(&self);
    async fn on_disconnect(&self);
}

/// WebSocket message types from Slack
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum WsMessage {
    Hello {
        #[serde(rename = "connection_info")]
        connection_info: ConnectionInfo,
    },
    Disconnect {
        reason: Option<String>,
        #[serde(rename = "debug_info")]
        debug_info: Option<serde_json::Value>,
    },
    EventsApi {
        envelope_id: String,
        payload: EventPayload,
    },
    Interactive {
        envelope_id: String,
        payload: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
struct ConnectionInfo {
    app_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct EventPayload {
    event: serde_json::Value,
    #[serde(rename = "event_id")]
    event_id: String,
    #[serde(rename = "event_time")]
    event_time: i64,
}

/// Acknowledgment response for events
#[derive(Debug, Clone, Serialize)]
struct AckMessage {
    envelope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<serde_json::Value>,
}

/// Slack API response for apps.connections.open
#[derive(Debug, Clone, Deserialize)]
struct OpenConnectionResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

impl SocketModeClient {
    pub fn new(app_token: String, bot_token: String) -> Self {
        Self {
            app_token,
            bot_token,
            event_handler: None,
        }
    }

    /// Set the event handler
    pub fn with_handler<H: EventHandler + 'static>(mut self, handler: H) -> Self {
        self.event_handler = Some(Arc::new(handler));
        self
    }

    /// Connect and start listening for events.
    ///
    /// Automatically reconnects with exponential backoff (1s → 2s → 4s … 60s)
    /// when the WebSocket connection drops or encounters errors.
    pub async fn connect(&self) -> Result<()> {
        let mut backoff_secs = 1u64;
        const MAX_BACKOFF_SECS: u64 = 60;

        loop {
            match self.connect_once().await {
                Ok(()) => {
                    tracing::info!("WebSocket session ended cleanly");
                    // Reset backoff on clean session
                    backoff_secs = 1;
                }
                Err(e) => {
                    tracing::error!("WebSocket connection failed: {}", e);
                }
            }

            tracing::info!("Reconnecting in {}s...", backoff_secs);
            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }
    }

    /// Single connection attempt — returns when the WebSocket closes or errors.
    async fn connect_once(&self) -> Result<()> {
        let ws_url = self.get_websocket_url().await?;
        tracing::info!("Connecting to Slack Socket Mode: {}", ws_url);

        let (ws_stream, _) = connect_async(&ws_url)
            .await
            .context("Failed to connect to Slack WebSocket")?;

        tracing::info!("WebSocket connected");

        let (mut write, mut read) = ws_stream.split();
        let (tx, mut rx) = mpsc::channel::<Message>(100);

        // Spawn writer task
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let Err(e) = write.send(msg).await {
                    tracing::error!("WebSocket send error: {}", e);
                    break;
                }
            }
        });

        // Notify handler of connection
        if let Some(handler) = &self.event_handler {
            handler.on_connect().await;
        }

        // Main event loop
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_message(&text, &tx).await {
                        tracing::error!("Error handling message: {}", e);
                    }
                }
                Ok(Message::Close(frame)) => {
                    tracing::info!("WebSocket closed: {:?}", frame);
                    break;
                }
                Ok(Message::Ping(data)) => {
                    let _ = tx.send(Message::Pong(data)).await;
                }
                Err(e) => {
                    tracing::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // Notify handler of disconnection
        if let Some(handler) = &self.event_handler {
            handler.on_disconnect().await;
        }

        drop(tx);
        let _ = writer_handle.await;

        Ok(())
    }

    /// Get WebSocket URL from Slack API
    async fn get_websocket_url(&self) -> Result<String> {
        let client = reqwest::Client::new();
        let response: OpenConnectionResponse = client
            .post("https://slack.com/api/apps.connections.open")
            .header("Authorization", format!("Bearer {}", self.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .context("Failed to request WebSocket URL")?
            .json()
            .await
            .context("Failed to parse Slack response")?;

        if !response.ok {
            anyhow::bail!(
                "Slack API error: {}",
                response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string())
            );
        }

        response.url.context("No WebSocket URL in response")
    }

    /// Handle incoming WebSocket message
    async fn handle_message(&self, text: &str, tx: &mpsc::Sender<Message>) -> Result<()> {
        let msg: WsMessage =
            serde_json::from_str(text).context("Failed to parse WebSocket message")?;

        match msg {
            WsMessage::Hello { connection_info } => {
                tracing::info!("Connected to Slack app: {}", connection_info.app_id);
            }
            WsMessage::Disconnect { reason, debug_info } => {
                tracing::warn!(
                    "Slack requested disconnect: {:?}, debug: {:?}",
                    reason,
                    debug_info
                );
            }
            WsMessage::EventsApi {
                envelope_id,
                payload,
            } => {
                // Send acknowledgment
                let ack = AckMessage {
                    envelope_id: envelope_id.clone(),
                    payload: None,
                };
                let ack_json = serde_json::to_string(&ack)?;
                tx.send(Message::Text(ack_json)).await?;

                // Process the event
                self.process_event(payload.event).await;
            }
            WsMessage::Interactive { envelope_id, .. } => {
                // Acknowledge interactive events
                let ack = AckMessage {
                    envelope_id,
                    payload: None,
                };
                let ack_json = serde_json::to_string(&ack)?;
                tx.send(Message::Text(ack_json)).await?;
            }
            WsMessage::Unknown => {
                tracing::debug!("Received unknown message type");
            }
        }

        Ok(())
    }

    /// Process a Slack event payload
    async fn process_event(&self, event: serde_json::Value) {
        let event_type = event.get("type").and_then(|t| t.as_str());

        let slack_event = match event_type {
            Some("app_mention") => self.parse_mention_event(event),
            Some("message") => self.parse_message_event(event),
            _ => {
                tracing::debug!("Unhandled event type: {:?}", event_type);
                return;
            }
        };

        if let Some(event) = slack_event {
            if let Some(handler) = &self.event_handler {
                let ctx = SlackContext {
                    channel: event.channel.clone(),
                    thread_ts: event.ts.clone().into(),
                    user: event.user.clone(),
                };
                handler.on_event(event, ctx).await;
            }
        }
    }

    /// Parse app_mention event
    fn parse_mention_event(&self, event: serde_json::Value) -> Option<SlackEvent> {
        Some(SlackEvent {
            event_type: SlackEventType::Mention,
            channel: event.get("channel")?.as_str()?.to_string(),
            ts: event.get("ts")?.as_str()?.to_string(),
            user: event.get("user")?.as_str()?.to_string(),
            text: event.get("text")?.as_str()?.to_string(),
            files: self.parse_files(&event),
        })
    }

    /// Parse message event (for DMs)
    fn parse_message_event(&self, event: serde_json::Value) -> Option<SlackEvent> {
        // Skip bot messages and message subtypes
        if event.get("bot_id").is_some() || event.get("subtype").is_some() {
            return None;
        }

        // Check if this is a DM (im channel)
        let channel_type = event
            .get("channel_type")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        if channel_type != "im" {
            return None;
        }

        Some(SlackEvent {
            event_type: SlackEventType::DirectMessage,
            channel: event.get("channel")?.as_str()?.to_string(),
            ts: event.get("ts")?.as_str()?.to_string(),
            user: event.get("user")?.as_str()?.to_string(),
            text: event.get("text")?.as_str()?.to_string(),
            files: self.parse_files(&event),
        })
    }

    /// Parse file attachments from event
    fn parse_files(&self, event: &serde_json::Value) -> Vec<super::SlackFile> {
        event
            .get("files")
            .and_then(|f| f.as_array())
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        Some(super::SlackFile {
                            name: f.get("name")?.as_str()?.to_string(),
                            url: f.get("url_private")?.as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Send a message to a Slack channel
    pub async fn send_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<()> {
        let client = reqwest::Client::new();
        let mut payload = json!({
            "channel": channel,
            "text": text,
        });

        if let Some(ts) = thread_ts {
            payload["thread_ts"] = json!(ts);
        }

        let response = client
            .post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send Slack message")?;

        let result: serde_json::Value = response.json().await?;

        if !result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            let error = result
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("Slack API error: {}", error);
        }

        Ok(())
    }
}

/// Default event handler that logs events
pub struct DefaultEventHandler;

#[async_trait::async_trait]
impl EventHandler for DefaultEventHandler {
    async fn on_event(&self, event: SlackEvent, _ctx: SlackContext) {
        tracing::info!(
            "Received Slack event: {:?} from user {} in channel {}",
            event.event_type,
            event.user,
            event.channel
        );
    }

    async fn on_connect(&self) {
        tracing::info!("Connected to Slack Socket Mode");
    }

    async fn on_disconnect(&self) {
        tracing::warn!("Disconnected from Slack Socket Mode");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ws_message_hello() {
        let json = r#"{"type":"hello","connection_info":{"app_id":"A123"}}"#;
        let msg: WsMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsMessage::Hello { connection_info } => {
                assert_eq!(connection_info.app_id, "A123");
            }
            _ => panic!("Expected Hello message"),
        }
    }

    #[test]
    fn test_ack_message_serialization() {
        let ack = AckMessage {
            envelope_id: "env-123".to_string(),
            payload: None,
        };
        let json = serde_json::to_string(&ack).unwrap();
        assert!(json.contains("env-123"));
    }
}
