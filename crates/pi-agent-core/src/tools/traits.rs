use async_trait::async_trait;
use serde_json::Value;

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// The output content (shown to the LLM)
    pub content: String,
    /// Whether the execution resulted in an error
    pub is_error: bool,
    /// Optional metadata (not sent to LLM, for UI/logging)
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            metadata: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Progress update during streaming tool execution
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Execution started
    Started,
    /// Intermediate progress update
    Update(String),
    /// Partial output available
    PartialOutput(String),
}

/// Context provided to tools during execution
pub struct ToolContext {
    /// Current working directory
    pub cwd: String,
    /// Abort signal - tool should check this periodically
    pub abort: tokio::sync::watch::Receiver<bool>,
    /// Optional environment variables
    pub env: std::collections::HashMap<String, String>,
}

impl ToolContext {
    pub fn new(cwd: String) -> Self {
        let (_, abort) = tokio::sync::watch::channel(false);
        Self {
            cwd,
            abort,
            env: std::collections::HashMap::new(),
        }
    }

    pub fn with_abort(mut self, abort: tokio::sync::watch::Receiver<bool>) -> Self {
        self.abort = abort;
        self
    }

    /// Check if the tool should abort
    pub fn is_aborted(&self) -> bool {
        *self.abort.borrow()
    }
}

/// The core AgentTool trait. All tools the agent can call implement this.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Tool name (must be unique within a tool registry)
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given arguments
    async fn execute(&self, args: Value, ctx: &ToolContext) -> crate::Result<ToolResult>;

    /// Execute with streaming progress updates.
    /// Default implementation delegates to execute().
    async fn execute_streaming(
        &self,
        args: Value,
        ctx: &ToolContext,
        progress_tx: tokio::sync::mpsc::Sender<ToolProgress>,
    ) -> crate::Result<ToolResult> {
        let _ = progress_tx.send(ToolProgress::Started).await;
        self.execute(args, ctx).await
    }

    /// Whether this tool requires explicit user approval before execution
    fn requires_approval(&self) -> bool {
        false
    }

    /// Short description for compact/token-constrained contexts
    fn compact_description(&self) -> &str {
        self.description()
    }

    /// Convert to a pi_ai ToolDefinition for sending to the LLM
    fn to_tool_definition(&self) -> pi_ai::ToolDefinition {
        pi_ai::ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    /// Clone this tool into a boxed trait object.
    /// Required for spawning tool execution in async tasks.
    fn clone_boxed(&self) -> Box<dyn AgentTool>;
}

// Make Box<dyn AgentTool> implement AgentTool for cloning
#[async_trait]
impl AgentTool for Box<dyn AgentTool> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> &str {
        (**self).description()
    }

    fn parameters_schema(&self) -> Value {
        (**self).parameters_schema()
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> crate::Result<ToolResult> {
        (**self).execute(args, ctx).await
    }

    async fn execute_streaming(
        &self,
        args: Value,
        ctx: &ToolContext,
        progress_tx: tokio::sync::mpsc::Sender<ToolProgress>,
    ) -> crate::Result<ToolResult> {
        (**self).execute_streaming(args, ctx, progress_tx).await
    }

    fn requires_approval(&self) -> bool {
        (**self).requires_approval()
    }

    fn compact_description(&self) -> &str {
        (**self).compact_description()
    }

    fn to_tool_definition(&self) -> pi_ai::ToolDefinition {
        (**self).to_tool_definition()
    }

    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        (**self).clone_boxed()
    }
}
