use std::sync::{Arc, RwLock};

use anyhow::Result;
use pi_agent_core::{Agent, AgentEvent};
use pi_ai::{Content, Message};
use std::io::{self, BufRead, Write};
use std::path::Path;
use tokio::task::LocalSet;

// Rustyline for tab completion
use rustyline::completion::Completer;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

// ─── Command Completer ────────────────────────────────────────────────────────

struct CommandCompleter;

impl Completer for CommandCompleter {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let commands = vec![
            "/skills",
            "/skill:list",
            "/skill:clear",
            "/skill:install ",
            "/setkey ",
            "/apikey ",
            "/providers",
            "/provider ",
            "/quit",
            "exit",
        ];

        // If line starts with '/', suggest matching commands
        if line.starts_with('/') || line.is_empty() {
            let matches: Vec<String> = commands
                .into_iter()
                .filter(|cmd| cmd.starts_with(line))
                .map(|s| s.to_string())
                .collect();
            if !matches.is_empty() {
                return Ok((0, matches));
            }
        }

        Ok((0, vec![]))
    }
}

impl Highlighter for CommandCompleter {}
impl Hinter for CommandCompleter {
    type Hint = String;
}
impl Validator for CommandCompleter {}
impl Helper for CommandCompleter {}

/// Run in interactive TUI mode
pub async fn run_interactive_mode(
    agent: Arc<Agent>,
    runtime_api_key: Arc<RwLock<Option<String>>>,
) -> Result<()> {
    println!("pi interactive mode (type 'exit' or '/quit' to quit)");
    println!("[auth] use /setkey <api-key> to set a runtime API key");
    println!("---");

    // Use a LocalSet so we can spawn non-Send futures
    let local = LocalSet::new();

    local.run_until(repl_loop(agent, runtime_api_key)).await
}

/// The main REPL loop
async fn repl_loop(agent: Arc<Agent>, runtime_api_key: Arc<RwLock<Option<String>>>) -> Result<()> {
    let mut catalog = crate::skills::SkillCatalog::discover(Path::new(&agent.config.cwd))?;
    let mut active_skills = crate::skills::ActiveSkills::default();
    if !catalog.is_empty() {
        println!(
            "[skills loaded: {}] use /skills, /skill:list, /skill:<name>, /skill:clear, /skill:install <path>",
            catalog.len()
        );
    }

    // Channel for sending readline results from blocking thread
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Option<String>>(1);

    // Spawn a blocking thread that owns the rustyline editor
    let readline_handle = tokio::task::spawn_blocking(move || {
        let mut editor: rustyline::Editor<CommandCompleter, rustyline::history::DefaultHistory> =
            match rustyline::Editor::new() {
                Ok(mut ed) => {
                    ed.set_helper(Some(CommandCompleter));
                    ed
                }
                Err(e) => {
                    eprintln!("[failed to initialize readline: {}]", e);
                    return;
                }
            };

        loop {
            match editor.readline("> ") {
                Ok(line) => {
                    if tx.blocking_send(Some(line)).is_err() {
                        break; // Receiver dropped
                    }
                }
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(rustyline::error::ReadlineError::Eof) => {
                    let _ = tx.blocking_send(None); // Signal EOF
                    break;
                }
                Err(e) => {
                    eprintln!("[readline error: {}]", e);
                    continue;
                }
            }
        }
    });

    loop {
        // Wait for input from the readline thread
        let line = match rx.recv().await {
            Some(Some(line)) => line,
            Some(None) => break, // EOF
            None => break,       // Channel closed
        };

        let input = line.trim().to_string();

        if input == "exit" || input == "/quit" {
            break;
        }
        if input == "/setkey" || input == "/apikey" {
            println!("[auth] usage: /setkey <api-key> (or /setkey clear)");
            continue;
        }
        if let Some(raw) = input
            .strip_prefix("/setkey ")
            .or_else(|| input.strip_prefix("/apikey "))
        {
            let value = raw.trim();
            if value.is_empty() {
                println!("[auth] usage: /setkey <api-key> (or /setkey clear)");
                continue;
            }

            if value.eq_ignore_ascii_case("clear") {
                *runtime_api_key.write().unwrap_or_else(|e| e.into_inner()) = None;
                println!("[auth] runtime API key cleared (falling back to provider defaults)");
                continue;
            }

            *runtime_api_key.write().unwrap_or_else(|e| e.into_inner()) = Some(value.to_string());
            println!("[auth] runtime API key set: {}", mask_secret(value));
            
            // Try to detect provider from key format
            let detected = detect_provider_from_key(value);
            println!("[auth] detected provider: {}. Restart with --provider {} to use it.", 
                detected, detected);
            continue;
        }
        if input == "/providers" {
            print_available_providers();
            continue;
        }
        if input == "/provider" {
            println!("[provider] usage: /provider <name> (or /providers to list)");
            println!("[provider] current: {} (restart with --provider <name> to change)", 
                agent.config.model.provider);
            continue;
        }
        if let Some(name) = input.strip_prefix("/provider ") {
            let provider_name = name.trim();
            if provider_name.is_empty() {
                println!("[provider] usage: /provider <name> (or /providers to list)");
            } else {
                println!("[provider] to switch to '{}', restart with: pi --provider {}", 
                    provider_name, provider_name);
            }
            continue;
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
                    crate::skills::register_skill_tool(&agent, installed.clone()).await;
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
        run_prompt_with_events(&agent, Message::user_with_images(blocks)).await?;
    }

    // Clean up the readline thread
    drop(rx);
    let _ = readline_handle.await;

    Ok(())
}

fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(4));
    }

    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(chars.len().saturating_sub(4)).collect();
    format!("{prefix}***{suffix}")
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
async fn run_prompt_with_events(agent: &Arc<Agent>, input: Message) -> Result<()> {
    // Subscribe *before* spawning the prompt so we don't miss any early events
    let mut rx = agent.subscribe();

    // Clone the Arc so the spawned task owns its own handle to the agent.
    let agent_clone = Arc::clone(agent);
    let prompt_handle =
        tokio::task::spawn_local(async move { agent_clone.prompt_message(input).await });

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

/// Detect provider from API key format
fn detect_provider_from_key(key: &str) -> &str {
    if key.starts_with("sk-ant-") {
        "anthropic"
    } else if key.starts_with("sk-or-") {
        "openrouter"
    } else if key.starts_with("sk-proj-") || key.starts_with("sk-") && key.len() > 20 {
        "openai"
    } else if key.starts_with("gsk_") {
        "groq"
    } else if key.starts_with("AIza") {
        "google"
    } else {
        "unknown (try: anthropic, openai, google, groq, openrouter)"
    }
}

/// Print available providers
fn print_available_providers() {
    println!("[providers] available providers:");
    println!("  anthropic   - Claude (ANTHROPIC_API_KEY)");
    println!("  openai      - GPT-4, GPT-3.5 (OPENAI_API_KEY)");
    println!("  google      - Gemini (GOOGLE_API_KEY)");
    println!("  groq        - Llama, Mixtral (GROQ_API_KEY)");
    println!("  openrouter  - Multi-provider (OPENROUTER_API_KEY)");
    println!("  azure       - Azure OpenAI (AZURE_OPENAI_API_KEY)");
    println!("  bedrock     - AWS Bedrock (AWS credentials)");
    println!("");
    println!("[providers] usage: pi --provider <name>");
    println!("[providers] or: /provider <name> (shows restart command)");
}
