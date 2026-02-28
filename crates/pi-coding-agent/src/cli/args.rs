use clap::Parser;

/// pi - AI coding agent
#[derive(Parser, Debug)]
#[command(name = "pi", version, about = "AI coding agent")]
pub struct Args {
    /// LLM provider (e.g. anthropic, openai, google)
    #[arg(long)]
    pub provider: Option<String>,

    /// Model to use (e.g. claude-sonnet-4-5)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Comma-separated model cycle list (e.g. claude-sonnet-4-5,gpt-4.1)
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,

    /// API key (overrides env var)
    #[arg(long)]
    pub api_key: Option<String>,

    /// System prompt override
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Append to default system prompt
    #[arg(long)]
    pub append_system_prompt: Option<String>,

    /// Thinking level (minimal, low, medium, high, xhigh)
    #[arg(long)]
    pub thinking: Option<String>,

    /// Continue from last session
    #[arg(short = 'c', long)]
    pub resume: bool,

    /// Output mode: text (default), json, rpc
    #[arg(long, default_value = "text")]
    pub mode: String,

    /// Don't save session
    #[arg(long)]
    pub no_session: bool,

    /// Session file path
    #[arg(long)]
    pub session: Option<String>,

    /// Session directory
    #[arg(long)]
    pub session_dir: Option<String>,

    /// Print mode: non-interactive, single response
    #[arg(short, long)]
    pub print: bool,

    /// Disable all tools
    #[arg(long)]
    pub no_tools: bool,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Initial prompt messages (positional args)
    #[arg(trailing_var_arg = true)]
    pub messages: Vec<String>,
}
