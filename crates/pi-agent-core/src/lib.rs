pub mod agent;
pub mod context;
pub mod error;
pub mod messages;
pub mod proxy;
pub mod tools;

pub use error::{AgentError, Result};
pub use tools::{AgentTool, ToolContext, ToolProgress, ToolRegistry, ToolResult};

// Agent core re-exports
pub use agent::ContextTransformFn;
pub use agent::{Agent, AgentConfig, AgentSharedState, AgentState};
pub use agent::{AgentEndReason, AgentEvent};

// Hook system re-exports
pub use agent::{
    resolve_hook_results, HookContext, HookEvent, HookHandler, HookOutcome, HookRegistry,
    HookResult,
};
