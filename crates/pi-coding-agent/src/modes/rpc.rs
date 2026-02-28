use anyhow::Result;
use pi_agent_core::Agent;

/// Run in RPC mode: communicate via stdin/stdout JSON messages
pub async fn run_rpc_mode(_agent: Agent) -> Result<()> {
    // TODO: Implement JSON-RPC 2.0 over stdin/stdout
    eprintln!("RPC mode not yet implemented");
    Ok(())
}
