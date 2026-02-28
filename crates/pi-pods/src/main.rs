use anyhow::Result;
use clap::Parser;

use pi_pods::cli::PodCommand;
use pi_pods::config::Config;

#[derive(Parser, Debug)]
#[command(name = "pi-pods", about = "GPU pod manager for vLLM")]
struct Args {
    #[command(subcommand)]
    command: Option<PodCommand>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    match args.command {
        Some(PodCommand::List) | None => {
            let config = Config::load()?;
            if config.pods.is_empty() {
                println!("No pods configured. Use 'pi-pods setup' to add one.");
            } else {
                for (name, pod) in &config.pods {
                    let active = config.active.as_deref() == Some(name.as_str());
                    let marker = if active { " *" } else { "" };
                    println!("{}{} - {} ({} GPUs)", name, marker, pod.ssh, pod.gpus.len());
                }
            }
        }
        Some(PodCommand::Active { name }) => {
            let mut config = Config::load()?;
            if !config.pods.contains_key(&name) {
                anyhow::bail!("Pod '{}' not found", name);
            }
            config.active = Some(name.clone());
            config.save()?;
            println!("Active pod set to: {}", name);
        }
        Some(cmd) => {
            println!("Command not yet implemented: {:?}", cmd);
        }
    }

    Ok(())
}
