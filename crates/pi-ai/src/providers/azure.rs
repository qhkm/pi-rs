/// Azure OpenAI Service provider.
///
/// Implements the Azure OpenAI API which is mostly compatible with OpenAI's
/// chat completions API but uses different authentication and URL structure.
///
/// Azure OpenAI uses API keys passed in the `api-key` header (not Bearer
/// tokens) and requires a base URL in the format:
/// `https://{resource-name}.openai.azure.com/openai/deployments/{deployment-id}`
use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{PiAiError, Result};
use crate::messages::types::{Content, Message, StopReason, UserContent};
use crate::models::registry::Model;
use crate::providers::traits::{
    make_partial, Context, LLMProvider, ProviderCapabilities, StreamOptions,
};
use crate::streaming::events::StreamEvent;
use crate::streaming::sse::sse_stream_from_response;
use crate::tools::schema::ToolCall;
use crate::utils::build_http_client;

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct AzureOpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    api_version: String,
}

impl AzureOpenAIProvider {
    /// Create a new Azure OpenAI provider.
    ///
    /// # Arguments
    /// * `api_key` - The Azure OpenAI API key
    /// * `base_url` - The Azure OpenAI endpoint, e.g.,
    ///   `https://{resource}.openai.azure.com/openai/deployments/{deployment}`
    /// * `api_version` - The Azure API version (defaults to "2024-06-01")
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        api_version: Option<&str>,
    ) -> Self {
        AzureOpenAIProvider {
            client: build_http_client(300),
            api_key: api_key.into(),
            base_url: base_url.into(),
            api_version: api_version.unwrap_or("2024-06-01").to_string(),
        }
    }

    /// Create from environment variables.
    ///
    /// Expects:
    /// - AZURE_OPENAI_API_KEY
    /// - AZURE_OPENAI_ENDPOINT (e.g., https://my-resource.openai.azure.com)
    /// - AZURE_OPENAI_DEPLOYMENT (the deployment name)
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("AZURE_OPENAI_API_KEY").ok()?;
        let endpoint = std::env::var("AZURE_OPENAI_ENDPOINT").ok()?;
        let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT").ok()?;
        let api_version = std::env::var("AZURE_OPENAI_API_VERSION").ok();

        let base_url = format!("{}/openai/deployments/{}", endpoint.trim_end_matches('/'), deployment);
        
        Some(Self::new(api_key, base_url, api_version.as_deref()))
    }

    fn api_key_for(&self, options: &StreamOptions) -> String {
        options
            .api_key
            .clone()
            .unwrap_or_else(|| self.api_key.clone())
    }
}

// ─── Request format conversion ────────────────────────────────────────────────

fn build_azure_messages(messages: &[Message]) -> Value {
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

                result.push(json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": text,
                }));
            }
        }
    }

    json!(result)
}

fn build_azure_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Value {
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
impl LLMProvider for AzureOpenAIProvider {
    fn name(&self) -> &str {
        "azure-openai"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: false,
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
        let api_key = self.api_key_for(options);
        let mut messages_value = build_azure_messages(&context.messages);
        let max_tokens = options.max_tokens.unwrap_or(model.max_tokens);

        // Prepend system prompt if provided
        if let Some(sp) = &context.system_prompt {
            let system_msg = json!({"role": "system", "content": sp});
            if let Some(arr) = messages_value.as_array_mut() {
                arr.insert(0, system_msg);
            }
        }

        let mut body = json!({
            "messages": messages_value,
            "max_tokens": max_tokens,
            "stream": true,
        });

        if let Some(temp) = options.temperature {
            body["temperature"] = json!(temp);
        }

        if !context.tools.is_empty() {
            body["tools"] = build_azure_tools(&context.tools);
        }

        // Azure uses api-key header instead of Bearer token
        let url = format!(
            "{}/chat/completions?api-version={}",
            self.base_url, self.api_version
        );

        let mut req_builder = self
            .client
            .post(&url)
            .header("api-key", api_key)
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
                provider: "azure-openai".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }

        self.parse_sse_response(response, model, tx).await
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

impl AzureOpenAIProvider {
    async fn parse_sse_response(
        &self,
        response: reqwest::Response,
        model: &Model,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        struct ToolState {
            id: String,
            name: String,
            args_buf: String,
            content_index: usize,
        }

        let mut partial = make_partial(model);
        let mut sse = sse_stream_from_response(response);
        let mut tool_states: HashMap<usize, ToolState> = HashMap::new();
        let mut text_content_index: Option<usize> = None;

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

            if sse_event.is_done() {
                break;
            }

            if sse_event.data.is_empty() {
                continue;
            }

            let chunk: ChatCompletionChunk = match serde_json::from_str(&sse_event.data) {
                Ok(c) => c,
                Err(e) => {
                    debug!(
                        "Failed to parse Azure chunk: {e} — data: {}",
                        sse_event.data
                    );
                    continue;
                }
            };

            if let Some(usage) = chunk.usage {
                partial.usage.input = usage.prompt_tokens;
                partial.usage.output = usage.completion_tokens;
                partial.usage.total_tokens = usage.total_tokens;
            }

            for choice in chunk.choices {
                let delta = choice.delta;
                let finish_reason = choice.finish_reason.as_deref();

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
                        if let Some(Content::Text {
                            text: ref mut t, ..
                        }) = partial.content.get_mut(ci)
                        {
                            t.push_str(text_delta);
                        }
                        let _ = tx
                            .send(StreamEvent::TextDelta {
                                content_index: ci,
                                delta: text_delta.clone(),
                            })
                            .await;
                    }
                }

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
                                if let Some(Content::ToolCall {
                                    arguments: ref mut a,
                                    ..
                                }) = partial.content.get_mut(ci)
                                {
                                    *a = serde_json::from_str(&state.args_buf)
                                        .unwrap_or(Value::String(state.args_buf.clone()));
                                }
                                let _ = tx
                                    .send(StreamEvent::ToolCallDelta {
                                        content_index: ci,
                                        delta: args,
                                    })
                                    .await;
                            }
                        }
                    }
                }

                if let Some(reason) = finish_reason {
                    partial.stop_reason = map_stop_reason(reason);

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

        partial.usage = model.annotate_usage(partial.usage.clone());

        let reason = partial.stop_reason.clone();
        if reason == StopReason::Error {
            let _ = tx
                .send(StreamEvent::Error {
                    reason,
                    error: partial,
                })
                .await;
        } else {
            let _ = tx
                .send(StreamEvent::Done {
                    reason,
                    message: partial,
                })
                .await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::types::{Content, Message, UserContent};
    use chrono::Utc;

    #[test]
    fn azure_provider_creation() {
        let provider = AzureOpenAIProvider::new(
            "test-key",
            "https://test.openai.azure.com/openai/deployments/gpt-4",
            None,
        );
        assert_eq!(provider.name(), "azure-openai");
        assert_eq!(provider.api_version, "2024-06-01");
    }

    #[test]
    fn azure_provider_with_custom_api_version() {
        let provider = AzureOpenAIProvider::new(
            "test-key",
            "https://test.openai.azure.com/openai/deployments/gpt-4",
            Some("2024-02-01"),
        );
        assert_eq!(provider.api_version, "2024-02-01");
    }

    #[test]
    fn image_content_serialized_as_azure_image_url() {
        let image = Content::Image {
            data: "aGVsbG8=".to_string(),
            mime_type: "image/jpeg".to_string(),
        };
        let msg = Message::User(crate::messages::types::UserMessage {
            content: UserContent::Blocks(vec![
                Content::text("What is in this picture?"),
                image,
            ]),
            timestamp: Utc::now().timestamp_millis(),
        });

        let messages_value = build_azure_messages(&[msg]);
        let content_parts = &messages_value[0]["content"];

        assert_eq!(content_parts[0]["type"], "text");
        assert_eq!(content_parts[0]["text"], "What is in this picture?");

        let img_part = &content_parts[1];
        assert_eq!(img_part["type"], "image_url");
        assert_eq!(
            img_part["image_url"]["url"],
            "data:image/jpeg;base64,aGVsbG8="
        );
    }
}
