/// Google Vertex AI provider.
///
/// Implements the Google Vertex AI Gemini API with OAuth2/ADC authentication.
/// This is the enterprise version of Google's Gemini API.
///
/// Authentication options (in order of priority):
/// 1. GOOGLE_APPLICATION_CREDENTIALS environment variable pointing to a service account JSON
/// 2. gcloud application-default login credentials
/// 3. Workload identity (when running on GCP)
///
/// Required environment variables:
/// - GOOGLE_CLOUD_PROJECT or GOOGLE_VERTEX_PROJECT (your GCP project ID)
/// - GOOGLE_CLOUD_LOCATION or GOOGLE_VERTEX_LOCATION (e.g., "us-central1")
///
/// Optional:
/// - GOOGLE_APPLICATION_CREDENTIALS (path to service account key file)
///
/// **Authentication Methods:**
/// - Service account JSON key files (JWT signing with RS256)
/// - Application Default Credentials via `gcloud auth application-default login`
/// - Static access tokens

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

pub struct VertexProvider {
    client: reqwest::Client,
    project_id: String,
    location: String,
    credentials: VertexCredentials,
    access_token: tokio::sync::RwLock<Option<(String, chrono::DateTime<chrono::Utc>)>>,
}

#[derive(Debug, Clone)]
enum VertexCredentials {
    /// Service account key file
    ServiceAccount {
        client_email: String,
        private_key: String,
        token_uri: String,
    },
    /// Access token (from gcloud or workload identity)
    AccessToken(String),
    /// ADC (Application Default Credentials) - will be resolved at runtime
    Adc,
}

impl VertexProvider {
    /// Create a new Vertex AI provider with explicit credentials.
    pub fn new(
        project_id: impl Into<String>,
        location: impl Into<String>,
        credentials: VertexCredentials,
    ) -> Self {
        VertexProvider {
            client: build_http_client(300),
            project_id: project_id.into(),
            location: location.into(),
            credentials,
            access_token: tokio::sync::RwLock::new(None),
        }
    }

    /// Create from environment variables.
    ///
    /// Resolves:
    /// - Project: GOOGLE_CLOUD_PROJECT, GOOGLE_VERTEX_PROJECT
    /// - Location: GOOGLE_CLOUD_LOCATION, GOOGLE_VERTEX_LOCATION, defaults to "us-central1"
    /// - Credentials: GOOGLE_APPLICATION_CREDENTIALS or uses gcloud ADC
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GOOGLE_VERTEX_PROJECT"))
            .ok()?;
        
        let location = std::env::var("GOOGLE_CLOUD_LOCATION")
            .or_else(|_| std::env::var("GOOGLE_VERTEX_LOCATION"))
            .unwrap_or_else(|_| "us-central1".to_string());

        // Try to load credentials from GOOGLE_APPLICATION_CREDENTIALS
        let credentials = if let Ok(cred_path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
            match Self::load_service_account(&cred_path) {
                Ok(creds) => creds,
                Err(_) => VertexCredentials::Adc,
            }
        } else {
            VertexCredentials::Adc
        };

        Some(Self::new(project_id, location, credentials))
    }

    /// Load service account credentials from a JSON key file.
    fn load_service_account(path: &str) -> Result<VertexCredentials> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PiAiError::Auth(format!("Failed to read service account file: {}", e)))?;
        
        let json: Value = serde_json::from_str(&content)
            .map_err(|e| PiAiError::Auth(format!("Invalid service account JSON: {}", e)))?;

        let client_email = json["client_email"]
            .as_str()
            .ok_or_else(|| PiAiError::Auth("Missing client_email in service account".to_string()))?
            .to_string();
        
        let private_key = json["private_key"]
            .as_str()
            .ok_or_else(|| PiAiError::Auth("Missing private_key in service account".to_string()))?
            .to_string();
        
        let token_uri = json["token_uri"]
            .as_str()
            .unwrap_or("https://oauth2.googleapis.com/token")
            .to_string();

        Ok(VertexCredentials::ServiceAccount {
            client_email,
            private_key,
            token_uri,
        })
    }

    /// Get or refresh the access token.
    async fn get_access_token(&self) -> Result<String> {
        // Check if we have a cached token that's still valid
        {
            let cached = self.access_token.read().await;
            if let Some((token, expiry)) = cached.as_ref() {
                // Refresh if token expires within 5 minutes
                if *expiry > chrono::Utc::now() + chrono::Duration::minutes(5) {
                    return Ok(token.clone());
                }
            }
        }

        // Need to get/refresh token
        let new_token = match &self.credentials {
            VertexCredentials::AccessToken(token) => token.clone(),
            VertexCredentials::ServiceAccount { client_email, private_key, token_uri } => {
                self.fetch_service_account_token(client_email, private_key, token_uri).await?
            }
            VertexCredentials::Adc => {
                self.fetch_adc_token().await?
            }
        };

        let mut cached = self.access_token.write().await;
        // Token expires in 1 hour from now (standard for Google OAuth)
        *cached = Some((new_token.clone(), chrono::Utc::now() + chrono::Duration::hours(1)));
        
        Ok(new_token)
    }

    /// Fetch access token using service account credentials.
    ///
    /// Builds a JWT assertion signed with the service account's private key,
    /// then exchanges it for an access token via the token endpoint.
    async fn fetch_service_account_token(
        &self,
        client_email: &str,
        private_key: &str,
        token_uri: &str,
    ) -> Result<String> {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use serde::{Deserialize as De, Serialize as Se};

        #[derive(Se)]
        struct Claims<'a> {
            iss: &'a str,
            aud: &'a str,
            iat: i64,
            exp: i64,
            scope: &'a str,
        }

        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            iss: client_email,
            aud: token_uri,
            iat: now,
            exp: now + 3600,
            scope: "https://www.googleapis.com/auth/cloud-platform",
        };

        let header = Header::new(Algorithm::RS256);
        let encoding_key = EncodingKey::from_rsa_pem(private_key.as_bytes())
            .map_err(|e| PiAiError::Auth(format!("Invalid RSA private key: {e}")))?;

        let jwt = encode(&header, &claims, &encoding_key)
            .map_err(|e| PiAiError::Auth(format!("JWT signing failed: {e}")))?;

        // Exchange the JWT assertion for an access token
        #[derive(De)]
        struct TokenResponse {
            access_token: String,
        }

        let resp = self
            .client
            .post(token_uri)
            .form(&[
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:jwt-bearer",
                ),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| PiAiError::Auth(format!("Token exchange request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!(
                "Token exchange failed (HTTP {status}): {body}"
            )));
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .map_err(|e| PiAiError::Auth(format!("Failed to parse token response: {e}")))?;

        Ok(token_resp.access_token)
    }

    /// Fetch access token using Application Default Credentials.
    /// This reads from the gcloud credentials file.
    async fn fetch_adc_token(&self) -> Result<String> {
        // Try to read gcloud application-default credentials
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| PiAiError::Auth("Cannot find HOME directory for ADC".to_string()))?;
        
        let adc_path = format!("{}/.config/gcloud/application_default_credentials.json", home);
        
        let content = std::fs::read_to_string(&adc_path)
            .map_err(|_| PiAiError::Auth(
                "No ADC credentials found. Run 'gcloud auth application-default login' \
                 or set GOOGLE_APPLICATION_CREDENTIALS".to_string()
            ))?;
        
        let json: Value = serde_json::from_str(&content)
            .map_err(|e| PiAiError::Auth(format!("Invalid ADC JSON: {}", e)))?;

        // Check if this is a refresh token type
        if let Some(refresh_token) = json["refresh_token"].as_str() {
            let client_id = json["client_id"]
                .as_str()
                .ok_or_else(|| PiAiError::Auth("Missing client_id in ADC".to_string()))?;
            let client_secret = json["client_secret"]
                .as_str()
                .ok_or_else(|| PiAiError::Auth("Missing client_secret in ADC".to_string()))?;
            
            return self.exchange_refresh_token(refresh_token, client_id, client_secret).await;
        }

        // Check if this is an access token directly
        if let Some(access_token) = json["access_token"].as_str() {
            return Ok(access_token.to_string());
        }

        Err(PiAiError::Auth("Unrecognized ADC format".to_string()))
    }

    /// Exchange a refresh token for an access token.
    async fn exchange_refresh_token(
        &self,
        refresh_token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ];

        let response = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| PiAiError::Auth(format!("Token exchange failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Token exchange failed: {}", error_text)));
        }

        let token_response: Value = response
            .json()
            .await
            .map_err(|e| PiAiError::Auth(format!("Invalid token response: {}", e)))?;

        token_response["access_token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| PiAiError::Auth("No access_token in response".to_string()))
    }

    fn get_model_name(&self, model: &Model) -> String {
        // Vertex model names are like: gemini-1.5-pro-002
        // Remove any "models/" prefix
        model.id.trim_start_matches("models/").to_string()
    }
}

// ─── Request format conversion ────────────────────────────────────────────────

fn build_vertex_messages(messages: &[Message]) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(um) => {
                let parts = match &um.content {
                    UserContent::Text(t) => {
                        vec![json!({"text": t})]
                    }
                    UserContent::Blocks(blocks) => {
                        blocks
                            .iter()
                            .filter_map(|c| match c {
                                Content::Text { text, .. } => Some(json!({"text": text})),
                                Content::Image { data, mime_type } => Some(json!({
                                    "inlineData": {
                                        "mimeType": mime_type,
                                        "data": data
                                    }
                                })),
                                _ => None,
                            })
                            .collect()
                    }
                };
                result.push(json!({
                    "role": "user",
                    "parts": parts
                }));
            }
            Message::Assistant(am) => {
                let mut parts: Vec<Value> = Vec::new();

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
                    parts.push(json!({"text": text}));
                }

                // Add function calls (tool calls)
                for c in &am.content {
                    if let Content::ToolCall { id: _, name, arguments, .. } = c {
                        parts.push(json!({
                            "functionCall": {
                                "name": name,
                                "args": arguments
                            }
                        }));
                    }
                }

                result.push(json!({
                    "role": "model",
                    "parts": parts
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
                    "parts": [{
                        "functionResponse": {
                            "name": tr.tool_name,
                            "response": {
                                "result": text
                            }
                        }
                    }]
                }));
            }
        }
    }

    result
}

fn build_vertex_tools(tools: &[crate::tools::schema::ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "functionDeclarations": [{
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }]
            })
        })
        .collect()
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "SAFETY" => StopReason::Error,
        "RECITATION" => StopReason::Stop,
        "OTHER" => StopReason::Stop,
        _ => StopReason::Stop,
    }
}

// ─── SSE event types from Vertex AI ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VertexStreamResponse {
    candidates: Option<Vec<VertexCandidate>>,
    usage_metadata: Option<VertexUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VertexCandidate {
    content: Option<VertexContent>,
    finish_reason: Option<String>,
    #[allow(dead_code)]
    index: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct VertexContent {
    #[allow(dead_code)]
    role: Option<String>,
    parts: Option<Vec<VertexPart>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VertexPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<VertexFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct VertexFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VertexUsage {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
    total_token_count: Option<u64>,
}

// ─── LLMProvider implementation ───────────────────────────────────────────────

#[async_trait]
impl LLMProvider for VertexProvider {
    fn name(&self) -> &str {
        "google-vertex"
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
        let access_token = self.get_access_token().await?;
        let model_name = self.get_model_name(model);

        let url = format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:streamGenerateContent",
            self.location,
            self.project_id,
            self.location,
            model_name
        );

        let contents = build_vertex_messages(&context.messages);

        let mut body = json!({
            "contents": contents,
            "generationConfig": {}
        });

        if let Some(temp) = options.temperature {
            body["generationConfig"]["temperature"] = json!(temp);
        }

        if let Some(max_tokens) = options.max_tokens {
            body["generationConfig"]["maxOutputTokens"] = json!(max_tokens);
        } else {
            body["generationConfig"]["maxOutputTokens"] = json!(model.max_tokens);
        }

        // Add system prompt
        if let Some(sp) = &context.system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": sp}]
            });
        }

        // Add tools
        if !context.tools.is_empty() {
            body["tools"] = json!(build_vertex_tools(&context.tools));
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Provider {
                provider: "google-vertex".into(),
                message: format!("HTTP {}: {}", status, text),
            });
        }

        self.parse_stream_response(response, model, tx).await
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

impl VertexProvider {
    async fn parse_stream_response(
        &self,
        response: reqwest::Response,
        model: &Model,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        let mut partial = make_partial(model);
        let mut bytes_stream = response.bytes_stream();
        let mut buffer = String::new();

        let mut text_content_index: Option<usize> = None;
        // Track tool calls by content index. Use a counter to generate unique
        // keys so the same tool called twice gets separate entries (I4 fix).
        let mut tool_indices: HashMap<usize, usize> = HashMap::new();
        let mut tool_call_counter: usize = 0;

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

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() || line == "," {
                    continue;
                }

                let line = line.trim_end_matches(',');

                let resp: VertexStreamResponse = match serde_json::from_str(line) {
                    Ok(r) => r,
                    Err(e) => {
                        debug!("Failed to parse Vertex response line: {e} — line: {line}");
                        continue;
                    }
                };

                if let Some(usage) = &resp.usage_metadata {
                    if let Some(input) = usage.prompt_token_count {
                        partial.usage.input = input;
                    }
                    if let Some(output) = usage.candidates_token_count {
                        partial.usage.output = output;
                    }
                    if let Some(total) = usage.total_token_count {
                        partial.usage.total_tokens = total;
                    }
                }

                if let Some(candidates) = &resp.candidates {
                    for candidate in candidates {
                        if let Some(finish) = &candidate.finish_reason {
                            partial.stop_reason = map_stop_reason(finish);
                        }

                        if let Some(content) = &candidate.content {
                            if let Some(parts) = &content.parts {
                                for part in parts {
                                    // Handle text
                                    if let Some(text) = &part.text {
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

                                    // Handle function calls — each call gets a
                                    // unique content index so parallel calls
                                    // (even the same tool twice) are tracked
                                    // independently.
                                    if let Some(fc) = &part.function_call {
                                        let i = partial.content.len();
                                        let tool_id = format!("vertex_tool_{}", i);
                                        partial.content.push(Content::ToolCall {
                                            id: tool_id,
                                            name: fc.name.clone(),
                                            arguments: fc.args.clone(),
                                            thought_signature: None,
                                        });
                                        tool_indices.insert(tool_call_counter, i);
                                        tool_call_counter += 1;
                                        let _ = tx
                                            .send(StreamEvent::ToolCallStart {
                                                content_index: i,
                                                partial: partial.clone(),
                                            })
                                            .await;
                                        let ci = i;
                                        if let Some(Content::ToolCall {
                                            arguments: ref mut a,
                                            ..
                                        }) = partial.content.get_mut(ci)
                                        {
                                            *a = fc.args.clone();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Send end events for text
        if let Some(ci) = text_content_index {
            let text = partial
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
                    content: text,
                    partial: partial.clone(),
                })
                .await;
        }

        // Send end events for each tool call
        for &ci in tool_indices.values() {
            let tool_call = partial.content.get(ci).and_then(|c| {
                if let Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } = c
                {
                    Some(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    })
                } else {
                    None
                }
            });
            if let Some(tc) = tool_call {
                let _ = tx
                    .send(StreamEvent::ToolCallEnd {
                        content_index: ci,
                        tool_call: tc,
                        partial: partial.clone(),
                    })
                    .await;
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
    fn vertex_provider_creation() {
        let provider = VertexProvider::new(
            "my-project",
            "us-central1",
            VertexCredentials::Adc,
        );
        assert_eq!(provider.name(), "google-vertex");
        assert_eq!(provider.project_id, "my-project");
        assert_eq!(provider.location, "us-central1");
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason("MAX_TOKENS"), StopReason::Length);
        assert_eq!(map_stop_reason("SAFETY"), StopReason::Error);
    }

    #[test]
    fn jwt_claims_serialize() {
        use serde::Serialize;

        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            aud: &'a str,
            iat: i64,
            exp: i64,
            scope: &'a str,
        }

        let claims = Claims {
            iss: "test@project.iam.gserviceaccount.com",
            aud: "https://oauth2.googleapis.com/token",
            iat: 1700000000,
            exp: 1700003600,
            scope: "https://www.googleapis.com/auth/cloud-platform",
        };

        let json = serde_json::to_value(&claims).expect("claims should serialize");
        assert_eq!(
            json["iss"].as_str().unwrap(),
            "test@project.iam.gserviceaccount.com"
        );
        assert_eq!(
            json["aud"].as_str().unwrap(),
            "https://oauth2.googleapis.com/token"
        );
        assert_eq!(json["iat"].as_i64().unwrap(), 1700000000);
        assert_eq!(json["exp"].as_i64().unwrap(), 1700003600);
        assert_eq!(
            json["scope"].as_str().unwrap(),
            "https://www.googleapis.com/auth/cloud-platform"
        );
    }
}
