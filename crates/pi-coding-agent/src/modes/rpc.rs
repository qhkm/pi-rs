use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use pi_agent_core::{Agent, AgentState};
use pi_ai::{Content, Message};
use std::path::Path;
use tokio::task::LocalSet;

use super::rpc_types::*;

/// Run in RPC mode: JSON protocol over stdin/stdout for IDE integration.
///
/// Commands arrive as JSON-lines on stdin. Responses and events are written as
/// JSON-lines on stdout. Diagnostic messages go to stderr.
pub async fn run_rpc_mode(agent: &Agent) -> Result<()> {
    let local = LocalSet::new();
    local.run_until(rpc_loop(agent)).await
}

/// The main RPC loop: subscribe to agent events, read stdin commands, dispatch.
async fn rpc_loop(agent: &Agent) -> Result<()> {
    // Track whether a prompt is currently running so we can reject concurrent prompts.
    let is_prompting = Arc::new(AtomicBool::new(false));

    // Subscribe to agent events and spawn a local task that forwards them to stdout.
    let mut event_rx = agent.subscribe();
    tokio::task::spawn_local(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if let Ok(event_json) = serde_json::to_value(&event) {
                        let rpc_event = RpcEvent {
                            event_type: "event".to_string(),
                            event: event_json,
                        };
                        if let Ok(line) = serde_json::to_string(&rpc_event) {
                            println!("{}", line);
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    eprintln!(
                        "[rpc] warning: missed {} events due to slow consumer",
                        skipped
                    );
                }
            }
        }
    });

    // Read commands from stdin line-by-line.
    // We use spawn_blocking because tokio::io::stdin().lines() can be problematic
    // with LocalSet, and we need non-blocking reads interleaved with local task polling.
    loop {
        let line = tokio::task::spawn_blocking(|| {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let mut buf = String::new();
            match stdin.lock().read_line(&mut buf) {
                Ok(0) => None, // EOF
                Ok(_) => Some(buf),
                Err(_) => None,
            }
        })
        .await?;

        let line = match line {
            None => break, // EOF — clean shutdown
            Some(l) => l,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<RpcCommand>(trimmed) {
            Ok(command) => {
                handle_command(agent, &command, &is_prompting).await;
            }
            Err(e) => {
                let response =
                    RpcResponse::error(None, "parse", &format!("Failed to parse command: {e}"));
                if let Ok(json) = serde_json::to_string(&response) {
                    println!("{}", json);
                }
            }
        }
    }

    Ok(())
}

/// Dispatch a single RPC command.
///
/// For `Prompt`, the actual LLM call is spawned as a local task so the stdin
/// reader continues to accept commands (particularly `abort`). All other
/// commands are handled synchronously.
async fn handle_command(agent: &Agent, command: &RpcCommand, is_prompting: &Arc<AtomicBool>) {
    let id = command.id().map(|s| s.to_string());
    let cmd_type = command.type_name();

    let response = match command {
        RpcCommand::Prompt { message, .. } => {
            if is_prompting.load(Ordering::SeqCst) {
                RpcResponse::error(
                    id,
                    cmd_type,
                    "A prompt is already in progress. Send 'abort' first.",
                )
            } else {
                is_prompting.store(true, Ordering::SeqCst);

                // Spawn the prompt as a local task so we keep reading stdin.
                let processed = match crate::input::file_processor::process_input(
                    message,
                    Path::new(&agent.config.cwd),
                ) {
                    Ok(value) => value,
                    Err(e) => {
                        return print_response(RpcResponse::error(
                            id,
                            cmd_type,
                            &format!("Failed to process input: {e}"),
                        ));
                    }
                };
                let mut blocks = Vec::new();
                if !processed.text.is_empty() {
                    blocks.push(Content::text(processed.text));
                }
                blocks.extend(processed.images.iter().map(|img| img.to_content()));
                let input_message = if blocks.is_empty() {
                    Message::user("")
                } else {
                    Message::user_with_images(blocks)
                };
                let flag = is_prompting.clone();

                // SAFETY: `agent` is owned by the caller and lives for the entire
                // RPC session. The spawned local task runs on the same thread and
                // the LocalSet outlives it.
                let agent_ptr = agent as *const Agent;
                struct SendAgent(*const Agent);
                unsafe impl Send for SendAgent {}
                let wrapped = SendAgent(agent_ptr);

                tokio::task::spawn_local(async move {
                    let a = unsafe { &*wrapped.0 };
                    if let Err(e) = a.prompt_message(input_message).await {
                        // Prompt errors are already emitted as AgentEnd events,
                        // but log to stderr for debugging.
                        eprintln!("[rpc] prompt error: {e}");
                    }
                    flag.store(false, Ordering::SeqCst);
                });

                // Immediately acknowledge — results arrive as events.
                RpcResponse::success(id, cmd_type, None)
            }
        }

        RpcCommand::Abort { .. } => {
            agent.abort();
            RpcResponse::success(id, cmd_type, None)
        }

        RpcCommand::GetState { .. } => {
            let state = agent.state().await;
            let messages = agent.messages().await;
            let session_state = RpcSessionState {
                is_streaming: matches!(state, AgentState::Streaming),
                message_count: messages.len(),
                auto_compaction_enabled: agent.config.compaction.enabled,
            };
            match serde_json::to_value(&session_state) {
                Ok(data) => RpcResponse::success(id, cmd_type, Some(data)),
                Err(e) => RpcResponse::error(id, cmd_type, &format!("Serialization error: {e}")),
            }
        }

        RpcCommand::GetMessages { .. } => {
            let messages = agent.messages().await;
            match serde_json::to_value(&messages) {
                Ok(data) => RpcResponse::success(id, cmd_type, Some(data)),
                Err(e) => RpcResponse::error(id, cmd_type, &format!("Serialization error: {e}")),
            }
        }

        RpcCommand::Compact {
            custom_instructions,
            ..
        } => {
            // TODO: Wire to compaction when full compaction plumbing is in place.
            let _ = custom_instructions;
            RpcResponse::error(id, cmd_type, "Compaction not yet wired in RPC mode")
        }

        RpcCommand::SetAutoCompaction { enabled, .. } => {
            // TODO: Wire to a mutable config toggle once AgentConfig supports it.
            let _ = enabled;
            RpcResponse::success(id, cmd_type, None)
        }
    };

    print_response(response);
}

fn print_response(response: RpcResponse) {
    if let Ok(json) = serde_json::to_string(&response) {
        println!("{}", json);
    }
}
