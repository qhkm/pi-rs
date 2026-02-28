/// Full TUI interactive mode using pi-tui components.
///
/// This is a placeholder for the full TUI implementation.
/// The full implementation will include:
/// - Streaming markdown display
/// - Status bar with model, tokens, cost
/// - Multi-line editor with syntax highlighting
/// - Command palette for slash commands
/// - Tool execution visualization
use std::sync::Arc;

use anyhow::Result;
use pi_agent_core::Agent;

/// Run the full TUI interactive mode.
/// 
/// Currently this is a placeholder that falls back to the basic interactive mode.
/// The full implementation will use the pi-tui framework.
pub async fn run_tui_mode(agent: Arc<Agent>) -> Result<()> {
    // For now, fall back to the basic interactive mode
    // The full TUI implementation will be added in a future update
    tracing::info!("TUI mode requested, using basic interactive mode (full TUI coming soon)");
    
    // Import and run the basic interactive mode
    super::interactive::run_interactive_mode(agent).await
}
