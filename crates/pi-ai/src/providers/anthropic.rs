/// Anthropic Messages API provider.
///
/// Implements the streaming `POST /v1/messages` endpoint with SSE parsing,
/// extended thinking support, and tool-call handling.
use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{PiAiError, Result};
use crate::messages::types::{
    Content, Message, StopReason, ThinkingBudgets, ThinkingLevel, UserContent,
};
use crate::models::registry::Model;
use crate::providers::traits::{
    make_partial, Context, LLMProvider, ProviderCapabilities, SimpleStreamOptions, StreamOptions,
};
use crate::streaming::events::StreamEvent;
use crate::streaming::sse::sse_stream_from_response;
use crate::tools::schema::ToolCall;

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, base_url: Option<&str>) -> Self {
        AnthropicProvider {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build HTTP client"),
            api_key: api_key.into(),
            base_url: base_url.unwrap_or("https://api.anthropic.com").to_string(),
        }
    }

    fn api_key_for(&self, options: &StreamOptions) -> String {
        options
            .api_key
            .clone()
            .unwrap_or_else(|| self.api_key.clone())
    }
}

// ─── Request format conversion ────────────────────────────────────────────────

/// Convert our internal `Message` list to Anthropic's `messages` array.
fn build_anthropic_messages(messages: &[Message]) -> Value {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(um) => {
                let content = match &um.content {
                    UserContent::Text(t) => json!([{"type": "text", "text": t}]),
                    UserContent::Blocks(blocks) => {
                        json!(blocks.iter().map(content_to_anthropic).collect::<Vec<_>>())
                    }
                };
                result.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(am) => {
                let content: Vec<Value> = am.content.iter().map(content_to_anthropic).collect();
                result.push(json!({"role": "assistant", "content": content}));
            }
            Message::ToolResult(tr) => {
                let tool_result_content: Vec<Value> = tr
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text, .. } = c {
                            Some(json!({ "type": "text", "text": text }))
                        } else {
                            None
                        }
                    })
                    .collect();

                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": tr.tool_call_id,
                    "content": tool_result_content,
                    "is_error": tr.is_error,
                });
                // Tool results go in a user turn.
                result.push(json!({"role": "user", "content": [block]}));
            }
        }
    }

    json!(result)
}

fn content_to_anthropic(c: &Content) -> Value {
    match c {
        Content::Text { text, .. } => json!({"type": "text", "text": text}),
        Content::Thinking {
            thinking,
            thinking_signature,
            ..
        } => {
            let mut v = json!({"type": "thinking", "thinking": thinking});
            if let Some(sig) = thinking_signature {
                v["signature"] = json!(sig);
            }
            v
        }
        Content::Image { data, mime_type } => json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": mime_type,
                "data": data,
            }
        }),
        Content::ToolCall {
            id,
            name,
            arguments,
            ..
        } => json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": arguments,
        }),
    }
}

fn build_anthropic_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Value {
    let converted: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();
    json!(converted)
}

fn build_thinking_config(level: ThinkingLevel, budgets: &ThinkingBudgets) -> Value {
    let budget_tokens = level.to_budget_tokens(budgets);
    json!({
        "type": "enabled",
        "budget_tokens": budget_tokens,
    })
}

// ─── SSE event types from Anthropic ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicSseEvent {
    MessageStart {
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: AnthropicContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsageDelta>,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorBody,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    id: String,
    model: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsageDelta {
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    RedactedThinking {},
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ─── Stop reason mapping ──────────────────────────────────────────────────────

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::Stop,
        _ => StopReason::Stop,
    }
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: true,
            vision: true,
        }
    }

    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        self.stream_with_thinking(model, context, options, None, None, tx)
            .await
    }

    async fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        self.stream_with_thinking(
            model,
            context,
            &options.base,
            options.reasoning,
            options.thinking_budgets.as_ref(),
            tx,
        )
        .await
    }
}

impl AnthropicProvider {
    async fn stream_with_thinking(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        thinking: Option<ThinkingLevel>,
        budgets: Option<&ThinkingBudgets>,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let api_key = self.api_key_for(options);

        // ── Build request body ──────────────────────────────────────────
        let messages_value = build_anthropic_messages(&context.messages);
        let max_tokens = options.max_tokens.unwrap_or(model.max_tokens);

        let mut body = json!({
            "model": model.id,
            "messages": messages_value,
            "max_tokens": max_tokens,
            "stream": true,
        });

        if let Some(sp) = &context.system_prompt {
            body["system"] = json!(sp);
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = json!(temp);
        }

        if !context.tools.is_empty() {
            body["tools"] = build_anthropic_tools(&context.tools);
        }

        if let Some(level) = thinking {
            let default_budgets = ThinkingBudgets::default();
            let b = budgets.unwrap_or(&default_budgets);
            body["thinking"] = build_thinking_config(level, b);
            // Extended thinking requires temperature=1
            body["temperature"] = json!(1);
        }

        // ── Extra headers ───────────────────────────────────────────────
        let mut req_builder = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "interleaved-thinking-2025-05-14")
            .header("content-type", "application/json")
            .json(&body);

        if let Some(extra_headers) = &options.headers {
            for (k, v) in extra_headers {
                req_builder = req_builder.header(k, v);
            }
        }

        // ── Fire request ────────────────────────────────────────────────
        let response = req_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Provider {
                provider: "anthropic".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }

        // ── Parse SSE stream ────────────────────────────────────────────
        let mut partial = make_partial(model);
        let mut sse = sse_stream_from_response(response);

        // Per-content-block state.
        struct BlockState {
            kind: BlockKind,
            text_buf: String,
            tool_id: String,
            tool_name: String,
            args_buf: String,
            thinking_sig: Option<String>,
        }

        #[derive(PartialEq)]
        enum BlockKind {
            Text,
            Thinking,
            ToolUse,
            Other,
        }

        let mut blocks: HashMap<usize, BlockState> = HashMap::new();

        // Emit Start event.
        let _ = tx
            .send(StreamEvent::Start {
                partial: partial.clone(),
            })
            .await;

        while let Some(sse_result) = sse.next().await {
            let sse_event = match sse_result {
                Ok(e) => e,
                Err(e) => {
                    warn!("SSE error: {e}");
                    break;
                }
            };

            if sse_event.data.is_empty() {
                continue;
            }

            let event: AnthropicSseEvent = match serde_json::from_str(&sse_event.data) {
                Ok(e) => e,
                Err(e) => {
                    debug!(
                        "Failed to parse Anthropic SSE event: {e} — data: {}",
                        sse_event.data
                    );
                    continue;
                }
            };

            match event {
                AnthropicSseEvent::MessageStart { message } => {
                    partial.usage.input = message.usage.input_tokens;
                    partial.usage.output = message.usage.output_tokens;
                    partial.usage.cache_read = message.usage.cache_read_input_tokens;
                    partial.usage.cache_write = message.usage.cache_creation_input_tokens;
                }

                AnthropicSseEvent::ContentBlockStart {
                    index,
                    content_block,
                } => match content_block {
                    AnthropicContentBlock::Text { .. } => {
                        partial.content.push(Content::Text {
                            text: String::new(),
                            text_signature: None,
                        });
                        blocks.insert(
                            index,
                            BlockState {
                                kind: BlockKind::Text,
                                text_buf: String::new(),
                                tool_id: String::new(),
                                tool_name: String::new(),
                                args_buf: String::new(),
                                thinking_sig: None,
                            },
                        );
                        let _ = tx
                            .send(StreamEvent::TextStart {
                                content_index: index,
                                partial: partial.clone(),
                            })
                            .await;
                    }
                    AnthropicContentBlock::Thinking { .. }
                    | AnthropicContentBlock::RedactedThinking {} => {
                        partial.content.push(Content::Thinking {
                            thinking: String::new(),
                            thinking_signature: None,
                            redacted: matches!(
                                content_block,
                                AnthropicContentBlock::RedactedThinking {}
                            ),
                        });
                        blocks.insert(
                            index,
                            BlockState {
                                kind: BlockKind::Thinking,
                                text_buf: String::new(),
                                tool_id: String::new(),
                                tool_name: String::new(),
                                args_buf: String::new(),
                                thinking_sig: None,
                            },
                        );
                        let _ = tx
                            .send(StreamEvent::ThinkingStart {
                                content_index: index,
                                partial: partial.clone(),
                            })
                            .await;
                    }
                    AnthropicContentBlock::ToolUse { id, name } => {
                        partial.content.push(Content::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: Value::Object(Default::default()),
                            thought_signature: None,
                        });
                        blocks.insert(
                            index,
                            BlockState {
                                kind: BlockKind::ToolUse,
                                text_buf: String::new(),
                                tool_id: id,
                                tool_name: name,
                                args_buf: String::new(),
                                thinking_sig: None,
                            },
                        );
                        let _ = tx
                            .send(StreamEvent::ToolCallStart {
                                content_index: index,
                                partial: partial.clone(),
                            })
                            .await;
                    }
                },

                AnthropicSseEvent::ContentBlockDelta { index, delta } => {
                    let block = match blocks.get_mut(&index) {
                        Some(b) => b,
                        None => continue,
                    };

                    match delta {
                        AnthropicDelta::TextDelta { text } => {
                            block.text_buf.push_str(&text);
                            // Update the content block in partial.
                            if let Some(Content::Text {
                                text: ref mut t, ..
                            }) = partial.content.get_mut(index)
                            {
                                *t = block.text_buf.clone();
                            }
                            let _ = tx
                                .send(StreamEvent::TextDelta {
                                    content_index: index,
                                    delta: text,
                                })
                                .await;
                        }
                        AnthropicDelta::ThinkingDelta { thinking } => {
                            block.text_buf.push_str(&thinking);
                            if let Some(Content::Thinking {
                                thinking: ref mut t,
                                ..
                            }) = partial.content.get_mut(index)
                            {
                                *t = block.text_buf.clone();
                            }
                            let _ = tx
                                .send(StreamEvent::ThinkingDelta {
                                    content_index: index,
                                    delta: thinking,
                                })
                                .await;
                        }
                        AnthropicDelta::SignatureDelta { signature } => {
                            block.thinking_sig = Some(signature);
                        }
                        AnthropicDelta::InputJsonDelta { partial_json } => {
                            block.args_buf.push_str(&partial_json);
                            if let Some(Content::ToolCall {
                                arguments: ref mut a,
                                ..
                            }) = partial.content.get_mut(index)
                            {
                                *a = serde_json::from_str(&block.args_buf)
                                    .unwrap_or(Value::String(block.args_buf.clone()));
                            }
                            let _ = tx
                                .send(StreamEvent::ToolCallDelta {
                                    content_index: index,
                                    delta: partial_json,
                                })
                                .await;
                        }
                    }
                }

                AnthropicSseEvent::ContentBlockStop { index } => {
                    let block = match blocks.remove(&index) {
                        Some(b) => b,
                        None => continue,
                    };

                    match block.kind {
                        BlockKind::Text => {
                            let _ = tx
                                .send(StreamEvent::TextEnd {
                                    content_index: index,
                                    content: block.text_buf,
                                    partial: partial.clone(),
                                })
                                .await;
                        }
                        BlockKind::Thinking => {
                            // Write signature back into the content block.
                            if let Some(sig) = &block.thinking_sig {
                                if let Some(Content::Thinking {
                                    thinking_signature: ref mut ts,
                                    ..
                                }) = partial.content.get_mut(index)
                                {
                                    *ts = Some(sig.clone());
                                }
                            }
                            let _ = tx
                                .send(StreamEvent::ThinkingEnd {
                                    content_index: index,
                                    content: block.text_buf,
                                    partial: partial.clone(),
                                })
                                .await;
                        }
                        BlockKind::ToolUse => {
                            let args: Value = serde_json::from_str(&block.args_buf)
                                .unwrap_or(Value::Object(Default::default()));
                            let tool_call = ToolCall {
                                id: block.tool_id.clone(),
                                name: block.tool_name.clone(),
                                arguments: args.clone(),
                            };
                            // Final update to the content block.
                            if let Some(Content::ToolCall {
                                arguments: ref mut a,
                                ..
                            }) = partial.content.get_mut(index)
                            {
                                *a = args;
                            }
                            let _ = tx
                                .send(StreamEvent::ToolCallEnd {
                                    content_index: index,
                                    tool_call,
                                    partial: partial.clone(),
                                })
                                .await;
                        }
                        BlockKind::Other => {}
                    }
                }

                AnthropicSseEvent::MessageDelta { delta, usage } => {
                    if let Some(reason) = &delta.stop_reason {
                        partial.stop_reason = map_stop_reason(reason);
                    }
                    if let Some(u) = usage {
                        partial.usage.output = u.output_tokens;
                    }
                }

                AnthropicSseEvent::MessageStop => {
                    // Finalise cost.
                    partial.usage = model.annotate_usage(partial.usage.clone());
                    partial.usage.total_tokens = partial.usage.input + partial.usage.output;

                    let reason = partial.stop_reason.clone();
                    if reason == StopReason::Error {
                        let _ = tx
                            .send(StreamEvent::Error {
                                reason: reason.clone(),
                                error: partial.clone(),
                            })
                            .await;
                    } else {
                        let _ = tx
                            .send(StreamEvent::Done {
                                reason,
                                message: partial.clone(),
                            })
                            .await;
                    }
                    return Ok(());
                }

                AnthropicSseEvent::Error { error } => {
                    partial.stop_reason = StopReason::Error;
                    partial.error_message = Some(error.message.clone());
                    let _ = tx
                        .send(StreamEvent::Error {
                            reason: StopReason::Error,
                            error: partial.clone(),
                        })
                        .await;
                    return Err(PiAiError::Provider {
                        provider: "anthropic".into(),
                        message: format!("{}: {}", error.error_type, error.message),
                    });
                }

                AnthropicSseEvent::Ping => {}
            }
        }

        // Stream ended without MessageStop — treat as closed.
        partial.usage = model.annotate_usage(partial.usage.clone());
        let _ = tx
            .send(StreamEvent::Done {
                reason: partial.stop_reason.clone(),
                message: partial,
            })
            .await;

        Ok(())
    }
}
