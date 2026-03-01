pub mod agent_loop;
pub mod events;
pub mod hooks;
pub mod state;

pub use agent_loop::{Agent, ContextTransformFn};
pub use events::{AgentEndReason, AgentEvent};
pub use hooks::{
    resolve_hook_results, HookContext, HookEvent, HookHandler, HookOutcome, HookRegistry,
    HookResult,
};
pub use state::{AgentConfig, AgentSharedState, AgentState};
