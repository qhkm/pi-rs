//! OAuth authentication for AI providers.
//!
//! Supports GitHub Copilot, Google Gemini CLI, and OpenAI Codex OAuth flows.

use crate::error::{PiAiError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
            client_id: "Iv1.b507a08c87ecfe98".to_string(), // GitHub Copilot client ID
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
            client_id: "935744712886-p8k8r4irgb56quh6i4r1sf4b5mha4ls1.apps.googleusercontent.com".to_string(), // Gemini CLI client ID
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
            client_id: "openai-codex-cli".to_string(),
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
        let config = OAuthConfig::for_provider(provider)
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
            ("client_id", "Iv1.b507a08c87ecfe98"),
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
            ("client_id", "Iv1.b507a08c87ecfe98"),
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
            ("client_id", "935744712886-p8k8r4irgb56quh6i4r1sf4b5mha4ls1.apps.googleusercontent.com"),
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
            ("client_id", "935744712886-p8k8r4irgb56quh6i4r1sf4b5mha4ls1.apps.googleusercontent.com"),
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
            ("client_id", "openai-codex-cli"),
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
            ("client_id", "openai-codex-cli"),
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
        Self::new().expect("Failed to create OAuthManager")
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

/// Token storage with encryption.
pub struct TokenStorage {
    /// Base directory for token storage.
    base_path: PathBuf,
}

impl TokenStorage {
    /// Create a new token storage in the default location.
    pub fn new() -> Result<Self> {
        let base_path = dirs::data_dir()
            .ok_or_else(|| PiAiError::Config("Could not find data directory".to_string()))?
            .join("pi-ai")
            .join("tokens");
        
        std::fs::create_dir_all(&base_path)
            ?;
        
        Ok(Self { base_path })
    }

    /// Create token storage with a custom path.
    pub fn with_path(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path)
            ?;
        Ok(Self { base_path: path })
    }

    /// Get the path for a provider's token file.
    fn token_path(&self, provider: &str) -> PathBuf {
        self.base_path.join(format!("{}.json", provider))
    }

    /// Save a token (encrypted).
    pub fn save_token(&self, provider: &str, token: &StoredToken) -> Result<()> {
        let json = serde_json::to_string(token)
            .map_err(|e| PiAiError::Config(e.to_string()))?;
        
        // Encrypt the token before saving
        let encrypted = self.encrypt(&json)?;
        
        std::fs::write(self.token_path(provider), encrypted)
            ?;
        
        Ok(())
    }

    /// Load a token (decrypted).
    pub fn load_token(&self, provider: &str) -> Result<Option<StoredToken>> {
        let path = self.token_path(provider);
        
        if !path.exists() {
            return Ok(None);
        }
        
        let encrypted = std::fs::read_to_string(&path)
            ?;
        
        // Decrypt the token
        let json = self.decrypt(&encrypted)?;
        
        let token: StoredToken = serde_json::from_str(&json)
            .map_err(|e| PiAiError::Config(e.to_string()))?;
        
        Ok(Some(token))
    }

    /// Delete a token.
    pub fn delete_token(&self, provider: &str) -> Result<()> {
        let path = self.token_path(provider);
        if path.exists() {
            std::fs::remove_file(path)
                ?;
        }
        Ok(())
    }

    /// List all stored token providers.
    pub fn list_tokens(&self) -> Result<Vec<String>> {
        let mut providers = Vec::new();
        
        for entry in std::fs::read_dir(&self.base_path)
            ? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            
            if name.ends_with(".json") {
                providers.push(name.trim_end_matches(".json").to_string());
            }
        }
        
        Ok(providers)
    }

    /// Encrypt data using platform keyring or simple XOR as fallback.
    fn encrypt(&self, data: &str) -> Result<String> {
        // Try to use keyring for encryption key
        match self.get_encryption_key_from_keyring() {
            Ok(key) => Ok(xor_encrypt(data, &key)),
            Err(_) => {
                // Fallback: use machine-specific key
                let key = self.get_machine_key()?;
                Ok(xor_encrypt(data, &key))
            }
        }
    }

    /// Decrypt data.
    fn decrypt(&self, data: &str) -> Result<String> {
        // Try to use keyring for decryption key
        match self.get_encryption_key_from_keyring() {
            Ok(key) => Ok(xor_decrypt(data, &key)?),
            Err(_) => {
                // Fallback: use machine-specific key
                let key = self.get_machine_key()?;
                Ok(xor_decrypt(data, &key)?)
            }
        }
    }

    /// Get or create encryption key from keyring.
    fn get_encryption_key_from_keyring(&self) -> Result<String> {
        use keyring::Entry;
        
        let entry = Entry::new("pi-ai", "token-encryption-key")
            .map_err(|e| PiAiError::Auth(e.to_string()))?;
        
        match entry.get_password() {
            Ok(key) => Ok(key),
            Err(_) => {
                // Generate new key
                use rand::Rng;
                let mut rng = rand::thread_rng();
                let key: String = (0..32)
                    .map(|_| rng.gen_range(33..127) as u8 as char)
                    .collect();
                
                entry.set_password(&key)
                    .map_err(|e| PiAiError::Auth(e.to_string()))?;
                
                Ok(key)
            }
        }
    }

    /// Get machine-specific key as fallback.
    fn get_machine_key(&self) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            // Use machine serial number or hardware UUID on macOS
            if let Ok(output) = std::process::Command::new("ioreg")
                .args(["-rd1", "-c", "IOPlatformExpertDevice"])
                .output() {
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
            // Use machine-id on Linux
            if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
                return Ok(id.trim().to_string());
            }
        }
        
        // Ultimate fallback: use a static key (not secure, but functional)
        Ok("pi-ai-fallback-encryption-key-v1".to_string())
    }
}

/// Simple XOR encryption (for obfuscation, not high security).
fn xor_encrypt(data: &str, key: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    
    let encrypted: Vec<u8> = data
        .bytes()
        .zip(key.bytes().cycle())
        .map(|(d, k)| d ^ k)
        .collect();
    
    STANDARD.encode(encrypted)
}

/// Simple XOR decryption.
fn xor_decrypt(data: &str, key: &str) -> Result<String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    
    let encrypted = STANDARD
        .decode(data)
        .map_err(|e| PiAiError::Config(format!("Base64 decode error: {e}")))?;
    
    let decrypted: Vec<u8> = encrypted
        .iter()
        .zip(key.bytes().cycle())
        .map(|(&d, k)| d ^ k)
        .collect();
    
    String::from_utf8(decrypted)
        .map_err(|e| PiAiError::Config(format!("UTF-8 decode error: {e}")))
}

/// Convenience function to get an OAuth token for a provider.
pub async fn get_oauth_token(provider: &str) -> Result<String> {
    let manager = OAuthManager::new()?;
    manager.get_access_token(provider).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
    fn test_xor_encryption() {
        let key = "test-key-12345";
        let data = "sensitive token data";
        
        let encrypted = xor_encrypt(data, key);
        assert_ne!(encrypted, data);
        
        let decrypted = xor_decrypt(&encrypted, key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_oauth_config_providers() {
        assert!(OAuthConfig::for_provider("github-copilot").is_some());
        assert!(OAuthConfig::for_provider("google-gemini").is_some());
        assert!(OAuthConfig::for_provider("openai-codex").is_some());
        assert!(OAuthConfig::for_provider("unknown").is_none());
    }
}
