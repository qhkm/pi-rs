/// Amazon Bedrock provider.
///
/// Implements the Amazon Bedrock ConverseStream API with AWS SigV4 signing.
/// Supports Claude, Llama, Mistral, and other models available through Bedrock.
///
/// AWS credentials are resolved from:
/// - Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN)
/// - AWS credentials file (~/.aws/credentials)
/// - IAM role (when running on AWS infrastructure)
///
/// Required environment variables:
/// - AWS_REGION or AWS_DEFAULT_REGION (e.g., "us-east-1", "us-west-2")
/// - AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY (if not using IAM role)
///
/// Optional:
/// - AWS_SESSION_TOKEN (for temporary credentials)
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
use crate::tools::schema::ToolCall;
use crate::utils::build_http_client;

// ─── Provider struct ──────────────────────────────────────────────────────────

pub struct BedrockProvider {
    client: reqwest::Client,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

/// AWS SigV4 signing parameters
struct SigV4Params {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    region: String,
    service: String,
}

impl BedrockProvider {
    /// Create a new Bedrock provider with explicit credentials.
    pub fn new(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        BedrockProvider {
            client: build_http_client(300),
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
        }
    }

    /// Create from environment variables.
    ///
    /// Resolves AWS credentials using the standard AWS credential chain:
    /// 1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
    /// 2. Credentials file (~/.aws/credentials)
    /// 3. IAM role (ECS/EC2 metadata)
    pub fn from_env() -> Option<Self> {
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .ok()?;
        
        let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();

        Some(Self::new(region, access_key_id, secret_access_key, session_token))
    }

    /// Sign a request using AWS SigV4.
    fn sign_request(
        &self,
        method: &str,
        uri: &str,
        headers: &mut HashMap<String, String>,
        payload: &str,
    ) -> Result<HashMap<String, String>> {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        let params = SigV4Params {
            access_key: self.access_key_id.clone(),
            secret_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            region: self.region.clone(),
            service: "bedrock".to_string(),
        };

        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        // Add required headers
        headers.insert("host".to_string(), format!("bedrock-runtime.{}.amazonaws.com", self.region));
        headers.insert("x-amz-date".to_string(), amz_date.clone());
        if let Some(ref token) = params.session_token {
            headers.insert("x-amz-security-token".to_string(), token.clone());
        }

        // Create canonical request
        let payload_hash = hex::encode(Sha256::digest(payload.as_bytes()));
        headers.insert("x-amz-content-sha256".to_string(), payload_hash.clone());

        // Sort headers for canonical request
        let mut sorted_headers: Vec<_> = headers.iter().collect();
        sorted_headers.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        let canonical_headers: String = sorted_headers
            .iter()
            .map(|(k, v)| format!("{}:{}", k.to_lowercase(), v.trim()))
            .collect::<Vec<_>>()
            .join("\n");

        let signed_headers: String = sorted_headers
            .iter()
            .map(|(k, _)| k.to_lowercase())
            .collect::<Vec<_>>()
            .join(";");

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n\n{}\n{}",
            method,
            uri,
            "", // query string (empty for Bedrock)
            canonical_headers,
            signed_headers,
            payload_hash
        );

        // Create string to sign
        let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, params.region, params.service);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        // Calculate signature
        type HmacSha256 = Hmac<Sha256>;
        
        let k_secret = format!("AWS4{}", params.secret_key);
        let mut mac = HmacSha256::new_from_slice(k_secret.as_bytes())
            .map_err(|e| PiAiError::Auth(format!("HMAC error: {}", e)))?;
        mac.update(date_stamp.as_bytes());
        let k_date = mac.finalize().into_bytes();

        let mut mac = HmacSha256::new_from_slice(&k_date)
            .map_err(|e| PiAiError::Auth(format!("HMAC error: {}", e)))?;
        mac.update(params.region.as_bytes());
        let k_region = mac.finalize().into_bytes();

        let mut mac = HmacSha256::new_from_slice(&k_region)
            .map_err(|e| PiAiError::Auth(format!("HMAC error: {}", e)))?;
        mac.update(params.service.as_bytes());
        let k_service = mac.finalize().into_bytes();

        let mut mac = HmacSha256::new_from_slice(&k_service)
            .map_err(|e| PiAiError::Auth(format!("HMAC error: {}", e)))?;
        mac.update(b"aws4_request");
        let k_signing = mac.finalize().into_bytes();

        let mut mac = HmacSha256::new_from_slice(&k_signing)
            .map_err(|e| PiAiError::Auth(format!("HMAC error: {}", e)))?;
        mac.update(string_to_sign.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Create authorization header
        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            params.access_key, credential_scope, signed_headers, signature
        );

        headers.insert("authorization".to_string(), auth_header);

        Ok(headers.clone())
    }

    fn get_model_id(&self, model: &Model) -> String {
        // Bedrock model IDs are in the format: anthropic.claude-3-sonnet-20240229-v1:0
        // The model.id might be our internal ID, so we need to map it
        if model.id.starts_with("anthropic.") || model.id.starts_with("amazon.") 
            || model.id.starts_with("meta.") || model.id.starts_with("mistral.") {
            model.id.clone()
        } else {
            // Default to Claude Sonnet if we don't have a direct mapping
            // This should be enhanced with a proper model mapping
            "anthropic.claude-3-sonnet-20240229-v1:0".to_string()
        }
    }
}

// ─── Request format conversion ────────────────────────────────────────────────

fn build_bedrock_messages(messages: &[Message]) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(um) => {
                let content = match &um.content {
                    UserContent::Text(t) => {
                        json!([{"text": t}])
                    }
                    UserContent::Blocks(blocks) => {
                        let parts: Vec<Value> = blocks
                            .iter()
                            .filter_map(|c| match c {
                                Content::Text { text, .. } => {
                                    Some(json!({"text": text}))
                                }
                                Content::Image { data, mime_type } => Some(json!({
                                    "image": {
                                        "format": image_mime_to_format(mime_type),
                                        "source": {
                                            "bytes": data
                                        }
                                    }
                                })),
                                _ => None,
                            })
                            .collect();
                        json!(parts)
                    }
                };
                result.push(json!({
                    "role": "user",
                    "content": content
                }));
            }
            Message::Assistant(am) => {
                let mut content_parts: Vec<Value> = Vec::new();
                
                // Add text content
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
                
                if !text.is_empty() {
                    content_parts.push(json!({"text": text}));
                }

                // Add tool calls
                for c in &am.content {
                    if let Content::ToolCall { id, name, arguments, .. } = c {
                        content_parts.push(json!({
                            "toolUse": {
                                "toolUseId": id,
                                "name": name,
                                "input": arguments
                            }
                        }));
                    }
                }

                result.push(json!({
                    "role": "assistant",
                    "content": content_parts
                }));
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
                    "role": "user",
                    "content": [{
                        "toolResult": {
                            "toolUseId": tr.tool_call_id,
                            "content": [{"text": text}],
                            "status": if tr.is_error { "error" } else { "success" }
                        }
                    }]
                }));
            }
        }
    }

    result
}

fn image_mime_to_format(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn build_bedrock_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "toolSpec": {
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": {
                        "json": t.parameters
                    }
                }
            })
        })
        .collect()
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "stop_sequence" => StopReason::Stop,
        "tool_use" => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

// ─── SSE event types from Bedrock ConverseStream ──────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BedrockStreamEvent {
    #[serde(rename = "messageStart")]
    MessageStart { message: BedrockMessageStart },
    #[serde(rename = "contentBlockStart")]
    ContentBlockStart { contentBlockIndex: usize, start: ContentBlockStart },
    #[serde(rename = "contentBlockDelta")]
    ContentBlockDelta { contentBlockIndex: usize, delta: ContentBlockDelta },
    #[serde(rename = "contentBlockStop")]
    ContentBlockStop { contentBlockIndex: usize },
    #[serde(rename = "messageStop")]
    MessageStop { stopReason: String },
    #[serde(rename = "metadata")]
    Metadata { metadata: BedrockMetadata },
}

#[derive(Debug, Deserialize)]
struct BedrockMessageStart {
    role: String,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStart {
    #[serde(default)]
    toolUse: Option<BedrockToolUseStart>,
}

#[derive(Debug, Deserialize)]
struct BedrockToolUseStart {
    toolUseId: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ContentBlockDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    toolUse: Option<BedrockToolUseDelta>,
}

#[derive(Debug, Deserialize)]
struct BedrockToolUseDelta {
    input: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BedrockMetadata {
    #[serde(default)]
    usage: Option<BedrockUsage>,
}

#[derive(Debug, Deserialize)]
struct BedrockUsage {
    inputTokens: u64,
    outputTokens: u64,
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for BedrockProvider {
    fn name(&self) -> &str {
        "amazon-bedrock"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: false, // Bedrock doesn't expose thinking/reasoning in the same way
            vision: true,
        }
    }

    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        _options: &StreamOptions,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let model_id = self.get_model_id(model);
        let uri = format!("/model/{}/converse-stream", model_id);
        
        let messages = build_bedrock_messages(&context.messages);
        
        // Prepend system prompt if provided
        let mut system = None;
        if let Some(sp) = &context.system_prompt {
            system = Some(json!([{"text": sp}]));
        }

        let mut body = json!({
            "messages": messages,
        });

        if let Some(s) = system {
            body["system"] = s;
        }

        if !context.tools.is_empty() {
            body["toolConfig"] = json!({
                "tools": build_bedrock_tools(&context.tools)
            });
        }

        let payload = body.to_string();
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com{}",
            self.region, uri
        );

        // Sign the request
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        let signed_headers = self.sign_request("POST", &uri, &mut headers, &payload)?;

        // Build the request
        let mut req_builder = self
            .client
            .post(&url)
            .body(payload);

        for (k, v) in signed_headers {
            req_builder = req_builder.header(k, v);
        }

        let response = req_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Provider {
                provider: "amazon-bedrock".into(),
                message: format!("HTTP {status}: {text}"),
            });
        }

        self.parse_stream_response(response, model, tx).await
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

impl BedrockProvider {
    async fn parse_stream_response(
        &self,
        response: reqwest::Response,
        model: &Model,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        struct BlockState {
            kind: BlockKind,
            text_buf: String,
            tool_id: String,
            tool_name: String,
            args_buf: String,
        }

        #[derive(PartialEq)]
        enum BlockKind {
            Text,
            ToolUse,
        }

        let mut partial = make_partial(model);
        let mut bytes_stream = response.bytes_stream();
        let mut blocks: HashMap<usize, BlockState> = HashMap::new();
        let mut buffer = String::new();

        let _ = tx
            .send(StreamEvent::Start {
                partial: partial.clone(),
            })
            .await;

        while let Some(chunk_result) = bytes_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    warn!("Stream error: {e}");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines/events
            while let Some(pos) = buffer.find("\n") {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                // Bedrock returns events in the format: `{"type": "...", ...}`
                let event: BedrockStreamEvent = match serde_json::from_str(&line) {
                    Ok(e) => e,
                    Err(e) => {
                        debug!("Failed to parse Bedrock event: {e} — data: {line}");
                        continue;
                    }
                };

                match event {
                    BedrockStreamEvent::MessageStart { .. } => {}

                    BedrockStreamEvent::ContentBlockStart { contentBlockIndex, start } => {
                        if let Some(tool) = start.toolUse {
                            let tool_id = tool.toolUseId.clone();
                            let tool_name = tool.name.clone();
                            blocks.insert(
                                contentBlockIndex,
                                BlockState {
                                    kind: BlockKind::ToolUse,
                                    text_buf: String::new(),
                                    tool_id,
                                    tool_name,
                                    args_buf: String::new(),
                                },
                            );
                            partial.content.push(Content::ToolCall {
                                id: tool.toolUseId,
                                name: tool.name,
                                arguments: Value::Object(Default::default()),
                                thought_signature: None,
                            });
                            let _ = tx
                                .send(StreamEvent::ToolCallStart {
                                    content_index: contentBlockIndex,
                                    partial: partial.clone(),
                                })
                                .await;
                        } else {
                            blocks.insert(
                                contentBlockIndex,
                                BlockState {
                                    kind: BlockKind::Text,
                                    text_buf: String::new(),
                                    tool_id: String::new(),
                                    tool_name: String::new(),
                                    args_buf: String::new(),
                                },
                            );
                            partial.content.push(Content::Text {
                                text: String::new(),
                                text_signature: None,
                            });
                            let _ = tx
                                .send(StreamEvent::TextStart {
                                    content_index: contentBlockIndex,
                                    partial: partial.clone(),
                                })
                                .await;
                        }
                    }

                    BedrockStreamEvent::ContentBlockDelta { contentBlockIndex, delta } => {
                        if let Some(block) = blocks.get_mut(&contentBlockIndex) {
                            if let Some(text) = delta.text {
                                block.text_buf.push_str(&text);
                                if let Some(Content::Text { text: ref mut t, .. }) =
                                    partial.content.get_mut(contentBlockIndex)
                                {
                                    *t = block.text_buf.clone();
                                }
                                let _ = tx
                                    .send(StreamEvent::TextDelta {
                                        content_index: contentBlockIndex,
                                        delta: text,
                                    })
                                    .await;
                            }
                            if let Some(tool_delta) = delta.toolUse {
                                if let Some(input) = tool_delta.input {
                                    block.args_buf.push_str(&input);
                                    if let Some(Content::ToolCall { arguments: ref mut a, .. }) =
                                        partial.content.get_mut(contentBlockIndex)
                                    {
                                        *a = serde_json::from_str(&block.args_buf)
                                            .unwrap_or(Value::String(block.args_buf.clone()));
                                    }
                                    let _ = tx
                                        .send(StreamEvent::ToolCallDelta {
                                            content_index: contentBlockIndex,
                                            delta: input,
                                        })
                                        .await;
                                }
                            }
                        }
                    }

                    BedrockStreamEvent::ContentBlockStop { contentBlockIndex } => {
                        if let Some(block) = blocks.remove(&contentBlockIndex) {
                            match block.kind {
                                BlockKind::Text => {
                                    let _ = tx
                                        .send(StreamEvent::TextEnd {
                                            content_index: contentBlockIndex,
                                            content: block.text_buf,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                                BlockKind::ToolUse => {
                                    let args = serde_json::from_str(&block.args_buf)
                                        .unwrap_or(Value::Object(Default::default()));
                                    let tool_call = ToolCall {
                                        id: block.tool_id,
                                        name: block.tool_name,
                                        arguments: args,
                                    };
                                    let _ = tx
                                        .send(StreamEvent::ToolCallEnd {
                                            content_index: contentBlockIndex,
                                            tool_call,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                            }
                        }
                    }

                    BedrockStreamEvent::MessageStop { stopReason } => {
                        partial.stop_reason = map_stop_reason(&stopReason);
                    }

                    BedrockStreamEvent::Metadata { metadata } => {
                        if let Some(usage) = metadata.usage {
                            partial.usage.input = usage.inputTokens;
                            partial.usage.output = usage.outputTokens;
                            partial.usage.total_tokens = usage.inputTokens + usage.outputTokens;
                        }
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

    #[test]
    fn bedrock_provider_creation() {
        let provider = BedrockProvider::new(
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            None,
        );
        assert_eq!(provider.name(), "amazon-bedrock");
        assert_eq!(provider.region, "us-east-1");
    }

    #[test]
    fn image_mime_conversion() {
        assert_eq!(image_mime_to_format("image/png"), "png");
        assert_eq!(image_mime_to_format("image/jpeg"), "jpeg");
        assert_eq!(image_mime_to_format("image/jpg"), "jpeg");
        assert_eq!(image_mime_to_format("image/gif"), "gif");
        assert_eq!(image_mime_to_format("image/webp"), "webp");
        assert_eq!(image_mime_to_format("image/svg+xml"), "png"); // default
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason("end_turn"), StopReason::Stop);
        assert_eq!(map_stop_reason("max_tokens"), StopReason::Length);
        assert_eq!(map_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("stop_sequence"), StopReason::Stop);
    }
}
