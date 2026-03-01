use anyhow::{Context, Result};
use clap::Parser;

use pi_pods::cli::PodCommand;
use pi_pods::config::pod::RunningModel;
use pi_pods::config::{Config, Gpu, Pod};
use pi_pods::providers::vllm::VllmManager;
use pi_pods::ssh::SshClient;

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
        Some(PodCommand::Setup {
            name,
            ssh,
            mount,
            vllm,
        }) => {
            cmd_setup(name, ssh, mount, vllm).await?;
        }
        Some(PodCommand::List) | None => {
            cmd_list().await?;
        }
        Some(PodCommand::Active { name }) => {
            cmd_active(name).await?;
        }
        Some(PodCommand::Remove { name }) => {
            cmd_remove(name).await?;
        }
        Some(PodCommand::Start {
            model,
            name,
            memory,
            context,
            gpus,
        }) => {
            cmd_start(model, name, memory, context, gpus).await?;
        }
        Some(PodCommand::Stop { name }) => {
            cmd_stop(name).await?;
        }
        Some(PodCommand::Logs { name }) => {
            cmd_logs(&name).await?;
        }
        Some(PodCommand::Shell { name }) => {
            cmd_shell(&name).await?;
        }
    }

    Ok(())
}

async fn cmd_setup(name: String, ssh: String, mount: Option<String>, vllm: String) -> Result<()> {
    println!("Setting up pod '{}' with SSH: {}", name, ssh);

    // Test SSH connection and detect GPUs
    let ssh_client = SshClient::new(&ssh)?;
    println!("Testing SSH connection...");

    let gpus = ssh_client.detect_gpus().await?;
    println!("Detected {} GPU(s):", gpus.len());
    for gpu in &gpus {
        println!("  GPU {}: {} ({})", gpu.id, gpu.name, gpu.memory);
    }

    // Create pod config
    let pod = Pod {
        ssh: ssh.clone(),
        gpus,
        models: Default::default(),
        models_path: mount,
        vllm_version: vllm,
    };

    // Save config
    let mut config = Config::load()?;
    config.pods.insert(name.clone(), pod);

    // Set as active if first pod
    if config.active.is_none() {
        config.active = Some(name.clone());
    }

    config.save()?;

    println!("✓ Pod '{}' configured successfully", name);
    if config.active.as_ref() == Some(&name) {
        println!("  (set as active pod)");
    }

    Ok(())
}

async fn cmd_list() -> Result<()> {
    let config = Config::load()?;

    if config.pods.is_empty() {
        println!("No pods configured. Use 'pi-pods setup' to add one.");
        return Ok(());
    }

    println!(
        "{:<15} {:<25} {:<10} {:<20}",
        "NAME", "SSH", "GPUS", "RUNNING MODELS"
    );
    println!("{}", "-".repeat(80));

    for (name, pod) in &config.pods {
        let active = config.active.as_deref() == Some(name.as_str());
        let marker = if active { "*" } else { " " };

        let gpu_summary = if pod.gpus.len() == 1 {
            format!("1 GPU")
        } else {
            format!("{} GPUs", pod.gpus.len())
        };

        let models_summary = if pod.models.is_empty() {
            "-".to_string()
        } else {
            pod.models.keys().cloned().collect::<Vec<_>>().join(", ")
        };

        println!(
            "{:<15} {:<25} {:<10} {:<20}",
            format!("[{}] {}", marker, name),
            truncate(&pod.ssh, 25),
            gpu_summary,
            truncate(&models_summary, 20)
        );
    }

    println!("\n* = active pod");

    Ok(())
}

async fn cmd_active(name: String) -> Result<()> {
    let mut config = Config::load()?;

    if !config.pods.contains_key(&name) {
        anyhow::bail!(
            "Pod '{}' not found. Run 'pi-pods list' to see available pods.",
            name
        );
    }

    config.active = Some(name.clone());
    config.save()?;

    println!("Active pod set to: {}", name);

    Ok(())
}

async fn cmd_remove(name: String) -> Result<()> {
    let mut config = Config::load()?;

    if !config.pods.contains_key(&name) {
        anyhow::bail!("Pod '{}' not found.", name);
    }

    // Check if pod has running models
    if let Some(pod) = config.pods.get(&name) {
        if !pod.models.is_empty() {
            println!("Warning: Pod '{}' has running models:", name);
            for model_name in pod.models.keys() {
                println!("  - {}", model_name);
            }
            println!("Stop them first with 'pi-pods stop <name>' or use --force");
        }
    }

    config.pods.remove(&name);

    // Update active pod if needed
    if config.active.as_ref() == Some(&name) {
        config.active = config.pods.keys().next().cloned();
        if let Some(new_active) = &config.active {
            println!("Active pod changed to: {}", new_active);
        }
    }

    config.save()?;
    println!("Pod '{}' removed.", name);

    Ok(())
}

async fn cmd_start(
    model: String,
    name: String,
    memory: Option<String>,
    context: Option<String>,
    gpus: Option<u32>,
) -> Result<()> {
    let mut config = Config::load()?;
    let pod_name = config
        .active_pod()
        .map(|(n, _)| n.to_string())
        .ok_or_else(|| anyhow::anyhow!("No active pod. Use 'pi-pods active <name>' to set one."))?;

    // Get pod info in a scoped block
    let (pod_ssh, pod_vllm_version) = {
        let pod = config
            .pods
            .get(&pod_name)
            .ok_or_else(|| anyhow::anyhow!("Active pod '{}' not found", pod_name))?;
        (pod.ssh.clone(), pod.vllm_version.clone())
    };

    // Check if model already running
    {
        let pod = config.pods.get(&pod_name).unwrap();
        if pod.models.contains_key(&name) {
            anyhow::bail!("Model '{}' is already running on this pod", name);
        }
    }

    let num_gpus = gpus.unwrap_or(1);

    println!("Starting model '{}' on pod '{}'...", name, pod_name);
    println!("  Model: {}", model);
    println!("  GPUs: {}", num_gpus);
    if let Some(mem) = &memory {
        println!("  Memory: {}", mem);
    }
    if let Some(ctx) = &context {
        println!("  Context: {}", ctx);
    }

    // Start vLLM via SSH
    let vllm = VllmManager::new(pod_ssh.clone(), pod_vllm_version);
    let port = vllm.find_free_port(&pod_ssh).await?;

    let gpu_ids: Vec<u32> = (0..num_gpus).collect();
    let pid = vllm
        .start_model(
            &model,
            port,
            &gpu_ids,
            memory.as_deref(),
            context.as_deref(),
        )
        .await?;

    // Update config with running model
    let running = RunningModel {
        model: model.clone(),
        port,
        gpus: gpu_ids,
        pid,
    };

    {
        let pod = config.pods.get_mut(&pod_name).unwrap();
        pod.models.insert(name.clone(), running);
    }

    config.save()?;

    println!("✓ Model '{}' started successfully", name);
    println!("  Port: {}", port);
    println!("  PID: {}", pid);
    println!("  API URL: http://{}:{}/v1", extract_host(&pod_ssh), port);

    Ok(())
}

async fn cmd_stop(name: String) -> Result<()> {
    let mut config = Config::load()?;
    let pod_name = if name.is_empty() {
        config
            .active_pod()
            .map(|(n, _)| n.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!("No active pod. Use 'pi-pods stop <name>' to specify.")
            })?
    } else {
        config
            .active
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active pod."))?
            .clone()
    };

    let pod = config
        .pods
        .get_mut(&pod_name)
        .ok_or_else(|| anyhow::anyhow!("Pod '{}' not found", pod_name))?;

    // Find model to stop
    let model_name = if name.is_empty() {
        // Stop all models if no name specified
        let models: Vec<String> = pod.models.keys().cloned().collect();
        if models.is_empty() {
            println!("No running models on pod '{}'", pod_name);
            return Ok(());
        }

        for m in &models {
            if let Some(running) = pod.models.get(m) {
                let vllm = VllmManager::new(pod.ssh.clone(), pod.vllm_version.clone());
                if let Err(e) = vllm.stop_model(running.pid).await {
                    eprintln!("Warning: Failed to stop '{}': {}", m, e);
                } else {
                    println!("Stopped model '{}'", m);
                }
            }
        }
        pod.models.clear();
        config.save()?;
        return Ok(());
    } else {
        name.clone()
    };

    let running = pod.models.get(&model_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Model '{}' is not running on pod '{}'",
            model_name,
            pod_name
        )
    })?;

    println!("Stopping model '{}' on pod '{}'...", model_name, pod_name);

    let vllm = VllmManager::new(pod.ssh.clone(), pod.vllm_version.clone());
    vllm.stop_model(running.pid).await?;

    pod.models.remove(&model_name);
    config.save()?;

    println!("✓ Model '{}' stopped", model_name);

    Ok(())
}

async fn cmd_logs(name: &str) -> Result<()> {
    let config = Config::load()?;
    let (pod_name, pod) = config
        .active_pod()
        .ok_or_else(|| anyhow::anyhow!("No active pod"))?;

    let running = pod
        .models
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Model '{}' is not running on pod '{}'", name, pod_name))?;

    let vllm = VllmManager::new(pod.ssh.clone(), pod.vllm_version.clone());
    let logs = vllm.get_logs(running.pid).await?;

    print!("{}", logs);

    Ok(())
}

async fn cmd_shell(name: &str) -> Result<()> {
    let config = Config::load()?;

    let ssh = if name.is_empty() {
        config
            .active_pod()
            .map(|(_, p)| p.ssh.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("No active pod. Use 'pi-pods shell <pod-name>' to specify.")
            })?
    } else {
        config
            .pods
            .get(name)
            .map(|p| p.ssh.clone())
            .ok_or_else(|| anyhow::anyhow!("Pod '{}' not found", name))?
    };

    println!("Opening SSH shell to {}...", ssh);

    let client = SshClient::new(&ssh)?;
    client.interactive_shell().await?;

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}

fn extract_host(ssh_url: &str) -> String {
    // Extract host from ssh://user@host:port or user@host format
    ssh_url
        .replace("ssh://", "")
        .split('@')
        .nth(1)
        .unwrap_or(ssh_url)
        .split(':')
        .next()
        .unwrap_or(ssh_url)
        .to_string()
}
