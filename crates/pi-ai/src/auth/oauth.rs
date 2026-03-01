//! OAuth authentication for AI providers.
//!
//! Supports GitHub Copilot, Google Gemini CLI, and OpenAI Codex OAuth flows.

use crate::error::{PiAiError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ─── OAuth Client ID Constants ───────────────────────────────────────────────
// Centralized to avoid duplication (C3 fix).

/// GitHub Copilot OAuth client ID.
const GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// Google Gemini CLI OAuth client ID.
const GOOGLE_GEMINI_CLIENT_ID: &str =
    "935744712886-p8k8r4irgb56quh6i4r1sf4b5mha4ls1.apps.googleusercontent.com";
/// OpenAI Codex CLI OAuth client ID.
const OPENAI_CODEX_CLIENT_ID: &str = "openai-codex-cli";

/// OAuth provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// The OAuth provider name.
    pub provider: String,
    /// OAuth authorization endpoint.
    pub auth_url: String,
    /// OAuth token endpoint.
    pub token_url: String,
    /// Client ID for the application.
    pub client_id: String,
    /// Client secret (optional for PKCE flows).
    pub client_secret: Option<String>,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// PKCE code challenge method (S256 or plain).
    pub pkce_method: Option<String>,
    /// Additional parameters for the auth URL.
    #[serde(default)]
    pub extra_auth_params: HashMap<String, String>,
}

impl OAuthConfig {
    /// GitHub Copilot OAuth configuration.
    pub fn github_copilot() -> Self {
        Self {
            provider: "github-copilot".to_string(),
            auth_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            client_id: GITHUB_COPILOT_CLIENT_ID.to_string(),
            client_secret: None, // Uses device flow
            scopes: vec!["read:user".to_string()],
            pkce_method: None,
            extra_auth_params: HashMap::new(),
        }
    }

    /// Google Gemini CLI OAuth configuration.
    pub fn google_gemini() -> Self {
        Self {
            provider: "google-gemini".to_string(),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            client_id: GOOGLE_GEMINI_CLIENT_ID.to_string(),
            client_secret: None, // Uses installed app flow
            scopes: vec![
                "https://www.googleapis.com/auth/generative-language.retriever".to_string(),
                "https://www.googleapis.com/auth/cloud-platform".to_string(),
            ],
            pkce_method: Some("S256".to_string()),
            extra_auth_params: HashMap::new(),
        }
    }

    /// OpenAI Codex OAuth configuration.
    pub fn openai_codex() -> Self {
        Self {
            provider: "openai-codex".to_string(),
            auth_url: "https://auth.openai.com/authorize".to_string(),
            token_url: "https://auth.openai.com/token".to_string(),
            client_id: OPENAI_CODEX_CLIENT_ID.to_string(),
            client_secret: None,
            scopes: vec!["codex".to_string()],
            pkce_method: Some("S256".to_string()),
            extra_auth_params: HashMap::new(),
        }
    }

    /// Get the configuration for a named provider.
    pub fn for_provider(provider: &str) -> Option<Self> {
        match provider {
            "github-copilot" => Some(Self::github_copilot()),
            "google-gemini" => Some(Self::google_gemini()),
            "openai-codex" => Some(Self::openai_codex()),
            _ => None,
        }
    }
}

/// OAuth token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    /// Access token.
    pub access_token: String,
    /// Token type (usually "Bearer").
    pub token_type: String,
    /// Refresh token (if provided).
    pub refresh_token: Option<String>,
    /// Expiration time in seconds.
    pub expires_in: Option<u64>,
    /// Scope granted.
    pub scope: Option<String>,
}

/// Stored token with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    /// The access token.
    pub access_token: String,
    /// Refresh token (if available).
    pub refresh_token: Option<String>,
    /// Token type.
    pub token_type: String,
    /// When the token was obtained.
    pub obtained_at: u64,
    /// When the token expires (Unix timestamp, 0 if unknown).
    pub expires_at: u64,
    /// Scopes granted.
    pub scopes: Vec<String>,
    /// Provider name.
    pub provider: String,
}

impl StoredToken {
    /// Create a new stored token from a response.
    pub fn from_response(response: TokenResponse, provider: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = response.expires_in.map(|secs| now + secs).unwrap_or(0);

        Self {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            token_type: response.token_type,
            obtained_at: now,
            expires_at,
            scopes: response.scope.map(|s| s.split(' ').map(String::from).collect()).unwrap_or_default(),
            provider: provider.to_string(),
        }
    }

    /// Check if the token is expired (with 5 minute buffer).
    pub fn is_expired(&self) -> bool {
        if self.expires_at == 0 {
            return false; // Unknown expiration, assume valid
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now >= self.expires_at.saturating_sub(300) // 5 minute buffer
    }

    /// Check if the token can be refreshed.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// PKCE code verifier generator.
#[derive(Debug)]
pub struct PkceVerifier {
    /// The code verifier string.
    pub verifier: String,
    /// The code challenge string.
    pub challenge: String,
    /// The challenge method (S256 or plain).
    pub method: String,
}

impl PkceVerifier {
    /// Generate a new PKCE verifier with S256 method.
    pub fn new_s256() -> Self {
        use rand::Rng;
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        let mut rng = rand::thread_rng();
        
        // Generate 128-character verifier
        let verifier: String = (0..128)
            .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
            .collect();

        // Compute S256 challenge
        let challenge = Self::compute_challenge(&verifier);

        Self {
            verifier,
            challenge,
            method: "S256".to_string(),
        }
    }

    /// Compute the S256 code challenge.
    fn compute_challenge(verifier: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};
        
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        URL_SAFE_NO_PAD.encode(hash)
    }
}

/// OAuth manager for handling authentication flows.
pub struct OAuthManager {
    /// Token storage backend.
    storage: TokenStorage,
    /// HTTP client for token requests.
    client: reqwest::Client,
}

impl OAuthManager {
    /// Create a new OAuth manager with file-based storage.
    pub fn new() -> Result<Self> {
        Ok(Self {
            storage: TokenStorage::new()?,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| PiAiError::Config(e.to_string()))?,
        })
    }

    /// Create a new OAuth manager with custom storage path.
    pub fn with_storage_path(path: PathBuf) -> Result<Self> {
        Ok(Self {
            storage: TokenStorage::with_path(path)?,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| PiAiError::Config(e.to_string()))?,
        })
    }

    /// Start a device flow authentication.
    ///
    /// Returns the device code and verification URI for the user to visit.
    pub async fn start_device_flow(&self, provider: &str) -> Result<DeviceFlowResponse> {
        // Validate provider name up front.
        let _config = OAuthConfig::for_provider(provider)
            .ok_or_else(|| PiAiError::Auth(format!("Unknown OAuth provider: {provider}")))?;

        match provider {
            "github-copilot" => self.github_device_flow().await,
            "google-gemini" => self.google_device_flow().await,
            "openai-codex" => self.openai_device_flow().await,
            _ => Err(PiAiError::Auth(format!("Device flow not supported for {provider}"))),
        }
    }

    /// Poll for device flow completion.
    pub async fn poll_device_flow(&self, device_code: &str, provider: &str) -> Result<StoredToken> {
        match provider {
            "github-copilot" => self.poll_github_device_flow(device_code).await,
            "google-gemini" => self.poll_google_device_flow(device_code).await,
            "openai-codex" => self.poll_openai_device_flow(device_code).await,
            _ => Err(PiAiError::Auth(format!("Device flow not supported for {provider}"))),
        }
    }

    /// Get a valid access token for a provider.
    /// 
    /// If the stored token is expired and has a refresh token, it will be refreshed.
    pub async fn get_access_token(&self, provider: &str) -> Result<String> {
        // Try to load existing token
        if let Some(token) = self.storage.load_token(provider)? {
            if !token.is_expired() {
                return Ok(token.access_token);
            }
            
            // Token expired, try to refresh
            if token.can_refresh() {
                match self.refresh_token(&token).await {
                    Ok(new_token) => {
                        self.storage.save_token(provider, &new_token)?;
                        return Ok(new_token.access_token);
                    }
                    Err(e) => {
                        // Refresh failed, continue to re-auth
                        eprintln!("Token refresh failed: {e}");
                    }
                }
            }
        }

        Err(PiAiError::Auth(format!(
            "No valid token for {provider}. Please authenticate first."
        )))
    }

    /// Refresh an access token.
    async fn refresh_token(&self, token: &StoredToken) -> Result<StoredToken> {
        let refresh_token = token.refresh_token.as_ref()
            .ok_or_else(|| PiAiError::Auth("No refresh token available".to_string()))?;

        let config = OAuthConfig::for_provider(&token.provider)
            .ok_or_else(|| PiAiError::Auth(format!("Unknown provider: {}", token.provider)))?;

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &config.client_id),
        ];

        let response = self.client
            .post(&config.token_url)
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Token refresh failed: {error_text}")));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(StoredToken::from_response(token_response, &token.provider))
    }

    /// GitHub device flow implementation.
    async fn github_device_flow(&self) -> Result<DeviceFlowResponse> {
        let params = [
            ("client_id", GITHUB_COPILOT_CLIENT_ID),
            ("scope", "read:user"),
        ];

        let response = self.client
            .post("https://github.com/login/device/code")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Device flow initiation failed: {error_text}")));
        }

        let device_response: GitHubDeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(DeviceFlowResponse {
            device_code: device_response.device_code,
            user_code: device_response.user_code,
            verification_uri: device_response.verification_uri,
            expires_in: device_response.expires_in,
            interval: device_response.interval,
        })
    }

    /// Poll GitHub for device flow completion.
    async fn poll_github_device_flow(&self, device_code: &str) -> Result<StoredToken> {
        let params = [
            ("client_id", GITHUB_COPILOT_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let response = self.client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Token request failed: {error_text}")));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        // For GitHub Copilot, we need to exchange the GitHub token for a Copilot token
        let copilot_token = self.exchange_github_for_copilot(&token_response.access_token).await?;
        
        Ok(StoredToken::from_response(
            TokenResponse {
                access_token: copilot_token,
                token_type: token_response.token_type,
                refresh_token: token_response.refresh_token,
                expires_in: token_response.expires_in,
                scope: token_response.scope,
            },
            "github-copilot"
        ))
    }

    /// Exchange GitHub token for Copilot token.
    async fn exchange_github_for_copilot(&self, github_token: &str) -> Result<String> {
        let response = self.client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("Authorization", format!("Bearer {}", github_token))
            .header("Accept", "application/json")
            .send()
            .await
            ?;

        if !response.status().is_success() {
            return Err(PiAiError::Auth("Failed to get Copilot token".to_string()));
        }

        let copilot_response: CopilotTokenResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(copilot_response.token)
    }

    /// Google device flow implementation.
    async fn google_device_flow(&self) -> Result<DeviceFlowResponse> {
        let params = [
            ("client_id", GOOGLE_GEMINI_CLIENT_ID),
            ("scope", "https://www.googleapis.com/auth/generative-language.retriever https://www.googleapis.com/auth/cloud-platform"),
        ];

        let response = self.client
            .post("https://oauth2.googleapis.com/device/code")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Device flow initiation failed: {error_text}")));
        }

        let device_response: GoogleDeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(DeviceFlowResponse {
            device_code: device_response.device_code,
            user_code: device_response.user_code,
            verification_uri: device_response.verification_url,
            expires_in: device_response.expires_in,
            interval: device_response.interval,
        })
    }

    /// Poll Google for device flow completion.
    async fn poll_google_device_flow(&self, device_code: &str) -> Result<StoredToken> {
        let params = [
            ("client_id", GOOGLE_GEMINI_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let response = self.client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Token request failed: {error_text}")));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(StoredToken::from_response(token_response, "google-gemini"))
    }

    /// OpenAI device flow implementation.
    async fn openai_device_flow(&self) -> Result<DeviceFlowResponse> {
        // OpenAI uses a custom device flow
        let params = [
            ("client_id", OPENAI_CODEX_CLIENT_ID),
            ("scope", "codex"),
        ];

        let response = self.client
            .post("https://auth.openai.com/device/code")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Device flow initiation failed: {error_text}")));
        }

        let device_response: OpenAIDeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(DeviceFlowResponse {
            device_code: device_response.device_code,
            user_code: device_response.user_code,
            verification_uri: device_response.verification_uri,
            expires_in: device_response.expires_in,
            interval: device_response.interval.unwrap_or(5),
        })
    }

    /// Poll OpenAI for device flow completion.
    async fn poll_openai_device_flow(&self, device_code: &str) -> Result<StoredToken> {
        let params = [
            ("client_id", OPENAI_CODEX_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];

        let response = self.client
            .post("https://auth.openai.com/token")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            ?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PiAiError::Auth(format!("Token request failed: {error_text}")));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(StoredToken::from_response(token_response, "openai-codex"))
    }

    /// Save a token to storage.
    pub fn save_token(&self, provider: &str, token: &StoredToken) -> Result<()> {
        self.storage.save_token(provider, token)
    }

    /// Load a token from storage.
    pub fn load_token(&self, provider: &str) -> Result<Option<StoredToken>> {
        self.storage.load_token(provider)
    }

    /// Delete a token from storage.
    pub fn delete_token(&self, provider: &str) -> Result<()> {
        self.storage.delete_token(provider)
    }

    /// List all stored tokens.
    pub fn list_tokens(&self) -> Result<Vec<String>> {
        self.storage.list_tokens()
    }
}

impl Default for OAuthManager {
    fn default() -> Self {
        // Fallback: if we cannot create a storage directory, use a temporary one.
        Self::new().unwrap_or_else(|_| {
            let tmp = std::env::temp_dir().join("pi-ai-tokens");
            Self::with_storage_path(tmp).expect("cannot create even a temp OAuthManager")
        })
    }
}

/// Device flow response.
#[derive(Debug, Clone)]
pub struct DeviceFlowResponse {
    /// Device code for polling.
    pub device_code: String,
    /// User code to display to the user.
    pub user_code: String,
    /// Verification URI for the user to visit.
    pub verification_uri: String,
    /// How long the device code is valid (seconds).
    pub expires_in: u64,
    /// Polling interval (seconds).
    pub interval: u64,
}

/// GitHub device code response.
#[derive(Debug, Deserialize)]
struct GitHubDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// Google device code response.
#[derive(Debug, Deserialize)]
struct GoogleDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

/// OpenAI device code response.
#[derive(Debug, Deserialize)]
struct OpenAIDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

/// Copilot token response.
#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
}

/// Token storage with AES-256-GCM encryption.
pub struct TokenStorage {
    /// Base directory for token storage.
    base_path: PathBuf,
}

impl TokenStorage {
    /// Create a new token storage in the default location.
    ///
    /// The token directory is created with mode 700 (owner-only) on Unix.
    pub fn new() -> Result<Self> {
        let base_path = dirs::data_dir()
            .ok_or_else(|| PiAiError::Config("Could not find data directory".to_string()))?
            .join("pi-ai")
            .join("tokens");

        Self::create_secure_dir(&base_path)?;
        Ok(Self { base_path })
    }

    /// Create token storage with a custom path.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        Self::create_secure_dir(&path)?;
        Ok(Self { base_path: path })
    }

    /// Create a directory with mode 700 (owner-only) on Unix.
    fn create_secure_dir(path: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Get the path for a provider's token file.
    fn token_path(&self, provider: &str) -> PathBuf {
        self.base_path.join(format!("{}.json.enc", provider))
    }

    /// Save a token (encrypted with AES-256-GCM).
    pub fn save_token(&self, provider: &str, token: &StoredToken) -> Result<()> {
        let json = serde_json::to_string(token)
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        let encrypted = self.encrypt(json.as_bytes())?;
        let path = self.token_path(provider);
        std::fs::write(&path, encrypted)?;

        // Set file permissions to 600 (owner read/write only) on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Load a token (decrypted).
    pub fn load_token(&self, provider: &str) -> Result<Option<StoredToken>> {
        let path = self.token_path(provider);
        if !path.exists() {
            // Also check for legacy unencrypted file during migration.
            let legacy = self.base_path.join(format!("{}.json", provider));
            if !legacy.exists() {
                return Ok(None);
            }
            // Legacy path: read the old file, re-save encrypted, delete legacy.
            let data = std::fs::read_to_string(&legacy)?;
            if let Ok(token) = serde_json::from_str::<StoredToken>(&data) {
                let _ = self.save_token(provider, &token);
                let _ = std::fs::remove_file(&legacy);
                return Ok(Some(token));
            }
            return Ok(None);
        }

        let encrypted = std::fs::read(&path)?;
        let decrypted = self.decrypt(&encrypted)?;

        let token: StoredToken = serde_json::from_slice(&decrypted)
            .map_err(|e| PiAiError::Config(e.to_string()))?;

        Ok(Some(token))
    }

    /// Delete a token.
    pub fn delete_token(&self, provider: &str) -> Result<()> {
        let path = self.token_path(provider);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// List all stored token providers.
    pub fn list_tokens(&self) -> Result<Vec<String>> {
        let mut providers = Vec::new();
        for entry in std::fs::read_dir(&self.base_path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(provider) = name.strip_suffix(".json.enc") {
                providers.push(provider.to_string());
            } else if let Some(provider) = name.strip_suffix(".json") {
                providers.push(provider.to_string());
            }
        }
        Ok(providers)
    }

    // ── AES-256-GCM encryption ─────────────────────────────────────────────

    /// Encrypt plaintext using AES-256-GCM.
    ///
    /// Output: `nonce (12 bytes) || ciphertext+tag`.
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};

        let key_bytes = self.derive_aes_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|e| PiAiError::Auth(format!("AES init error: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ct = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| PiAiError::Auth(format!("Encryption error: {e}")))?;

        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt ciphertext produced by [`Self::encrypt`].
    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};

        if data.len() < 12 {
            return Err(PiAiError::Auth("Ciphertext too short".to_string()));
        }
        let key_bytes = self.derive_aes_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|e| PiAiError::Auth(format!("AES init error: {e}")))?;

        let nonce = Nonce::from_slice(&data[..12]);
        cipher
            .decrypt(nonce, &data[12..])
            .map_err(|e| PiAiError::Auth(format!("Decryption error: {e}")))
    }

    /// Derive a 32-byte AES key from the seed using HKDF-SHA256.
    fn derive_aes_key(&self) -> Result<[u8; 32]> {
        let seed = self.get_key_seed()?;
        type HmacSha256 = hmac::Hmac<sha2::Sha256>;
        let salt = b"pi-ai-token-encryption-v2";
        // Extract
        let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(salt)
            .map_err(|e| PiAiError::Auth(format!("HKDF error: {e}")))?;
        hmac::Mac::update(&mut mac, seed.as_bytes());
        let prk = hmac::Mac::finalize(mac).into_bytes();
        // Expand (single 32-byte block)
        let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(&prk)
            .map_err(|e| PiAiError::Auth(format!("HKDF error: {e}")))?;
        hmac::Mac::update(&mut mac, b"aes-256-gcm-key");
        hmac::Mac::update(&mut mac, &[1u8]);
        let okm = hmac::Mac::finalize(mac).into_bytes();
        let mut key = [0u8; 32];
        key.copy_from_slice(&okm);
        Ok(key)
    }

    /// Obtain the raw key seed. Prefers the OS keyring; falls back to a
    /// machine-specific value.
    fn get_key_seed(&self) -> Result<String> {
        // Skip the keyring in test builds to avoid interactive macOS Keychain
        // authorization prompts that would block the test runner.
        #[cfg(not(test))]
        if let Ok(seed) = self.get_seed_from_keyring() {
            return Ok(seed);
        }
        self.get_machine_key()
    }

    /// Get or create a random seed stored in the OS keyring.
    fn get_seed_from_keyring(&self) -> Result<String> {
        use keyring::Entry;

        let entry = Entry::new("pi-ai", "token-encryption-key")
            .map_err(|e| PiAiError::Auth(e.to_string()))?;

        match entry.get_password() {
            Ok(key) => Ok(key),
            Err(_) => {
                use rand::Rng;
                let key: String = (0..32)
                    .map(|_| rand::thread_rng().gen_range(33u8..127u8) as char)
                    .collect();
                entry
                    .set_password(&key)
                    .map_err(|e| PiAiError::Auth(e.to_string()))?;
                Ok(key)
            }
        }
    }

    /// Get a machine-specific key as fallback when the keyring is unavailable.
    fn get_machine_key(&self) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("ioreg")
                .args(["-rd1", "-c", "IOPlatformExpertDevice"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().find(|l| l.contains("IOPlatformUUID")) {
                    if let Some(uuid) = line.split('"').nth(3) {
                        return Ok(uuid.to_string());
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
                let trimmed = id.trim();
                if !trimmed.is_empty() {
                    return Ok(trimmed.to_string());
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Try to read the Windows MachineGuid from the registry.
            if let Ok(output) = std::process::Command::new("reg")
                .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Output line format: "    MachineGuid    REG_SZ    <guid>"
                for line in stdout.lines() {
                    if line.contains("MachineGuid") {
                        if let Some(guid) = line.split_whitespace().last() {
                            if !guid.is_empty() {
                                return Ok(guid.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Ultimate fallback: generate a random key and persist it to a file
        // within the secure token directory. This is stronger than deriving
        // from environment variables since an attacker cannot predict the key
        // from system metadata alone.
        let fallback_key_path = self.base_path.join(".encryption-key");
        if let Ok(existing) = std::fs::read_to_string(&fallback_key_path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }

        tracing::warn!("Generating fallback encryption key — install a keyring backend for proper token security");
        use rand::Rng;
        let key: String = (0..64)
            .map(|_| rand::thread_rng().gen_range(33u8..127u8) as char)
            .collect();
        std::fs::write(&fallback_key_path, &key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fallback_key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(key)
    }
}

/// Convenience function to get an OAuth token for a provider.
pub async fn get_oauth_token(provider: &str) -> Result<String> {
    let manager = OAuthManager::new()?;
    manager.get_access_token(provider).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stored_token_expiration() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Token that expires in 1 hour
        let token = StoredToken {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            obtained_at: now,
            expires_at: now + 3600,
            scopes: vec![],
            provider: "test".to_string(),
        };
        assert!(!token.is_expired());

        // Token that expired 10 minutes ago
        let expired_token = StoredToken {
            access_token: "test".to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            obtained_at: now - 4000,
            expires_at: now - 600,
            scopes: vec![],
            provider: "test".to_string(),
        };
        assert!(expired_token.is_expired());
    }

    #[test]
    fn test_pkce_verifier() {
        let pkce = PkceVerifier::new_s256();
        assert_eq!(pkce.verifier.len(), 128);
        assert!(!pkce.challenge.is_empty());
        assert_eq!(pkce.method, "S256");
    }

    #[test]
    fn test_aes_gcm_encryption_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pi-ai-test-{}", uuid::Uuid::new_v4()));
        let storage = TokenStorage::with_path(dir.clone()).unwrap();

        let plaintext = b"sensitive token data";
        let encrypted = storage.encrypt(plaintext).unwrap();

        // Ciphertext should differ from plaintext (unless key is all zeros, unlikely)
        assert_ne!(encrypted, plaintext);

        let decrypted = storage.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_oauth_config_providers() {
        assert!(OAuthConfig::for_provider("github-copilot").is_some());
        assert!(OAuthConfig::for_provider("google-gemini").is_some());
        assert!(OAuthConfig::for_provider("openai-codex").is_some());
        assert!(OAuthConfig::for_provider("unknown").is_none());
    }
}
