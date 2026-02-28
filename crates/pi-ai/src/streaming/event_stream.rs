use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::{mpsc, watch};

use crate::error::{PiAiError, Result};
use crate::messages::types::AssistantMessage;
use crate::streaming::events::StreamEvent;

// ─── Sender side ─────────────────────────────────────────────────────────────

/// The producer end of an [`EventStream`].  Obtained via [`create_event_stream`].
pub struct EventStreamSender {
    tx: mpsc::Sender<StreamEvent>,
    result_tx: watch::Sender<Option<std::result::Result<AssistantMessage, String>>>,
}

impl EventStreamSender {
    /// Send a single stream event to the consumer.
    pub async fn push(&self, event: StreamEvent) -> Result<()> {
        self.tx.send(event).await.map_err(|_| PiAiError::StreamClosed)
    }

    /// Signal successful completion. Closes the event channel after this call.
    pub fn end(self, message: AssistantMessage) {
        // Ignore send errors — the receiver may have already dropped.
        let _ = self.result_tx.send(Some(Ok(message)));
        // `tx` is dropped here, which closes the mpsc channel.
    }

    /// Signal an error. Closes the event channel after this call.
    pub fn end_err(self, error: impl Into<String>) {
        let _ = self.result_tx.send(Some(Err(error.into())));
    }

    /// Returns a clone of the underlying mpsc sender so it can be passed to
    /// provider implementations that only need to push events.
    pub fn mpsc_sender(&self) -> mpsc::Sender<StreamEvent> {
        self.tx.clone()
    }
}

// ─── Receiver side ────────────────────────────────────────────────────────────

/// An async stream of [`StreamEvent`]s produced by an LLM provider.
///
/// Implements [`futures::Stream`] so it can be used with `StreamExt` and in
/// `while let Some(event) = stream.next().await { … }` loops.
pub struct EventStreamReceiver {
    rx: mpsc::Receiver<StreamEvent>,
    result_rx: watch::Receiver<Option<std::result::Result<AssistantMessage, String>>>,
}

impl EventStreamReceiver {
    /// Await the final [`AssistantMessage`] (resolves once the provider calls
    /// [`EventStreamSender::end`]).
    pub async fn result(mut self) -> Result<AssistantMessage> {
        // Drain any remaining events first so the provider can finish.
        while self.rx.recv().await.is_some() {}

        // Wait for the result watch channel to carry a value.
        self.result_rx
            .wait_for(|v| v.is_some())
            .await
            .map_err(|_| PiAiError::StreamClosed)?;

        match self.result_rx.borrow().as_ref() {
            Some(Ok(msg)) => Ok(msg.clone()),
            Some(Err(e)) => Err(PiAiError::Provider {
                provider: "unknown".into(),
                message: e.clone(),
            }),
            None => Err(PiAiError::StreamClosed),
        }
    }

    /// Collect all events into a `Vec`, then resolve the final message.
    /// Useful in tests and non-streaming contexts.
    pub async fn collect_all(mut self) -> Result<(Vec<StreamEvent>, AssistantMessage)> {
        let mut events = Vec::new();
        while let Some(event) = self.rx.recv().await {
            events.push(event);
        }

        self.result_rx
            .wait_for(|v| v.is_some())
            .await
            .map_err(|_| PiAiError::StreamClosed)?;

        let result = match self.result_rx.borrow().as_ref() {
            Some(Ok(msg)) => Ok(msg.clone()),
            Some(Err(e)) => Err(PiAiError::Provider {
                provider: "unknown".into(),
                message: e.clone(),
            }),
            None => Err(PiAiError::StreamClosed),
        };

        result.map(|msg| (events, msg))
    }
}

impl Stream for EventStreamReceiver {
    type Item = StreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

/// Default mpsc channel capacity.  Large enough to buffer a burst of small
/// deltas without backpressure while still bounding memory.
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Create a linked sender/receiver pair for streaming LLM events.
///
/// ```rust,no_run
/// use pi_ai::streaming::event_stream::create_event_stream;
///
/// let (tx, rx) = create_event_stream();
/// // Pass `tx` to a provider; consume events from `rx`.
/// ```
pub fn create_event_stream() -> (EventStreamSender, EventStreamReceiver) {
    let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);
    let (result_tx, result_rx) = watch::channel(None);

    let sender = EventStreamSender { tx, result_tx };
    let receiver = EventStreamReceiver { rx, result_rx };

    (sender, receiver)
}

// ─── Convenience type alias ───────────────────────────────────────────────────

/// Type alias for the concrete event stream used throughout this crate.
pub type AssistantMessageEventStream = EventStreamReceiver;

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::types::{Api, Provider, StopReason, Usage};
    use crate::streaming::events::StreamEvent;
    use futures::StreamExt;

    fn make_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: Api::AnthropicMessages,
            provider: Provider::Anthropic,
            model: "test".into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn test_basic_stream() {
        let (tx, rx) = create_event_stream();

        let msg = make_message();
        let msg_clone = msg.clone();

        tokio::spawn(async move {
            let event = StreamEvent::Start { partial: msg_clone.clone() };
            tx.push(event).await.unwrap();
            let done = StreamEvent::Done { reason: StopReason::Stop, message: msg_clone };
            tx.push(done).await.unwrap();
            tx.end(msg);
        });

        let events: Vec<StreamEvent> = rx.collect().await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_collect_all() {
        let (tx, rx) = create_event_stream();

        let msg = make_message();
        let msg_for_end = msg.clone();

        tokio::spawn(async move {
            let event = StreamEvent::Start { partial: msg.clone() };
            tx.push(event).await.unwrap();
            tx.end(msg_for_end);
        });

        let (events, final_msg) = rx.collect_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(final_msg.model, "test");
    }
}
