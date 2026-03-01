use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use pi_agent_core::context::budget::TokenBudget;
use pi_agent_core::context::compaction::CompactionSettings;
use pi_agent_core::{Agent, AgentConfig};
use pi_ai::{Content, Message};
use pi_coding_agent::cli::Args;
use pi_coding_agent::session::SessionManager;
use pi_coding_agent::tools::operations::LocalFileOps;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let runtime_api_key = Arc::new(RwLock::new(args.api_key.clone()));

    // Initialize tracing
    if args.verbose {
        tracing_subscriber::fmt().with_env_filter("pi=debug").init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    let cwd = std::env::current_dir()?.display().to_string();

    // Load context files (.pi/, AGENTS.md, CLAUDE.md, SYSTEM.md)
    let loaded_context =
        pi_coding_agent::context::resource_loader::load_context(std::path::Path::new(&cwd))?;

    let default_prompt = "You are a helpful AI coding assistant. You have access to tools for reading, writing, and editing files, running bash commands, and searching code.";

    let system_prompt = pi_coding_agent::context::resource_loader::build_system_prompt(
        &loaded_context,
        args.system_prompt.as_deref(),
        default_prompt,
    );

    // Resolve provider and model(s)
    // Allow starting without a provider configured - will show error in TUI when user tries to send a message
    let provider_name = resolve_provider_name(args.provider.as_deref().unwrap_or("anthropic"));
    let model_ids: Vec<String> = if !args.models.is_empty() {
        args.models.clone()
    } else {
        vec![args
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-5".to_string())]
    };

    // Register default providers
    pi_ai::register_defaults();

    // Provider is optional at startup. If the env-backed provider isn't available,
    // bootstrap a local provider so runtime `/setkey` overrides can work without restart.
    let provider_from_registry = pi_ai::get_provider(provider_name);
    let mut provider = provider_from_registry.clone();
    if provider.is_none() {
        let bootstrap_key = args.api_key.as_deref().unwrap_or("pi-runtime-key");
        provider = build_fallback_provider(provider_name, bootstrap_key);
    }

    if provider.is_none() {
        eprintln!(
            "Warning: Provider '{}' not available. Set the API key env var to enable.",
            provider_name
        );
        eprintln!("You can still use the TUI, but you'll need to configure a provider before sending messages.");
    } else if provider_from_registry.is_none() {
        eprintln!(
            "Info: Provider '{}' initialized without env credentials. Use --api-key or /setkey <key>.",
            provider_name
        );
    }

    let mut resolved_models = Vec::new();
    for model_id in &model_ids {
        if let Some(model) = pi_ai::find_model(model_id) {
            resolved_models.push(model);
        } else if provider.is_none() {
            // Use a placeholder model if no provider and model not found
            // This allows TUI to start even without proper configuration
            continue;
        } else {
            return Err(anyhow::anyhow!("Model '{}' not found", model_id));
        }
    }

    // Use first resolved model, or a placeholder if none found
    let model = resolved_models.first().cloned().unwrap_or_else(|| {
        // Create a minimal placeholder model (when no provider configured)
        use pi_ai::models::registry::{InputType, ModelCost};
        pi_ai::Model {
            id: model_ids
                .first()
                .map(|s| s.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            name: "Unknown Model".to_string(),
            api: pi_ai::messages::types::Api::AnthropicMessages,
            provider: pi_ai::messages::types::Provider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            reasoning: false,
            input_types: vec![InputType::Text],
            cost: ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 100000,
            max_tokens: 4096,
            headers: None,
        }
    });

    // Parse thinking level
    let thinking_level = args.thinking.as_deref().map(|s| match s {
        "minimal" => pi_ai::ThinkingLevel::Minimal,
        "low" => pi_ai::ThinkingLevel::Low,
        "medium" => pi_ai::ThinkingLevel::Medium,
        "high" => pi_ai::ThinkingLevel::High,
        "xhigh" => pi_ai::ThinkingLevel::XHigh,
        _ => pi_ai::ThinkingLevel::Medium,
    });

    let api_key_resolver = {
        let runtime_api_key = Arc::clone(&runtime_api_key);
        Some(Box::new(move || {
            runtime_api_key
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }) as Box<dyn Fn() -> Option<String> + Send + Sync>)
    };

    let config = AgentConfig {
        provider, // Optional - can be None if no API key set
        model,
        system_prompt: Some(system_prompt),
        max_turns: 50,
        token_budget: TokenBudget::default(),
        compaction: CompactionSettings::default(),
        thinking_level,
        cwd: cwd.clone(),
        api_key_override: None,
        api_key_resolver,
        thinking_budgets: None,
        session_id: None,
        event_log_path: None,
        streaming_tool_execution: false,
        thinking_budget_selector: None,
    };

    let agent = Arc::new(Agent::new(config));
    if resolved_models.len() > 1 {
        agent.configure_model_cycle(resolved_models).await;
    }

    // Register built-in tools
    let ops: Arc<dyn pi_coding_agent::tools::FileOperations> = Arc::new(LocalFileOps);
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::read::ReadTool::new(
            ops.clone(),
        )))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::write::WriteTool::new(
            ops.clone(),
        )))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::edit::EditTool::new(
            ops.clone(),
        )))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::bash::BashTool::new()))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::grep::GrepTool::new()))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::find::FindTool::new()))
        .await;
    agent
        .register_tool(Arc::new(pi_coding_agent::tools::ls::LsTool::new()))
        .await;

    // Discover and register skills as callable tools.
    let skill_catalog =
        pi_coding_agent::skills::SkillCatalog::discover(std::path::Path::new(&cwd))?;
    let skill_tools = pi_coding_agent::skills::register_skill_tools(&agent, &skill_catalog).await;
    if skill_tools > 0 {
        tracing::info!("Registered {} skill tools", skill_tools);
    }

    // Discover and register extension tools.
    let extensions = pi_coding_agent::extensions::discover_extensions(std::path::Path::new(&cwd))?;
    let extension_tools =
        pi_coding_agent::extensions::register_extension_tools(&agent, &extensions, None).await;
    if extension_tools > 0 {
        tracing::info!(
            "Registered {} extension tools from {} extensions",
            extension_tools,
            extensions.len()
        );
    }

    let mut session_manager = init_session(&args, &cwd, &agent).await?;

    // Route to appropriate mode
    let raw_prompt = args.messages.join(" ");

    // Process @file references in CLI prompt (expand text files, extract images)
    let processed = pi_coding_agent::input::file_processor::process_input(
        &raw_prompt,
        std::path::Path::new(&cwd),
    )?;
    let prompt = processed.text;
    let mut user_blocks: Vec<Content> = Vec::new();
    if !prompt.is_empty() {
        user_blocks.push(Content::text(prompt.clone()));
    }
    user_blocks.extend(processed.images.iter().map(|img| img.to_content()));
    let initial_message = if user_blocks.is_empty() {
        Message::user("")
    } else {
        Message::user_with_images(user_blocks)
    };
    let has_initial_input = !prompt.is_empty() || !processed.images.is_empty();

    if args.print && has_initial_input {
        let baseline = agent.messages().await.len();
        let mode_result =
            pi_coding_agent::modes::print::run_print_mode_message(&agent, initial_message.clone())
                .await;
        persist_new_messages(&mut session_manager, &agent, baseline).await?;
        mode_result?;
    } else if args.mode == "json" && has_initial_input {
        let baseline = agent.messages().await.len();
        let mode_result =
            pi_coding_agent::modes::json::run_json_mode_message(&agent, initial_message).await;
        persist_new_messages(&mut session_manager, &agent, baseline).await?;
        mode_result?;
    } else if args.mode == "rpc" {
        pi_coding_agent::modes::rpc::run_rpc_mode(Arc::clone(&agent)).await?;
    } else {
        pi_coding_agent::modes::interactive::run_interactive_mode(
            Arc::clone(&agent),
            Arc::clone(&runtime_api_key),
        )
        .await?;
    }

    Ok(())
}

fn build_fallback_provider(api: &str, bootstrap_key: &str) -> Option<Arc<dyn pi_ai::LLMProvider>> {
    match api {
        "anthropic-messages" => Some(Arc::new(pi_ai::AnthropicProvider::new(bootstrap_key, None))),
        "openai-completions" => Some(Arc::new(pi_ai::OpenAIProvider::new(
            bootstrap_key,
            None,
            pi_ai::OpenAICompat::default(),
        ))),
        "google-generative-ai" => Some(Arc::new(pi_ai::GoogleProvider::new(bootstrap_key, None))),
        _ => None,
    }
}

fn resolve_sessions_dir(args: &Args) -> PathBuf {
    if let Some(dir) = args.session_dir.as_ref() {
        return PathBuf::from(dir);
    }

    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pi")
        .join("agent")
        .join("sessions")
}

async fn init_session(args: &Args, cwd: &str, agent: &Agent) -> Result<Option<SessionManager>> {
    if args.no_session {
        return Ok(None);
    }

    let mut manager = SessionManager::new(resolve_sessions_dir(args));
    let mut restored_messages = Vec::new();

    if let Some(session_path) = args.session.as_ref() {
        let path = PathBuf::from(session_path);
        restored_messages = manager.open_or_create_session(cwd, &path).await?;
    } else if args.resume {
        let sessions = manager.list_sessions().await?;
        if let Some((_, path)) = sessions.last() {
            restored_messages = manager.open_or_create_session(cwd, path).await?;
        } else {
            manager.create_session(cwd).await?;
        }
    } else {
        manager.create_session(cwd).await?;
    }

    if !restored_messages.is_empty() {
        agent.preload_messages(restored_messages).await;
    }

    Ok(Some(manager))
}

/// Map short provider names to full API identifiers.
fn resolve_provider_name(name: &str) -> &str {
    match name {
        "anthropic" => "anthropic-messages",
        "openai" => "openai-completions",
        "google" => "google-generative-ai",
        "azure" => "azure-open-ai-responses",
        "bedrock" => "bedrock-converse-stream",
        "mistral" => "mistral-native",
        // Already full names or unknown
        _ => name,
    }
}

async fn persist_new_messages(
    session_manager: &mut Option<SessionManager>,
    agent: &Agent,
    baseline_len: usize,
) -> Result<()> {
    let Some(manager) = session_manager.as_mut() else {
        return Ok(());
    };

    let messages = agent.messages().await;
    for message in messages.into_iter().skip(baseline_len) {
        manager.append_message(message).await?;
    }

    Ok(())
}
