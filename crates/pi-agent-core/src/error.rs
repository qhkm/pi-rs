use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error(transparent)]
    Ai(#[from] pi_ai::PiAiError),
    #[error("Tool execution failed: {tool_name}: {message}")]
    ToolExecution { tool_name: String, message: String },
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    #[error("Tool validation failed: {tool_name}: {message}")]
    ToolValidation { tool_name: String, message: String },
    #[error("Session error: {0}")]
    Session(String),
    #[error("Context overflow: used {used} tokens, limit {limit}")]
    ContextOverflow { used: u64, limit: u64 },
    #[error("Agent aborted")]
    Aborted,
    #[error("Max turns reached: {0}")]
    MaxTurns(usize),
    #[error("No provider configured")]
    NoProvider,
    #[error("Compaction failed: {0}")]
    Compaction(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, AgentError>;
