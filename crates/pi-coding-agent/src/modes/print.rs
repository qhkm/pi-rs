use anyhow::Result;
use pi_agent_core::{Agent, AgentEvent};
use pi_ai::Message;

/// Run in print mode: send a single prompt, print the response, exit.
pub async fn run_print_mode(agent: &Agent, prompt: &str) -> Result<()> {
    run_print_mode_message(agent, Message::user(prompt)).await
}

/// Run in print mode with a fully constructed user message.
pub async fn run_print_mode_message(agent: &Agent, message: Message) -> Result<()> {
    let mut rx = agent.subscribe();

    // Spawn event printer
    let printer = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                AgentEvent::MessageUpdate { event, .. } => {
                    if let pi_ai::StreamEvent::TextDelta { delta, .. } = event {
                        print!("{}", delta);
                    }
                }
                AgentEvent::AgentEnd { .. } => break,
                _ => {}
            }
        }
    });

    let result = agent.prompt_message(message).await;
    let _ = printer.await;
    println!(); // Final newline

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            Err(e.into())
        }
    }
}
