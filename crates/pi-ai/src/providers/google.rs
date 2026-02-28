/// Google Gemini GenerativeAI provider.
///
/// Implements streaming via `POST /v1beta/models/{model}:streamGenerateContent?alt=sse`.
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

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct GoogleProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl GoogleProvider {
    pub fn new(api_key: impl Into<String>, base_url: Option<&str>) -> Self {
        GoogleProvider {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build HTTP client"),
            api_key: api_key.into(),
            base_url: base_url
                .unwrap_or("https://generativelanguage.googleapis.com")
                .to_string(),
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

fn build_gemini_contents(messages: &[Message]) -> Value {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(um) => {
                let parts = match &um.content {
                    UserContent::Text(t) => vec![json!({"text": t})],
                    UserContent::Blocks(blocks) => {
                        blocks.iter().filter_map(content_to_gemini_part).collect()
                    }
                };
                result.push(json!({"role": "user", "parts": parts}));
            }
            Message::Assistant(am) => {
                let parts: Vec<Value> = am
                    .content
                    .iter()
                    .filter_map(content_to_gemini_part)
                    .collect();
                result.push(json!({"role": "model", "parts": parts}));
            }
            Message::ToolResult(tr) => {
                let output_value: Value = tr
                    .content
                    .iter()
                    .find_map(|c| {
                        if let Content::Text { text, .. } = c {
                            serde_json::from_str(text)
                                .ok()
                                .or_else(|| Some(json!({"output": text})))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(json!({}));

                let part = json!({
                    "functionResponse": {
                        "name": tr.tool_name,
                        "response": output_value,
                    }
                });
                // Tool responses go in a user turn.
                result.push(json!({"role": "user", "parts": [part]}));
            }
        }
    }

    json!(result)
}

fn content_to_gemini_part(c: &Content) -> Option<Value> {
    match c {
        Content::Text { text, .. } => Some(json!({"text": text})),
        Content::Image { data, mime_type } => Some(json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": data,
            }
        })),
        Content::ToolCall {
            id: _,
            name,
            arguments,
            ..
        } => Some(json!({
            "functionCall": {
                "name": name,
                "args": arguments,
            }
        })),
        Content::Thinking { .. } => None, // Strip thinking from history
    }
}

fn build_gemini_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Value {
    let declarations: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();

    json!([{ "functionDeclarations": declarations }])
}

// ─── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default)]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<GeminiFunctionCall>,
    #[serde(default)]
    thought: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    #[serde(default)]
    total_token_count: u64,
}

fn map_gemini_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => StopReason::Error,
        "TOOL_USE" | "FUNCTION_CALL" => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
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
        let api_key = self.api_key_for(options);
        let contents = build_gemini_contents(&context.messages);

        let mut generation_config = json!({
            "maxOutputTokens": options.max_tokens.unwrap_or(model.max_tokens),
        });

        if let Some(temp) = options.temperature {
            generation_config["temperature"] = json!(temp);
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config,
        });

        if let Some(sp) = &context.system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": sp}]
            });
        }

        if !context.tools.is_empty() {
            body["tools"] = build_gemini_tools(&context.tools);
        }

        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.base_url, model.id
        );

        let mut req_builder = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", &api_key)
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
                provider: "google".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }

        // ── Parse SSE stream ────────────────────────────────────────────

        let mut partial = make_partial(model);
        let mut sse = sse_stream_from_response(response);

        // Content accumulation.
        // We build the content as we receive chunks and emit events.
        // Gemini sends complete content objects per chunk, not token-level deltas,
        // so we treat each chunk as a TextDelta.

        // Track indices per "slot" type.
        let mut text_content_index: Option<usize> = None;
        let mut thinking_content_index: Option<usize> = None;
        // tool calls: name -> (content_index, args accumulated)
        let mut tool_call_map: HashMap<String, (usize, String)> = HashMap::new();
        // sequential tool call ID counter (Gemini doesn't provide IDs)
        let mut tool_id_counter = 0u64;

        let _ = tx
            .send(StreamEvent::Start {
                partial: partial.clone(),
            })
            .await;

        while let Some(sse_result) = sse.next().await {
            let sse_event = match sse_result {
                Ok(e) => e,
                Err(e) => {
                    warn!("Google SSE error: {e}");
                    break;
                }
            };

            if sse_event.data.is_empty() {
                continue;
            }

            let resp: GenerateContentResponse = match serde_json::from_str(&sse_event.data) {
                Ok(r) => r,
                Err(e) => {
                    debug!(
                        "Failed to parse Gemini response: {e} — data: {}",
                        sse_event.data
                    );
                    continue;
                }
            };

            // Update usage.
            if let Some(usage) = &resp.usage_metadata {
                partial.usage.input = usage.prompt_token_count;
                partial.usage.output = usage.candidates_token_count;
                partial.usage.total_tokens = usage.total_token_count;
            }

            for candidate in &resp.candidates {
                // Handle finish reason.
                if let Some(finish_reason) = &candidate.finish_reason {
                    partial.stop_reason = map_gemini_stop_reason(finish_reason);
                }

                if let Some(content) = &candidate.content {
                    for part in &content.parts {
                        let is_thought = part.thought.unwrap_or(false);

                        if let Some(text) = &part.text {
                            if is_thought {
                                // Thinking / reasoning block.
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
                                if let Some(Content::Thinking {
                                    thinking: ref mut t,
                                    ..
                                }) = partial.content.get_mut(ci)
                                {
                                    t.push_str(text);
                                }
                                let _ = tx
                                    .send(StreamEvent::ThinkingDelta {
                                        content_index: ci,
                                        delta: text.clone(),
                                    })
                                    .await;
                            } else {
                                // Regular text block.
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
                                    t.push_str(text);
                                }
                                let _ = tx
                                    .send(StreamEvent::TextDelta {
                                        content_index: ci,
                                        delta: text.clone(),
                                    })
                                    .await;
                            }
                        }

                        if let Some(fc) = &part.function_call {
                            let key = fc.name.clone();
                            let (ci, _) = tool_call_map.entry(key.clone()).or_insert_with(|| {
                                tool_id_counter += 1;
                                let id = format!("call_{tool_id_counter}");
                                let i = partial.content.len();
                                partial.content.push(Content::ToolCall {
                                    id,
                                    name: fc.name.clone(),
                                    arguments: Value::Object(Default::default()),
                                    thought_signature: None,
                                });
                                (i, String::new())
                            });
                            let ci = *ci;

                            // Gemini gives us the full args object, not a delta string.
                            let args_str = fc.args.to_string();
                            if let Some(Content::ToolCall {
                                arguments: ref mut a,
                                ..
                            }) = partial.content.get_mut(ci)
                            {
                                *a = fc.args.clone();
                            }

                            let _ = tx
                                .send(StreamEvent::ToolCallStart {
                                    content_index: ci,
                                    partial: partial.clone(),
                                })
                                .await;
                            let _ = tx
                                .send(StreamEvent::ToolCallDelta {
                                    content_index: ci,
                                    delta: args_str.clone(),
                                })
                                .await;
                        }
                    }
                }
            }
        }

        // Finalise: emit end events.
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

        for (name, (ci, _)) in &tool_call_map {
            let tool_call = match partial.content.get(*ci) {
                Some(Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                }) => ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                },
                _ => continue,
            };
            let _ = tx
                .send(StreamEvent::ToolCallEnd {
                    content_index: *ci,
                    tool_call,
                    partial: partial.clone(),
                })
                .await;
        }

        // Annotate cost.
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
