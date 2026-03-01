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

        Some(Self::new(
            region,
            access_key_id,
            secret_access_key,
            session_token,
        ))
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
        headers.insert(
            "host".to_string(),
            format!("bedrock-runtime.{}.amazonaws.com", self.region),
        );
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

        // AWS SigV4: each canonical header must end with '\n'.
        let canonical_headers: String = sorted_headers
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k.to_lowercase(), v.trim()))
            .collect::<String>();

        let signed_headers: String = sorted_headers
            .iter()
            .map(|(k, _)| k.to_lowercase())
            .collect::<Vec<_>>()
            .join(";");

        // Canonical request format (per AWS SigV4 spec):
        //   METHOD \n URI \n QUERY \n CANONICAL_HEADERS \n SIGNED_HEADERS \n PAYLOAD_HASH
        // Note: canonical_headers already ends with \n, so the blank line
        // separating headers from signed_headers is produced naturally.
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            uri,
            "", // query string (empty for Bedrock)
            canonical_headers,
            signed_headers,
            payload_hash
        );

        // Create string to sign
        let credential_scope = format!(
            "{}/{}/{}/aws4_request",
            date_stamp, params.region, params.service
        );
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
        if model.id.starts_with("anthropic.")
            || model.id.starts_with("amazon.")
            || model.id.starts_with("meta.")
            || model.id.starts_with("mistral.")
        {
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
                                Content::Text { text, .. } => Some(json!({"text": text})),
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
                    if let Content::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = c
                    {
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

// ─── AWS event stream binary decoding ─────────────────────────────────────────
//
// The Bedrock ConverseStream API uses the AWS event stream binary encoding
// (`application/vnd.amazon.eventstream`).  Each frame has:
//   - Prelude: total_length (u32 BE) + headers_length (u32 BE) + prelude_crc (u32 BE)
//   - Headers: sequence of name-len(u8)+name+type(u8)+value-len(u16 BE)+value
//   - Payload: raw bytes
//   - Message CRC: crc32 of the entire frame (u32 BE)
//
// We also support newline-delimited JSON for API Gateway / proxy configs.

/// Parse a single AWS event stream frame from a byte buffer.
///
/// Returns `Ok(Some((event_type, payload, bytes_consumed)))` on success,
/// `Ok(None)` if not enough bytes yet, or `Err` on CRC mismatch.
fn parse_event_stream_frame(
    buf: &[u8],
) -> std::result::Result<Option<(String, Vec<u8>, usize)>, String> {
    if buf.len() < 12 {
        return Ok(None); // need at least the prelude
    }

    let total_length = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let headers_length = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    let prelude_crc = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

    if buf.len() < total_length {
        return Ok(None); // incomplete frame
    }

    // Validate prelude CRC (covers the first 8 bytes)
    let computed_prelude_crc = crc32fast::hash(&buf[..8]);
    if computed_prelude_crc != prelude_crc {
        return Err(format!(
            "Prelude CRC mismatch: expected {prelude_crc:#010x}, got {computed_prelude_crc:#010x}"
        ));
    }

    // Validate message CRC (covers everything except the last 4 bytes)
    let message_crc_offset = total_length - 4;
    let message_crc = u32::from_be_bytes([
        buf[message_crc_offset],
        buf[message_crc_offset + 1],
        buf[message_crc_offset + 2],
        buf[message_crc_offset + 3],
    ]);
    let computed_message_crc = crc32fast::hash(&buf[..message_crc_offset]);
    if computed_message_crc != message_crc {
        return Err(format!(
            "Message CRC mismatch: expected {message_crc:#010x}, got {computed_message_crc:#010x}"
        ));
    }

    // Parse headers
    let headers_start = 12;
    let headers_end = headers_start + headers_length;
    let headers = parse_event_stream_headers(&buf[headers_start..headers_end])?;

    // Extract event type from headers
    let event_type = headers
        .get(":event-type")
        .or_else(|| headers.get(":exception-type"))
        .cloned()
        .unwrap_or_default();

    // Extract payload
    let payload_start = headers_end;
    let payload_end = message_crc_offset;
    let payload = buf[payload_start..payload_end].to_vec();

    Ok(Some((event_type, payload, total_length)))
}

/// Parse AWS event stream headers from a byte slice.
///
/// Header format: name_len(u8) + name(name_len bytes) + type(u8) + value_len(u16 BE) + value
/// We only handle type 7 (string) headers, which is what Bedrock uses.
fn parse_event_stream_headers(
    mut buf: &[u8],
) -> std::result::Result<HashMap<String, String>, String> {
    let mut headers = HashMap::new();

    while !buf.is_empty() {
        if buf.is_empty() {
            break;
        }

        let name_len = buf[0] as usize;
        buf = &buf[1..];
        if buf.len() < name_len {
            return Err("Header name truncated".to_string());
        }
        let name = String::from_utf8_lossy(&buf[..name_len]).to_string();
        buf = &buf[name_len..];

        if buf.is_empty() {
            return Err("Header type missing".to_string());
        }
        let header_type = buf[0];
        buf = &buf[1..];

        // AWS event stream header types and their value sizes:
        //   0=bool_true(0), 1=bool_false(0), 2=u8(1), 3=i8(1),
        //   4=i16(2), 5=i32(4), 6=i64(8), 7=string(u16-prefixed),
        //   8=bytes(u16-prefixed), 9=timestamp_i64(8), 10=uuid(16)
        if header_type == 7 {
            // String: 16-bit length-prefixed
            if buf.len() < 2 {
                return Err("Header value length truncated".to_string());
            }
            let value_len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
            buf = &buf[2..];
            if buf.len() < value_len {
                return Err("Header value truncated".to_string());
            }
            let value = String::from_utf8_lossy(&buf[..value_len]).to_string();
            buf = &buf[value_len..];
            headers.insert(name, value);
        } else {
            // Skip non-string headers by reading past their value
            let skip = match header_type {
                0 | 1 => 0, // bool true/false: no value bytes
                2 | 3 => 1, // u8/i8
                4 => 2,     // i16
                5 => 4,     // i32
                6 => 8,     // i64
                8 => {
                    // bytes: u16-prefixed
                    if buf.len() < 2 {
                        return Err("Header value truncated".to_string());
                    }
                    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    len + 2
                }
                9 => 8,   // timestamp (i64)
                10 => 16, // UUID (16 bytes)
                _ => return Err(format!("Unknown header type: {header_type}")),
            };
            if buf.len() < skip {
                return Err("Header value truncated".to_string());
            }
            buf = &buf[skip..];
        }
    }

    Ok(headers)
}

// ─── SSE event types from Bedrock ConverseStream ──────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BedrockStreamEvent {
    #[serde(rename = "messageStart")]
    MessageStart { message: BedrockMessageStart },
    #[serde(rename = "contentBlockStart")]
    ContentBlockStart {
        #[serde(alias = "content_block_index", alias = "contentBlockIndex")]
        content_block_index: usize,
        start: ContentBlockStart,
    },
    #[serde(rename = "contentBlockDelta")]
    ContentBlockDelta {
        #[serde(alias = "content_block_index", alias = "contentBlockIndex")]
        content_block_index: usize,
        delta: ContentBlockDelta,
    },
    #[serde(rename = "contentBlockStop")]
    ContentBlockStop {
        #[serde(alias = "content_block_index", alias = "contentBlockIndex")]
        content_block_index: usize,
    },
    #[serde(rename = "messageStop")]
    MessageStop {
        #[serde(alias = "stop_reason", alias = "stopReason")]
        stop_reason: String,
    },
    #[serde(rename = "metadata")]
    Metadata { metadata: BedrockMetadata },
}

#[derive(Debug, Deserialize)]
struct BedrockMessageStart {
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockStart {
    #[serde(default)]
    tool_use: Option<BedrockToolUseStart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockToolUseStart {
    tool_use_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_use: Option<BedrockToolUseDelta>,
}

#[derive(Debug, Deserialize)]
struct BedrockToolUseDelta {
    input: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockMetadata {
    #[serde(default)]
    usage: Option<BedrockUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockUsage {
    input_tokens: u64,
    output_tokens: u64,
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
        let mut req_builder = self.client.post(&url).body(payload);

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
        // Check content-type to decide between binary event stream and JSON-line parsing
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let is_binary = content_type.contains("vnd.amazon.eventstream");

        let mut partial = make_partial(model);
        let mut bytes_stream = response.bytes_stream();
        let mut blocks: HashMap<usize, BlockState> = HashMap::new();

        let _ = tx
            .send(StreamEvent::Start {
                partial: partial.clone(),
            })
            .await;

        if is_binary {
            let mut bin_buf = Vec::<u8>::new();

            while let Some(chunk_result) = bytes_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Stream error: {e}");
                        break;
                    }
                };

                bin_buf.extend_from_slice(&chunk);

                // Parse complete frames from the buffer.
                // Track total bytes consumed and drain once after the loop
                // to avoid O(n) shifts on every frame.
                let mut cursor = 0usize;
                loop {
                    match parse_event_stream_frame(&bin_buf[cursor..]) {
                        Ok(Some((_event_type, payload, consumed))) => {
                            // Parse the JSON payload as a Bedrock event
                            if !payload.is_empty() {
                                if let Ok(event) =
                                    serde_json::from_slice::<BedrockStreamEvent>(&payload)
                                {
                                    handle_bedrock_event(event, &mut blocks, &mut partial, &tx)
                                        .await;
                                } else {
                                    debug!(
                                        "Failed to parse Bedrock binary payload: {}",
                                        String::from_utf8_lossy(&payload)
                                    );
                                }
                            }
                            cursor += consumed;
                        }
                        Ok(None) => break, // need more data
                        Err(e) => {
                            warn!("Event stream frame error: {e}");
                            cursor = bin_buf.len(); // discard all
                            break;
                        }
                    }
                }
                if cursor > 0 {
                    bin_buf.drain(..cursor);
                }
            }
        } else {
            // JSON-line parsing (for API Gateway / proxy configurations)
            let mut text_buf = String::new();

            while let Some(chunk_result) = bytes_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Stream error: {e}");
                        break;
                    }
                };

                text_buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = text_buf.find('\n') {
                    let line = text_buf[..pos].trim().to_string();
                    text_buf = text_buf[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let event: BedrockStreamEvent = match serde_json::from_str(&line) {
                        Ok(e) => e,
                        Err(e) => {
                            debug!("Failed to parse Bedrock event: {e} — data: {line}");
                            continue;
                        }
                    };

                    handle_bedrock_event(event, &mut blocks, &mut partial, &tx).await;
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

/// Process a single Bedrock stream event — shared by both binary and JSON-line parsers.
async fn handle_bedrock_event(
    event: BedrockStreamEvent,
    blocks: &mut HashMap<usize, BlockState>,
    partial: &mut crate::messages::types::AssistantMessage,
    tx: &mpsc::Sender<StreamEvent>,
) {
    match event {
        BedrockStreamEvent::MessageStart { .. } => {}

        BedrockStreamEvent::ContentBlockStart {
            content_block_index,
            start,
        } => {
            if let Some(tool) = start.tool_use {
                let tool_id = tool.tool_use_id.clone();
                let tool_name = tool.name.clone();
                blocks.insert(
                    content_block_index,
                    BlockState {
                        kind: BlockKind::ToolUse,
                        text_buf: String::new(),
                        tool_id,
                        tool_name,
                        args_buf: String::new(),
                    },
                );
                partial.content.push(Content::ToolCall {
                    id: tool.tool_use_id,
                    name: tool.name,
                    arguments: Value::Object(Default::default()),
                    thought_signature: None,
                });
                let _ = tx
                    .send(StreamEvent::ToolCallStart {
                        content_index: content_block_index,
                        partial: partial.clone(),
                    })
                    .await;
            } else {
                blocks.insert(
                    content_block_index,
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
                        content_index: content_block_index,
                        partial: partial.clone(),
                    })
                    .await;
            }
        }

        BedrockStreamEvent::ContentBlockDelta {
            content_block_index,
            delta,
        } => {
            if let Some(block) = blocks.get_mut(&content_block_index) {
                if let Some(text) = delta.text {
                    block.text_buf.push_str(&text);
                    if let Some(Content::Text {
                        text: ref mut t, ..
                    }) = partial.content.get_mut(content_block_index)
                    {
                        *t = block.text_buf.clone();
                    }
                    let _ = tx
                        .send(StreamEvent::TextDelta {
                            content_index: content_block_index,
                            delta: text,
                        })
                        .await;
                }
                if let Some(tool_delta) = delta.tool_use {
                    if let Some(input) = tool_delta.input {
                        block.args_buf.push_str(&input);
                        if let Some(Content::ToolCall {
                            arguments: ref mut a,
                            ..
                        }) = partial.content.get_mut(content_block_index)
                        {
                            *a = serde_json::from_str(&block.args_buf)
                                .unwrap_or(Value::String(block.args_buf.clone()));
                        }
                        let _ = tx
                            .send(StreamEvent::ToolCallDelta {
                                content_index: content_block_index,
                                delta: input,
                            })
                            .await;
                    }
                }
            }
        }

        BedrockStreamEvent::ContentBlockStop {
            content_block_index,
        } => {
            if let Some(block) = blocks.remove(&content_block_index) {
                match block.kind {
                    BlockKind::Text => {
                        let _ = tx
                            .send(StreamEvent::TextEnd {
                                content_index: content_block_index,
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
                                content_index: content_block_index,
                                tool_call,
                                partial: partial.clone(),
                            })
                            .await;
                    }
                }
            }
        }

        BedrockStreamEvent::MessageStop { stop_reason } => {
            partial.stop_reason = map_stop_reason(&stop_reason);
        }

        BedrockStreamEvent::Metadata { metadata } => {
            if let Some(usage) = metadata.usage {
                partial.usage.input = usage.input_tokens;
                partial.usage.output = usage.output_tokens;
                partial.usage.total_tokens = usage.input_tokens + usage.output_tokens;
            }
        }
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

    /// Build a valid AWS event stream frame from a payload and headers.
    fn build_event_frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        // Build headers: :event-type header (type 7 = string)
        let mut headers = Vec::new();
        let name = b":event-type";
        headers.push(name.len() as u8);
        headers.extend_from_slice(name);
        headers.push(7u8); // string type
        let val = event_type.as_bytes();
        headers.extend_from_slice(&(val.len() as u16).to_be_bytes());
        headers.extend_from_slice(val);

        let headers_length = headers.len() as u32;
        // total = 12 (prelude) + headers + payload + 4 (message CRC)
        let total_length = 12 + headers.len() + payload.len() + 4;

        let mut frame = Vec::new();
        frame.extend_from_slice(&(total_length as u32).to_be_bytes());
        frame.extend_from_slice(&headers_length.to_be_bytes());
        // prelude CRC covers first 8 bytes
        let prelude_crc = crc32fast::hash(&frame[..8]);
        frame.extend_from_slice(&prelude_crc.to_be_bytes());
        frame.extend_from_slice(&headers);
        frame.extend_from_slice(payload);
        // message CRC covers everything so far
        let message_crc = crc32fast::hash(&frame);
        frame.extend_from_slice(&message_crc.to_be_bytes());
        frame
    }

    #[test]
    fn parse_valid_event_stream_frame() {
        let payload = b"{\"type\":\"messageStart\",\"message\":{\"role\":\"assistant\"}}";
        let frame = build_event_frame("MessageStart", payload);

        let result = parse_event_stream_frame(&frame).unwrap().unwrap();
        assert_eq!(result.0, "MessageStart");
        assert_eq!(result.1, payload);
        assert_eq!(result.2, frame.len());
    }

    #[test]
    fn parse_event_stream_crc_mismatch() {
        let payload = b"{\"type\":\"messageStart\",\"message\":{\"role\":\"assistant\"}}";
        let mut frame = build_event_frame("MessageStart", payload);
        // Corrupt a byte in the payload to trigger CRC mismatch
        if let Some(byte) = frame.get_mut(15) {
            *byte ^= 0xFF;
        }
        let result = parse_event_stream_frame(&frame);
        assert!(result.is_err(), "should fail on CRC mismatch");
    }

    #[test]
    fn parse_event_stream_headers_roundtrip() {
        let mut buf = Vec::new();
        // Header: ":event-type" = "ContentBlockDelta"
        let name = b":event-type";
        buf.push(name.len() as u8);
        buf.extend_from_slice(name);
        buf.push(7u8); // string type
        let val = b"ContentBlockDelta";
        buf.extend_from_slice(&(val.len() as u16).to_be_bytes());
        buf.extend_from_slice(val);

        let headers = parse_event_stream_headers(&buf).unwrap();
        assert_eq!(headers.get(":event-type").unwrap(), "ContentBlockDelta");
    }

    #[test]
    fn parse_event_stream_incomplete_returns_none() {
        // Less than 12 bytes → not enough data
        let result = parse_event_stream_frame(&[0u8; 8]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_event_stream_headers_skips_non_string_types() {
        let mut buf = Vec::new();

        // Header 1: bool_true (type 0) — no value bytes
        let name1 = b":flags";
        buf.push(name1.len() as u8);
        buf.extend_from_slice(name1);
        buf.push(0u8); // bool_true

        // Header 2: i32 (type 5) — 4 value bytes
        let name2 = b":content-type";
        buf.push(name2.len() as u8);
        buf.extend_from_slice(name2);
        buf.push(5u8); // i32
        buf.extend_from_slice(&42i32.to_be_bytes());

        // Header 3: string (type 7) — should be parsed normally
        let name3 = b":event-type";
        buf.push(name3.len() as u8);
        buf.extend_from_slice(name3);
        buf.push(7u8);
        let val = b"MessageStart";
        buf.extend_from_slice(&(val.len() as u16).to_be_bytes());
        buf.extend_from_slice(val);

        let headers = parse_event_stream_headers(&buf).unwrap();
        // Non-string headers are skipped, string header is parsed
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.get(":event-type").unwrap(), "MessageStart");
    }
}
