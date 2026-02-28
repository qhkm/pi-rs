use thiserror::Error;

#[derive(Error, Debug)]
pub enum PiAiError {
    #[error("Provider error: {provider}: {message}")]
    Provider { provider: String, message: String },
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Stream closed unexpectedly")]
    StreamClosed,
    #[error("Invalid configuration: {0}")]
    Config(String),
    #[error("Authentication error: {0}")]
    Auth(String),
    #[error("Rate limited: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Unsupported feature: {0}")]
    Unsupported(String),
    #[error("Aborted")]
    Aborted,
}

pub type Result<T> = std::result::Result<T, PiAiError>;
