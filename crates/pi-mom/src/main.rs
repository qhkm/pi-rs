use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pi-mom", about = "AI Slack bot")]
struct Args {
    /// Slack app token
    #[arg(long, env = "SLACK_APP_TOKEN")]
    app_token: Option<String>,

    /// Slack bot token
    #[arg(long, env = "SLACK_BOT_TOKEN")]
    bot_token: Option<String>,

    /// Workspace directory
    #[arg(long, default_value = ".pi-mom")]
    workspace: String,

    /// Sandbox mode
    #[arg(long, default_value = "host")]
    sandbox: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let app_token = args
        .app_token
        .ok_or_else(|| anyhow::anyhow!("SLACK_APP_TOKEN required"))?;
    let bot_token = args
        .bot_token
        .ok_or_else(|| anyhow::anyhow!("SLACK_BOT_TOKEN required"))?;

    tracing::info!("Starting pi-mom with workspace: {}", args.workspace);

    let client = pi_mom::slack::socket_mode::SocketModeClient::new(app_token, bot_token);
    client.connect().await?;

    Ok(())
}
