use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

use pi_agent_core::{Agent, AgentConfig};
use pi_agent_core::context::budget::TokenBudget;
use pi_agent_core::context::compaction::CompactionSettings;
use pi_coding_agent::cli::Args;
use pi_coding_agent::tools::operations::LocalFileOps;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    if args.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("pi=debug")
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    let cwd = std::env::current_dir()?.display().to_string();

    // Resolve provider and model
    let provider_name = args.provider.as_deref().unwrap_or("anthropic");
    let model_name = args.model.as_deref().unwrap_or("claude-sonnet-4-5");

    // Register default providers
    pi_ai::register_defaults();

    let provider = pi_ai::get_provider(provider_name)
        .ok_or_else(|| anyhow::anyhow!(
            "Provider '{}' not available. Set the API key env var (e.g. ANTHROPIC_API_KEY).",
            provider_name
        ))?;

    let model = pi_ai::find_model(model_name)
        .ok_or_else(|| anyhow::anyhow!("Model '{}' not found", model_name))?
        .clone();

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
        system_prompt: args.system_prompt.clone().or_else(|| {
            Some("You are a helpful AI coding assistant. You have access to tools for reading, writing, and editing files, running bash commands, and searching code.".to_string())
        }),
        max_turns: 50,
        token_budget: TokenBudget::default(),
        compaction: CompactionSettings::default(),
        thinking_level,
        cwd: cwd.clone(),
    };

    let agent = Agent::new(config);

    // Register built-in tools
    let ops: Arc<dyn pi_coding_agent::tools::FileOperations> = Arc::new(LocalFileOps);
    agent.register_tool(Arc::new(pi_coding_agent::tools::read::ReadTool::new(ops.clone()))).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::write::WriteTool::new(ops.clone()))).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::edit::EditTool::new(ops.clone()))).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::bash::BashTool::new())).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::grep::GrepTool::new())).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::find::FindTool::new())).await;
    agent.register_tool(Arc::new(pi_coding_agent::tools::ls::LsTool::new())).await;

    // Route to appropriate mode
    let prompt = args.messages.join(" ");

    if args.print && !prompt.is_empty() {
        pi_coding_agent::modes::print::run_print_mode(agent, &prompt).await?;
    } else if args.mode == "json" && !prompt.is_empty() {
        pi_coding_agent::modes::json::run_json_mode(agent, &prompt).await?;
    } else if args.mode == "rpc" {
        pi_coding_agent::modes::rpc::run_rpc_mode(agent).await?;
    } else {
        pi_coding_agent::modes::interactive::run_interactive_mode(agent).await?;
    }

    Ok(())
}
