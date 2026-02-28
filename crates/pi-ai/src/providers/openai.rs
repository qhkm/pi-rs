/// OpenAI-compatible chat completions provider.
///
/// Supports OpenAI, Groq, Mistral, xAI (Grok), Cerebras, OpenRouter, Ollama,
/// and any other server that speaks the OpenAI chat-completions SSE protocol.
use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{PiAiError, Result};
use crate::messages::types::{
    AssistantMessage, Content, Message, StopReason, Usage, UserContent,
};
use crate::models::registry::Model;
use crate::providers::traits::{
    Context, LLMProvider, ProviderCapabilities, StreamOptions, make_partial, resolve_api_key,
};
use crate::streaming::events::StreamEvent;
use crate::streaming::sse::sse_stream_from_response;
use crate::tools::schema::ToolCall;

// ─── Compat flags ─────────────────────────────────────────────────────────────

/// Compatibility settings for OpenAI-compatible providers that deviate from
/// the vanilla OpenAI behaviour.
#[derive(Debug, Clone, Default)]
pub struct OpenAICompat {
    /// Whether the provider accepts `"role": "developer"` system messages
    /// (OpenAI o-series).  Falls back to `"system"` if `false`.
    pub supports_developer_role: bool,
    /// Whether the provider supports the `reasoning_effort` field.
    pub supports_reasoning_effort: bool,
    /// Which field name to use for the output token limit.
    pub max_tokens_field: MaxTokensField,
    /// Whether tool results must include the `name` field (required by some
    /// providers but rejected by others).
    pub requires_tool_result_name: bool,
}

impl OpenAICompat {
    pub fn for_openai() -> Self {
        OpenAICompat {
            supports_developer_role: true,
            supports_reasoning_effort: true,
            max_tokens_field: MaxTokensField::MaxCompletionTokens,
            requires_tool_result_name: false,
        }
    }

    pub fn for_groq() -> Self {
        OpenAICompat {
            supports_developer_role: false,
            supports_reasoning_effort: false,
            max_tokens_field: MaxTokensField::MaxTokens,
            requires_tool_result_name: false,
        }
    }

    pub fn for_mistral() -> Self {
        OpenAICompat {
            supports_developer_role: false,
            supports_reasoning_effort: false,
            max_tokens_field: MaxTokensField::MaxTokens,
            requires_tool_result_name: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum MaxTokensField {
    #[default]
    MaxCompletionTokens,
    MaxTokens,
}

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    pub compat: OpenAICompat,
}

impl OpenAIProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<&str>,
        compat: OpenAICompat,
    ) -> Self {
        OpenAIProvider {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: base_url.unwrap_or("https://api.openai.com").to_string(),
            compat,
        }
    }

    fn api_key_for(&self, options: &StreamOptions) -> String {
        options.api_key.clone().unwrap_or_else(|| self.api_key.clone())
    }
}

// ─── Request format conversion ────────────────────────────────────────────────

fn build_openai_messages(messages: &[Message], compat: &OpenAICompat) -> Value {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(um) => {
                let content = match &um.content {
                    UserContent::Text(t) => json!(t),
                    UserContent::Blocks(blocks) => {
                        let parts: Vec<Value> = blocks
                            .iter()
                            .filter_map(|c| match c {
                                Content::Text { text, .. } => {
                                    Some(json!({"type": "text", "text": text}))
                                }
                                Content::Image { data, mime_type } => Some(json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{mime_type};base64,{data}")
                                    }
                                })),
                                _ => None,
                            })
                            .collect();
                        json!(parts)
                    }
                };
                result.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(am) => {
                // Check if there are tool calls.
                let tool_calls: Vec<Value> = am
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::ToolCall { id, name, arguments, .. } = c {
                            Some(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": arguments.to_string(),
                                }
                            }))
                        } else {
                            None
                        }
                    })
                    .collect();

                let text: String = am
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text, .. } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();

                let mut msg_obj = json!({"role": "assistant"});
                if !text.is_empty() {
                    msg_obj["content"] = json!(text);
                } else {
                    msg_obj["content"] = json!(null);
                }
                if !tool_calls.is_empty() {
                    msg_obj["tool_calls"] = json!(tool_calls);
                }
                result.push(msg_obj);
            }
            Message::ToolResult(tr) => {
                let text = tr
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
                    .join("");

                let mut tool_msg = json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": text,
                });
                if compat.requires_tool_result_name {
                    tool_msg["name"] = json!(tr.tool_name);
                }
                result.push(tool_msg);
            }
        }
    }

    json!(result)
}

fn build_openai_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Value {
    let converted: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();
    json!(converted)
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::Error,
        _ => StopReason::Stop,
    }
}

// ─── SSE delta types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
    index: usize,
}

#[derive(Debug, Deserialize, Default)]
struct ChunkDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallChunk>>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolCallChunk {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionChunk>,
}

#[derive(Debug, Deserialize)]
struct FunctionChunk {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { streaming: true, tool_calling: true, thinking: false, vision: true }
    }

    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let api_key = self.api_key_for(options);
        let messages_value = build_openai_messages(&context.messages, &self.compat);
        let max_tokens = options.max_tokens.unwrap_or(model.max_tokens);

        let max_tokens_key = match self.compat.max_tokens_field {
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
            MaxTokensField::MaxTokens => "max_tokens",
        };

        let mut body = json!({
            "model": model.id,
            "messages": messages_value,
            max_tokens_key: max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        if let Some(temp) = options.temperature {
            body["temperature"] = json!(temp);
        }

        if !context.tools.is_empty() {
            body["tools"] = build_openai_tools(&context.tools);
        }

        let mut req_builder = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {api_key}"))
            .header("content-type", "application/json")
            .json(&body);

        if let Some(extra) = &options.headers {
            for (k, v) in extra {
                req_builder = req_builder.header(k, v);
            }
        }

        let response = req_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Provider {
                provider: "openai".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }

        // ── SSE parsing state ───────────────────────────────────────────

        let mut partial = make_partial(model);
        let mut sse = sse_stream_from_response(response);

        // Tool call accumulation: index → (id, name, args_buf, content_index)
        struct ToolState {
            id: String,
            name: String,
            args_buf: String,
            content_index: usize,
        }
        let mut tool_states: HashMap<usize, ToolState> = HashMap::new();

        // Text content block tracking
        let mut text_content_index: Option<usize> = None;
        // Thinking content block tracking
        let mut thinking_content_index: Option<usize> = None;

        let _ = tx.send(StreamEvent::Start { partial: partial.clone() }).await;

        while let Some(sse_result) = sse.next().await {
            let sse_event = match sse_result {
                Ok(e) => e,
                Err(e) => {
                    warn!("SSE error: {e}");
                    break;
                }
            };

            if sse_event.is_done() {
                break;
            }

            if sse_event.data.is_empty() {
                continue;
            }

            let chunk: ChatCompletionChunk = match serde_json::from_str(&sse_event.data) {
                Ok(c) => c,
                Err(e) => {
                    debug!("Failed to parse OpenAI chunk: {e} — data: {}", sse_event.data);
                    continue;
                }
            };

            // Usage data (last chunk from OpenAI).
            if let Some(usage) = chunk.usage {
                partial.usage.input = usage.prompt_tokens;
                partial.usage.output = usage.completion_tokens;
                partial.usage.total_tokens = usage.total_tokens;
            }

            for choice in chunk.choices {
                let delta = choice.delta;
                let finish_reason = choice.finish_reason.as_deref();

                // ── Thinking / reasoning content ────────────────────────
                if let Some(thinking_delta) = &delta.reasoning_content {
                    if !thinking_delta.is_empty() {
                        let ci = match thinking_content_index {
                            Some(i) => i,
                            None => {
                                let i = partial.content.len();
                                partial.content.push(Content::Thinking {
                                    thinking: String::new(),
                                    thinking_signature: None,
                                    redacted: false,
                                });
                                thinking_content_index = Some(i);
                                let _ = tx
                                    .send(StreamEvent::ThinkingStart {
                                        content_index: i,
                                        partial: partial.clone(),
                                    })
                                    .await;
                                i
                            }
                        };

                        if let Some(Content::Thinking { thinking: ref mut t, .. }) =
                            partial.content.get_mut(ci)
                        {
                            t.push_str(thinking_delta);
                        }
                        let _ = tx
                            .send(StreamEvent::ThinkingDelta {
                                content_index: ci,
                                delta: thinking_delta.clone(),
                                partial: partial.clone(),
                            })
                            .await;
                    }
                }

                // ── Text content ─────────────────────────────────────────
                if let Some(text_delta) = &delta.content {
                    if !text_delta.is_empty() {
                        let ci = match text_content_index {
                            Some(i) => i,
                            None => {
                                let i = partial.content.len();
                                partial.content.push(Content::Text {
                                    text: String::new(),
                                    text_signature: None,
                                });
                                text_content_index = Some(i);
                                let _ = tx
                                    .send(StreamEvent::TextStart {
                                        content_index: i,
                                        partial: partial.clone(),
                                    })
                                    .await;
                                i
                            }
                        };

                        if let Some(Content::Text { text: ref mut t, .. }) =
                            partial.content.get_mut(ci)
                        {
                            t.push_str(text_delta);
                        }
                        let _ = tx
                            .send(StreamEvent::TextDelta {
                                content_index: ci,
                                delta: text_delta.clone(),
                                partial: partial.clone(),
                            })
                            .await;
                    }
                }

                // ── Tool call deltas ─────────────────────────────────────
                if let Some(tool_call_chunks) = delta.tool_calls {
                    for tc_chunk in tool_call_chunks {
                        let tc_idx = tc_chunk.index;

                        let state = tool_states.entry(tc_idx).or_insert_with(|| {
                            let content_index = partial.content.len();
                            partial.content.push(Content::ToolCall {
                                id: String::new(),
                                name: String::new(),
                                arguments: Value::Null,
                                thought_signature: None,
                            });
                            ToolState {
                                id: String::new(),
                                name: String::new(),
                                args_buf: String::new(),
                                content_index,
                            }
                        });

                        if let Some(id) = tc_chunk.id {
                            state.id = id;
                        }

                        if let Some(func) = tc_chunk.function {
                            if let Some(name) = func.name {
                                state.name = name;
                            }
                            if let Some(args) = func.arguments {
                                // First chunk: emit ToolCallStart.
                                if state.args_buf.is_empty() && !args.is_empty() {
                                    let ci = state.content_index;
                                    if let Some(Content::ToolCall {
                                        id: ref mut cid,
                                        name: ref mut cname,
                                        ..
                                    }) = partial.content.get_mut(ci)
                                    {
                                        *cid = state.id.clone();
                                        *cname = state.name.clone();
                                    }
                                    let _ = tx
                                        .send(StreamEvent::ToolCallStart {
                                            content_index: ci,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }

                                let ci = state.content_index;
                                state.args_buf.push_str(&args);
                                if let Some(Content::ToolCall { arguments: ref mut a, .. }) =
                                    partial.content.get_mut(ci)
                                {
                                    *a = serde_json::from_str(&state.args_buf)
                                        .unwrap_or(Value::String(state.args_buf.clone()));
                                }
                                let _ = tx
                                    .send(StreamEvent::ToolCallDelta {
                                        content_index: ci,
                                        delta: args,
                                        partial: partial.clone(),
                                    })
                                    .await;
                            }
                        }
                    }
                }

                // ── Finish reason ─────────────────────────────────────────
                if let Some(reason) = finish_reason {
                    partial.stop_reason = map_stop_reason(reason);

                    // Emit TextEnd if we had text.
                    if let Some(ci) = text_content_index {
                        let full_text = partial
                            .content
                            .get(ci)
                            .and_then(|c| {
                                if let Content::Text { text, .. } = c {
                                    Some(text.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();
                        let _ = tx
                            .send(StreamEvent::TextEnd {
                                content_index: ci,
                                content: full_text,
                                partial: partial.clone(),
                            })
                            .await;
                    }

                    // Emit ThinkingEnd if we had thinking.
                    if let Some(ci) = thinking_content_index {
                        let full_thinking = partial
                            .content
                            .get(ci)
                            .and_then(|c| {
                                if let Content::Thinking { thinking, .. } = c {
                                    Some(thinking.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();
                        let _ = tx
                            .send(StreamEvent::ThinkingEnd {
                                content_index: ci,
                                content: full_thinking,
                                partial: partial.clone(),
                            })
                            .await;
                    }

                    // Emit ToolCallEnd for all accumulated tool calls.
                    let states: Vec<ToolState> = tool_states.drain().map(|(_, v)| v).collect();
                    for state in states {
                        let args = serde_json::from_str(&state.args_buf)
                            .unwrap_or(Value::Object(Default::default()));
                        let tool_call = ToolCall {
                            id: state.id,
                            name: state.name,
                            arguments: args,
                        };
                        let _ = tx
                            .send(StreamEvent::ToolCallEnd {
                                content_index: state.content_index,
                                tool_call,
                                partial: partial.clone(),
                            })
                            .await;
                    }
                }
            }
        }

        // Finalise cost.
        partial.usage = model.annotate_usage(partial.usage.clone());

        let reason = partial.stop_reason.clone();
        if reason == StopReason::Error {
            let _ = tx
                .send(StreamEvent::Error { reason, error: partial })
                .await;
        } else {
            let _ = tx
                .send(StreamEvent::Done { reason, message: partial })
                .await;
        }

        Ok(())
    }
}
