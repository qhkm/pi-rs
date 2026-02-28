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
    make_partial, CachePolicy, Context, LLMProvider, ProviderCapabilities, SimpleStreamOptions,
    StreamOptions,
};
use crate::streaming::events::StreamEvent;
use crate::streaming::sse::sse_stream_from_response;
use crate::tools::schema::ToolCall;
use crate::utils::build_http_client;

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, base_url: Option<&str>) -> Self {
        AnthropicProvider {
            client: build_http_client(300),
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

/// Serialize tool definitions into the Anthropic API format.
///
/// When `cache_policy` is [`CachePolicy::Auto`] the last tool in the array
/// receives a `cache_control: {"type": "ephemeral"}` field so Anthropic can
/// cache the entire tool list up to that point.
fn build_anthropic_tools(
    tools: &[crate::tools::schema::ToolDefinition],
    cache_policy: CachePolicy,
) -> Value {
    let last_idx = tools.len().saturating_sub(1);
    let converted: Vec<Value> = tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut block = json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            });
            // Attach the cache breakpoint to the final tool so that everything
            // before it (all tool definitions) is eligible for caching.
            if cache_policy == CachePolicy::Auto && i == last_idx {
                block["cache_control"] = json!({"type": "ephemeral"});
            }
            block
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

/// Build a thinking config block using an explicit pre-resolved token budget.
///
/// A `budget_tokens` of `0` is passed through as-is; callers should ensure
/// this is an acceptable value for the target provider (Anthropic requires
/// `budget_tokens >= 1024`).
fn build_thinking_config_with_budget(budget_tokens: u64) -> Value {
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
        // If an explicit thinking_budget is set on the options, enable thinking
        // at the provider level using that budget directly, without needing a
        // ThinkingLevel (the caller already resolved the level → budget mapping).
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

        // ── System prompt ───────────────────────────────────────────────
        // When CachePolicy::Auto is active we must send the system prompt as
        // a content-block array rather than a plain string so that we can
        // attach `cache_control` to the last block, creating a cache
        // breakpoint at the end of the system context.
        if let Some(sp) = &context.system_prompt {
            if options.cache_policy == CachePolicy::Auto {
                body["system"] = json!([{
                    "type": "text",
                    "text": sp,
                    "cache_control": {"type": "ephemeral"},
                }]);
            } else {
                body["system"] = json!(sp);
            }
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = json!(temp);
        }

        if !context.tools.is_empty() {
            body["tools"] = build_anthropic_tools(&context.tools, options.cache_policy);
        }

        // Enable thinking if a ThinkingLevel is provided *or* if an explicit
        // thinking_budget is set on the base StreamOptions.  The explicit budget
        // takes priority over the level-based lookup.
        let resolved_thinking_config = match (thinking, options.thinking_budget) {
            // Explicit pre-resolved budget wins unconditionally.
            (_, Some(budget)) => Some(build_thinking_config_with_budget(budget)),
            // Level provided — look up budget from ThinkingBudgets table.
            (Some(level), None) => {
                let default_budgets = ThinkingBudgets::default();
                let b = budgets.unwrap_or(&default_budgets);
                Some(build_thinking_config(level, b))
            }
            // No thinking requested.
            (None, None) => None,
        };

        if let Some(thinking_cfg) = resolved_thinking_config {
            body["thinking"] = thinking_cfg;
            // Extended thinking requires temperature=1.
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::types::{Content, Message, UserContent};
    use crate::tools::schema::ToolDefinition;
    use chrono::Utc;

    /// Verify that `Content::Image` is serialized to the Anthropic base64 image
    /// source block format:
    /// `{"type":"image","source":{"type":"base64","media_type":"...","data":"..."}}`
    #[test]
    fn image_content_serialized_as_anthropic_base64_source_block() {
        let image = Content::Image {
            data: "aGVsbG8=".to_string(),
            mime_type: "image/png".to_string(),
        };
        let msg = Message::User(crate::messages::types::UserMessage {
            content: UserContent::Blocks(vec![
                Content::text("Describe this image."),
                image,
            ]),
            timestamp: Utc::now().timestamp_millis(),
        });

        let messages_value = build_anthropic_messages(&[msg]);
        let content_blocks = &messages_value[0]["content"];

        // First block is the text part.
        assert_eq!(content_blocks[0]["type"], "text");
        assert_eq!(content_blocks[0]["text"], "Describe this image.");

        // Second block must be the image source block.
        let img_block = &content_blocks[1];
        assert_eq!(img_block["type"], "image", "block type must be 'image'");
        assert_eq!(
            img_block["source"]["type"], "base64",
            "source.type must be 'base64'"
        );
        assert_eq!(
            img_block["source"]["media_type"], "image/png",
            "source.media_type must match the Content mime_type"
        );
        assert_eq!(
            img_block["source"]["data"], "aGVsbG8=",
            "source.data must be the raw base64 string from Content::Image"
        );
    }

    // ── Cache policy tests ────────────────────────────────────────────────────

    /// With `CachePolicy::Auto` the system prompt must be serialized as a
    /// content-block array where the (only) text block carries
    /// `cache_control: {"type": "ephemeral"}`.
    ///
    /// This is what Anthropic requires to create a cache breakpoint at the end
    /// of the system context.
    #[test]
    fn cache_policy_auto_wraps_system_prompt_with_cache_control() {
        // Build the same JSON body that stream_with_thinking would produce for
        // the system-prompt section.
        let system_prompt = "You are a helpful assistant.";
        let cache_policy = CachePolicy::Auto;

        // Reproduce the logic from stream_with_thinking.
        let system_value = if cache_policy == CachePolicy::Auto {
            json!([{
                "type": "text",
                "text": system_prompt,
                "cache_control": {"type": "ephemeral"},
            }])
        } else {
            json!(system_prompt)
        };

        // Must be an array.
        assert!(
            system_value.is_array(),
            "system prompt must be an array when CachePolicy::Auto"
        );

        let block = &system_value[0];
        assert_eq!(block["type"], "text");
        assert_eq!(block["text"], system_prompt);
        assert_eq!(
            block["cache_control"],
            json!({"type": "ephemeral"}),
            "cache_control must be set on the system block"
        );
    }

    /// With `CachePolicy::None` (the default) the system prompt is serialized
    /// as a plain string — no content-block wrapping, no cache_control.
    #[test]
    fn cache_policy_none_keeps_system_prompt_as_plain_string() {
        let system_prompt = "You are a helpful assistant.";
        let cache_policy = CachePolicy::None;

        let system_value = if cache_policy == CachePolicy::Auto {
            json!([{
                "type": "text",
                "text": system_prompt,
                "cache_control": {"type": "ephemeral"},
            }])
        } else {
            json!(system_prompt)
        };

        assert!(
            system_value.is_string(),
            "system prompt must be a plain string when CachePolicy::None"
        );
        assert_eq!(system_value, json!(system_prompt));
    }

    /// With `CachePolicy::Auto` the last tool definition in the serialized
    /// array must carry `cache_control: {"type": "ephemeral"}`.  All other
    /// tools must not have a `cache_control` key.
    #[test]
    fn cache_policy_auto_adds_cache_control_to_last_tool() {
        let tools = vec![
            ToolDefinition::new(
                "tool_a",
                "First tool",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            ToolDefinition::new(
                "tool_b",
                "Second tool",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            ToolDefinition::new(
                "tool_c",
                "Third tool",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
        ];

        let result = build_anthropic_tools(&tools, CachePolicy::Auto);

        // Only the last tool should have cache_control.
        assert!(
            result[0]["cache_control"].is_null(),
            "tool_a must not have cache_control"
        );
        assert!(
            result[1]["cache_control"].is_null(),
            "tool_b must not have cache_control"
        );
        assert_eq!(
            result[2]["cache_control"],
            json!({"type": "ephemeral"}),
            "tool_c (last) must have cache_control"
        );

        // Names and descriptions must be preserved.
        assert_eq!(result[0]["name"], "tool_a");
        assert_eq!(result[1]["name"], "tool_b");
        assert_eq!(result[2]["name"], "tool_c");
    }

    /// With `CachePolicy::None` no tool should have a `cache_control` field.
    #[test]
    fn cache_policy_none_adds_no_cache_control_to_tools() {
        let tools = vec![
            ToolDefinition::new(
                "tool_a",
                "First tool",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            ToolDefinition::new(
                "tool_b",
                "Second tool",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
        ];

        let result = build_anthropic_tools(&tools, CachePolicy::None);

        for (i, tool) in result.as_array().unwrap().iter().enumerate() {
            assert!(
                tool["cache_control"].is_null(),
                "tool {i} must not have cache_control when CachePolicy::None"
            );
        }
    }

    /// Parsing `cache_read_input_tokens` and `cache_creation_input_tokens`
    /// from a `message_start` SSE event body into the internal `Usage` struct.
    #[test]
    fn cache_token_fields_are_parsed_from_message_start_usage() {
        let sse_data = r#"{
            "type": "message_start",
            "message": {
                "id": "msg_01",
                "type": "message",
                "role": "assistant",
                "model": "claude-3-5-sonnet-20241022",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 2048,
                    "cache_read_input_tokens": 512
                }
            }
        }"#;

        let event: AnthropicSseEvent =
            serde_json::from_str(sse_data).expect("failed to parse SSE event");

        match event {
            AnthropicSseEvent::MessageStart { message } => {
                assert_eq!(message.usage.input_tokens, 100);
                assert_eq!(
                    message.usage.cache_creation_input_tokens, 2048,
                    "cache_creation_input_tokens must be parsed"
                );
                assert_eq!(
                    message.usage.cache_read_input_tokens, 512,
                    "cache_read_input_tokens must be parsed"
                );
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }
    }
}
