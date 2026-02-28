use anyhow::Result;
use pi_agent_core::{Agent, AgentEvent};
use pi_ai::{Content, Message};
use std::io::{self, BufRead, Write};
use std::path::Path;
use tokio::task::LocalSet;

/// Run in interactive TUI mode
pub async fn run_interactive_mode(agent: &Agent) -> Result<()> {
    println!("pi interactive mode (type 'exit' or '/quit' to quit)");
    println!("---");

    // Use a LocalSet so we can spawn non-Send futures (the agent lives as &Agent)
    let local = LocalSet::new();

    local.run_until(repl_loop(agent)).await
}

/// The main REPL loop
async fn repl_loop(agent: &Agent) -> Result<()> {
    let mut catalog = crate::skills::SkillCatalog::discover(Path::new(&agent.config.cwd))?;
    let mut active_skills = crate::skills::ActiveSkills::default();
    if !catalog.is_empty() {
        println!(
            "[skills loaded: {}] use /skills, /skill:list, /skill:<name>, /skill:clear, /skill:install <path>",
            catalog.len()
        );
    }

    loop {
        // Step 1: prompt the user for input
        print!("> ");
        io::stdout().flush()?;

        // Read a line via spawn_blocking so we don't block the async executor
        let line = tokio::task::spawn_blocking(|| {
            let stdin = io::stdin();
            let mut buf = String::new();
            match stdin.lock().read_line(&mut buf) {
                Ok(0) => None, // EOF
                Ok(_) => Some(buf),
                Err(_) => None,
            }
        })
        .await?;

        let line = match line {
            None => break, // EOF
            Some(l) => l,
        };

        let input = line.trim().to_string();

        if input == "exit" || input == "/quit" {
            break;
        }
        if input.is_empty() {
            continue;
        }
        if input == "/skills" || input == "/skill:list" {
            print_skill_list(&catalog, &active_skills);
            continue;
        }
        if input == "/skill:clear" {
            active_skills.clear();
            println!("[skills] cleared");
            continue;
        }
        if let Some(path) = input.strip_prefix("/skill:install ") {
            let source = Path::new(path.trim());
            match crate::skills::install_skill_into_project(Path::new(&agent.config.cwd), source) {
                Ok(installed) => {
                    crate::skills::register_skill_tool(agent, installed.clone()).await;
                    catalog.upsert(installed.clone());
                    println!(
                        "[skills] installed '{}' at {}",
                        installed.name,
                        installed.path.display()
                    );
                }
                Err(err) => {
                    println!("[skills] install failed: {}", err);
                }
            }
            continue;
        }
        if let Some(name) = input.strip_prefix("/skill:") {
            if name.trim().is_empty() {
                println!("[skills] usage: /skill:<name> (or /skill:list)");
                continue;
            }
            if let Some(skill) = catalog.get(name.trim()) {
                active_skills.set(&skill.name);
                println!("[skills] activated '{}'", skill.name);
            } else {
                println!("[skills] '{}' not found", name.trim());
            }
            continue;
        }

        // Steps 2-6: subscribe, call prompt concurrently, handle events, print result
        let processed =
            crate::input::file_processor::process_input(&input, Path::new(&agent.config.cwd))?;
        let prompt_text =
            crate::skills::decorate_user_text(&processed.text, &catalog, &active_skills);
        let mut blocks = Vec::new();
        if !prompt_text.is_empty() {
            blocks.push(Content::text(prompt_text));
        }
        blocks.extend(processed.images.iter().map(|img| img.to_content()));
        if blocks.is_empty() {
            continue;
        }
        run_prompt_with_events(agent, Message::user_with_images(blocks)).await?;
    }

    Ok(())
}

fn print_skill_list(catalog: &crate::skills::SkillCatalog, active: &crate::skills::ActiveSkills) {
    if catalog.is_empty() {
        println!("[skills] none found under ~/.pi/skills or .pi/skills");
        return;
    }

    let active_names = active.list();
    for name in catalog.names() {
        let marker = if active_names.contains(&name) {
            "*"
        } else {
            " "
        };
        if let Some(skill) = catalog.get(&name) {
            println!(
                "[{}] {} - {} ({})",
                marker,
                skill.name,
                skill.description,
                skill.path.display()
            );
        }
    }
}

/// Subscribe to agent events, fire off agent.prompt() as a local task, and drive the
/// event loop on the current task until the agent signals it is done.
async fn run_prompt_with_events(agent: &Agent, input: Message) -> Result<()> {
    // Subscribe *before* spawning the prompt so we don't miss any early events
    let mut rx = agent.subscribe();

    // Spawn agent.prompt() as a local task (no Send requirement)
    // SAFETY: `agent` lives for the duration of this function.  The local task is
    // awaited (via the join handle) before we return, so the borrow is valid.
    let input_owned = input;
    let agent_ptr = agent as *const Agent;
    // Wrap the raw pointer in a newtype that asserts Send.  This is safe because:
    //   1. Agent is internally Arc-based with tokio primitives (all Send).
    //   2. The spawned local task completes before we leave this function.
    struct SendAgent(*const Agent);
    // SAFETY: Agent contains only Send types; the raw pointer stays valid.
    unsafe impl Send for SendAgent {}

    let wrapped = SendAgent(agent_ptr);
    let prompt_handle = tokio::task::spawn_local(async move {
        // SAFETY: wrapped.0 was created from a valid &Agent that outlives this task.
        let a = unsafe { &*wrapped.0 };
        a.prompt_message(input_owned).await
    });

    // Drive the event stream until the agent signals completion
    let mut agent_done = false;
    while !agent_done {
        match rx.recv().await {
            Ok(event) => {
                handle_event(agent, event, &mut agent_done).await?;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                agent_done = true;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                eprintln!(
                    "\n[warning: missed {} events due to slow consumer]",
                    skipped
                );
            }
        }
    }

    // Collect the prompt result
    println!(); // ensure we're on a new line after streaming output
    match prompt_handle.await {
        Ok(Ok(msg)) => {
            let usage = &msg.usage;
            println!("[tokens: {} in / {} out]", usage.input, usage.output);
        }
        Ok(Err(e)) => {
            eprintln!("[error: {}]", e);
        }
        Err(e) => {
            eprintln!("[task error: {}]", e);
        }
    }

    Ok(())
}

/// Handle a single AgentEvent, printing appropriate output and handling approvals.
/// Sets `done` to true when the agent signals it has finished.
async fn handle_event(agent: &Agent, event: AgentEvent, done: &mut bool) -> Result<()> {
    match event {
        // Stream text delta — print immediately with no newline
        AgentEvent::MessageUpdate { event, .. } => {
            if let pi_ai::StreamEvent::TextDelta { delta, .. } = event {
                print!("{}", delta);
                io::stdout().flush()?;
            }
        }

        // Tool starting — show the tool name and arguments
        AgentEvent::ToolExecutionStart {
            tool_name,
            arguments,
            ..
        } => {
            println!();
            println!("[tool: {}]  args: {}", tool_name, arguments);
        }

        // Tool finished — show a brief result summary
        AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            duration_ms,
            is_error,
            ..
        } => {
            let status = if is_error { "error" } else { "ok" };
            // Truncate long results so the terminal stays readable
            let summary: String = result.chars().take(200).collect();
            let ellipsis = if result.len() > 200 { "..." } else { "" };
            println!(
                "[tool: {} -> {} in {}ms] {}{}",
                tool_name, status, duration_ms, summary, ellipsis
            );
        }

        // Tool requires approval — ask the user interactively
        AgentEvent::ToolApprovalRequired {
            call_id,
            tool_name,
            arguments,
        } => {
            println!();
            println!(
                "[approval required] '{}' with args: {}",
                tool_name, arguments
            );
            print!("Allow? [y/N]: ");
            io::stdout().flush()?;

            // Read y/n via spawn_blocking so we don't block the async executor
            let answer = tokio::task::spawn_blocking(|| {
                let stdin = io::stdin();
                let mut buf = String::new();
                stdin.lock().read_line(&mut buf).ok();
                buf.trim().to_lowercase()
            })
            .await?;

            let approved = answer == "y" || answer == "yes";
            agent.approve_tool(&call_id, approved).await;

            if approved {
                println!("[approved]");
            } else {
                println!("[denied]");
            }
        }

        // Message ended — print token usage
        AgentEvent::MessageEnd { usage, .. } => {
            if let Some(u) = usage {
                println!("\n[usage: {} in / {} out]", u.input, u.output);
            }
        }

        // Agent finished — signal the event loop to exit
        AgentEvent::AgentEnd { .. } => {
            *done = true;
        }

        // All other events are intentionally ignored
        _ => {}
    }

    Ok(())
}
