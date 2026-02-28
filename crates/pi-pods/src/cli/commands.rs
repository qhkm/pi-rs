use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum PodCommand {
    /// Set up a new pod
    Setup {
        name: String,
        #[arg(long)]
        ssh: String,
        #[arg(long)]
        mount: Option<String>,
        #[arg(long, default_value = "release")]
        vllm: String,
    },
    /// List all pods
    List,
    /// Set active pod
    Active { name: String },
    /// Remove a pod
    Remove { name: String },
    /// Start a model
    Start {
        model: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        memory: Option<String>,
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        gpus: Option<u32>,
    },
    /// Stop a model
    Stop {
        #[arg(default_value = "")]
        name: String,
    },
    /// Show logs for a model
    Logs { name: String },
    /// Open SSH shell
    Shell {
        #[arg(default_value = "")]
        name: String,
    },
}
