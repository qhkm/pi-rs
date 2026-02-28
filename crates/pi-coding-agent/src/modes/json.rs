use anyhow::Result;
use pi_agent_core::proxy::ProxyEvent;
use pi_agent_core::{Agent, AgentEvent};

/// Run in JSON mode: emit JSONL events to stdout
pub async fn run_json_mode(agent: &Agent, prompt: &str) -> Result<()> {
    let mut rx = agent.subscribe();

    let printer = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match &event {
                AgentEvent::MessageUpdate {
                    event: stream_event,
                    ..
                } => {
                    let proxy = ProxyEvent::from_stream_event(stream_event);
                    if let Ok(json) = serde_json::to_string(&proxy) {
                        println!("{}", json);
                    }
                }
                AgentEvent::ToolExecutionStart {
                    tool_name,
                    call_id,
                    arguments,
                } => {
                    let json = serde_json::json!({
                        "type": "tool_start",
                        "tool": tool_name,
                        "id": call_id,
                        "arguments": arguments,
                    });
                    println!("{}", json);
                }
                AgentEvent::ToolExecutionEnd {
                    call_id,
                    tool_name,
                    result,
                    duration_ms,
                    is_error,
                } => {
                    let json = serde_json::json!({
                        "type": "tool_end",
                        "tool": tool_name,
                        "id": call_id,
                        "result": result,
                        "duration_ms": duration_ms,
                        "is_error": is_error,
                    });
                    println!("{}", json);
                }
                AgentEvent::AgentEnd { reason, .. } => {
                    let json = serde_json::json!({
                        "type": "agent_end",
                        "reason": reason,
                    });
                    println!("{}", json);
                    break;
                }
                _ => {}
            }
        }
    });

    let result = agent.prompt(prompt).await;
    let _ = printer.await;

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
