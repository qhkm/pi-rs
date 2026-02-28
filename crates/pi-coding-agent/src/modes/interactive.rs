use anyhow::Result;
use pi_agent_core::Agent;

/// Run in interactive TUI mode
pub async fn run_interactive_mode(_agent: Agent) -> Result<()> {
    // TODO: Implement full TUI with pi_tui components
    // For now, use a simple line-based REPL
    use std::io::{self, BufRead, Write};

    println!("pi interactive mode (type 'exit' to quit)");
    println!("---");

    // This is a placeholder - the full TUI mode will use pi_tui
    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // EOF
        }

        let trimmed = line.trim();
        if trimmed == "exit" || trimmed == "/quit" {
            break;
        }
        if trimmed.is_empty() {
            continue;
        }

        // TODO: wire to agent.prompt() with streaming event display
        println!("(interactive mode not yet fully implemented)");
    }
    Ok(())
}
