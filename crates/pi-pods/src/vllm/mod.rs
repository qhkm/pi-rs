use anyhow::Result;

/// Build vLLM start command
pub fn build_vllm_command(
    model: &str,
    port: u16,
    gpus: &[u32],
    memory_percent: u32,
    context: u64,
    extra_args: &[String],
) -> String {
    let gpu_ids = gpus
        .iter()
        .map(|g| g.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let tp = gpus.len();

    let mut cmd = format!(
        "CUDA_VISIBLE_DEVICES={} python -m vllm.entrypoints.openai.api_server \
         --model {} --port {} --tensor-parallel-size {} \
         --gpu-memory-utilization {} --max-model-len {}",
        gpu_ids,
        model,
        port,
        tp,
        memory_percent as f64 / 100.0,
        context,
    );

    for arg in extra_args {
        cmd.push(' ');
        cmd.push_str(arg);
    }

    cmd
}
