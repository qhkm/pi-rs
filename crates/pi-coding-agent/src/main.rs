use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

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
    let provider_name = args.provider.as_deref().unwrap_or("anthropic");
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

    let provider = pi_ai::get_provider(provider_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider '{}' not available. Set the API key env var (e.g. ANTHROPIC_API_KEY).",
            provider_name
        )
    })?;

    let mut resolved_models = Vec::new();
    for model_id in &model_ids {
        let model = pi_ai::find_model(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_id))?;
        resolved_models.push(model);
    }
    let model = resolved_models[0].clone();

    // Parse thinking level
    let thinking_level = args.thinking.as_deref().map(|s| match s {
        "minimal" => pi_ai::ThinkingLevel::Minimal,
        "low" => pi_ai::ThinkingLevel::Low,
        "medium" => pi_ai::ThinkingLevel::Medium,
        "high" => pi_ai::ThinkingLevel::High,
        "xhigh" => pi_ai::ThinkingLevel::XHigh,
        _ => pi_ai::ThinkingLevel::Medium,
    });

    let config = AgentConfig {
        provider,
        model,
        system_prompt: Some(system_prompt),
        max_turns: 50,
        token_budget: TokenBudget::default(),
        compaction: CompactionSettings::default(),
        thinking_level,
        cwd: cwd.clone(),
        api_key_override: None,
        api_key_resolver: None,
        thinking_budgets: None,
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
        pi_coding_agent::extensions::register_extension_tools(&agent, &extensions).await;
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
        pi_coding_agent::modes::interactive::run_interactive_mode(Arc::clone(&agent)).await?;
    }

    Ok(())
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
