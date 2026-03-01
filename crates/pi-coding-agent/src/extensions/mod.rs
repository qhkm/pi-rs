pub mod hooks;
pub mod types;
pub mod wasm;

pub use hooks::HookRegistry;
pub use types::{Extension, ExtensionManifest};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use pi_agent_core::{AgentTool, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tracing::warn;

use self::types::{ExecutorType, ExtensionCommand, ExtensionToolDef};
use self::wasm::WasmModuleCache;

const MANIFEST_FILE_NAME: &str = "extension.json";

/// Global WASM module cache (lazy-initialized)
fn get_wasm_cache() -> &'static WasmModuleCache {
    use std::sync::OnceLock;
    static CACHE: OnceLock<WasmModuleCache> = OnceLock::new();
    CACHE.get_or_init(WasmModuleCache::new)
}

pub fn discover_extensions(cwd: &Path) -> Result<Vec<Extension>> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".pi")
                .join("agent")
                .join("extensions"),
        );
    }
    roots.push(cwd.join(".pi").join("extensions"));

    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let dirs = std::fs::read_dir(&root)
            .with_context(|| format!("failed reading extension dir '{}'", root.display()))?;
        for entry in dirs {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = path.join(MANIFEST_FILE_NAME);
            if !manifest_path.exists() {
                continue;
            }
            let raw = std::fs::read_to_string(&manifest_path).with_context(|| {
                format!(
                    "failed to read extension manifest '{}'",
                    manifest_path.display()
                )
            })?;
            let manifest: ExtensionManifest = serde_json::from_str(&raw).with_context(|| {
                format!(
                    "failed to parse extension manifest '{}'",
                    manifest_path.display()
                )
            })?;
            out.push(Extension { manifest, path });
        }
    }
    Ok(out)
}

pub async fn register_extension_tools(
    agent: &pi_agent_core::Agent,
    extensions: &[Extension],
) -> usize {
    let mut count = 0usize;
    for extension in extensions {
        for tool in &extension.manifest.tools {
            if let Some(runtime_tool) = RuntimeExtensionTool::new(extension, tool) {
                agent.register_tool(Arc::new(runtime_tool)).await;
                count += 1;
            }
        }
    }
    count
}

/// Register extension commands with the command dispatcher.
pub fn register_extension_commands(
    dispatcher: &mut CommandDispatcher,
    extensions: &[Extension],
) -> usize {
    let mut count = 0usize;
    for extension in extensions {
        for cmd in &extension.manifest.commands {
            let ext_cmd = ExtensionCommandHandler {
                extension_path: extension.path.clone(),
                command: cmd.clone(),
            };
            dispatcher.register(&cmd.name, Box::new(ext_cmd));
            count += 1;
        }
    }
    count
}

/// Command handler trait for extension commands.
pub trait CommandHandler: Send + Sync {
    fn execute(&self, args: &[String]) -> Result<String>;
}

/// Command dispatcher for extension commands.
pub struct CommandDispatcher {
    commands: HashMap<String, Box<dyn CommandHandler>>,
}

impl CommandDispatcher {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }
    
    pub fn register(&mut self, name: &str, handler: Box<dyn CommandHandler>) {
        self.commands.insert(name.to_string(), handler);
    }
    
    pub fn get(&self, name: &str) -> Option<&dyn CommandHandler> {
        self.commands.get(name).map(|h| h.as_ref())
    }
    
    pub fn has(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }
    
    pub fn execute(&self, name: &str, args: &[String]) -> Result<String> {
        if let Some(handler) = self.get(name) {
            handler.execute(args)
        } else {
            anyhow::bail!("Unknown command: {}", name)
        }
    }
    
    pub fn list_commands(&self) -> Vec<&str> {
        self.commands.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension command handler.
struct ExtensionCommandHandler {
    extension_path: PathBuf,
    command: ExtensionCommand,
}

impl CommandHandler for ExtensionCommandHandler {
    fn execute(&self, args: &[String]) -> Result<String> {
        // For now, commands are executed as shell scripts in the extension directory
        // Sanitize command name to prevent path traversal
        let command_name = &self.command.name;
        if command_name.contains('/') || command_name.contains('\\') || command_name.contains("..") {
            anyhow::bail!("Invalid command name: {}", command_name);
        }
        
        let script_path = self.extension_path.join("commands").join(format!("{}.sh", command_name));
        
        // Verify the script exists before canonicalizing
        if !script_path.exists() {
            anyhow::bail!("Command script not found: {}", script_path.display());
        }

        // Ensure the resolved path is within the extension directory.
        // Both paths MUST canonicalize successfully — if either fails, reject.
        let canonical_script = script_path.canonicalize()
            .with_context(|| format!("Failed to resolve script path: {}", script_path.display()))?;
        let canonical_ext = self.extension_path.canonicalize()
            .with_context(|| format!("Failed to resolve extension path: {}", self.extension_path.display()))?;

        if !canonical_script.starts_with(&canonical_ext) {
            anyhow::bail!("Command script escapes extension directory");
        }
        
        let output = std::process::Command::new("bash")
            .arg(&script_path)
            .args(args)
            .current_dir(&self.extension_path)
            .output()
            .context("Failed to execute command")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Command failed: {}", stderr);
        }
        
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[derive(Debug, Clone)]
struct RuntimeExtensionTool {
    tool_name: String,
    description: String,
    parameters: Value,
    declared_tool_name: String,
    executor: RuntimeExecutor,
}

#[derive(Debug, Clone)]
enum RuntimeExecutor {
    Shell { command: String },
    Binary { path: String },
    Wasm { path: String },
}

impl RuntimeExtensionTool {
    fn new(extension: &Extension, tool: &ExtensionToolDef) -> Option<Self> {
        let executor = match &tool.executor {
            ExecutorType::Shell => {
                let command = match tool.command.clone() {
                    Some(c) if !c.trim().is_empty() => c,
                    _ => {
                        warn!(
                            extension = %extension.manifest.name,
                            tool = %tool.name,
                            "Skipping shell extension tool without command"
                        );
                        return None;
                    }
                };
                RuntimeExecutor::Shell { command }
            }
            ExecutorType::Binary => {
                let path = match tool.binary.clone() {
                    Some(p) if !p.trim().is_empty() => p,
                    _ => {
                        warn!(
                            extension = %extension.manifest.name,
                            tool = %tool.name,
                            "Skipping binary extension tool without binary path"
                        );
                        return None;
                    }
                };
                let full_path = extension.path.join(&path);
                RuntimeExecutor::Binary {
                    path: full_path.to_string_lossy().to_string(),
                }
            }
            ExecutorType::Wasm => {
                let path = match tool.binary.clone() {
                    Some(p) if !p.trim().is_empty() => p,
                    _ => {
                        warn!(
                            extension = %extension.manifest.name,
                            tool = %tool.name,
                            "Skipping WASM extension tool without wasm path"
                        );
                        return None;
                    }
                };
                let full_path = extension.path.join(&path);
                RuntimeExecutor::Wasm {
                    path: full_path.to_string_lossy().to_string(),
                }
            }
        };

        let tool_name = format!(
            "ext_{}_{}",
            slugify(&extension.manifest.name),
            slugify(&tool.name)
        );

        let description = if tool.description.trim().is_empty() {
            format!(
                "Extension tool '{}' from '{}'",
                tool.name, extension.manifest.name
            )
        } else {
            tool.description.clone()
        };

        let parameters = if tool.parameters.is_null() {
            serde_json::json!({ "type": "object" })
        } else {
            tool.parameters.clone()
        };

        Some(Self {
            tool_name,
            description,
            parameters,
            declared_tool_name: tool.name.clone(),
            executor,
        })
    }
}

#[async_trait]
impl AgentTool for RuntimeExtensionTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    fn requires_approval(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> pi_agent_core::Result<ToolResult> {
        let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(120);
        match &self.executor {
            RuntimeExecutor::Shell { command } => {
                let child = Command::new("bash")
                    .arg("-c")
                    .arg(command)
                    .current_dir(&ctx.cwd)
                    .env("PI_EXTENSION_ARGS", args.to_string())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                        tool_name: self.tool_name.clone(),
                        message: format!("Failed to spawn process: {e}"),
                    })?;

                execute_child(
                    child,
                    timeout_secs,
                    &ctx.abort,
                    &self.tool_name,
                    OutputMode::Plain,
                )
                .await
            }
            RuntimeExecutor::Binary { path } => {
                let mut child = Command::new(path)
                    .current_dir(&ctx.cwd)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                        tool_name: self.tool_name.clone(),
                        message: format!("Failed to spawn process: {e}"),
                    })?;

                if let Some(mut stdin) = child.stdin.take() {
                    let request = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": "1",
                        "method": "tool.execute",
                        "params": {
                            "tool": self.declared_tool_name.clone(),
                            "args": args,
                            "cwd": ctx.cwd.clone(),
                        }
                    });
                    stdin
                        .write_all(request.to_string().as_bytes())
                        .await
                        .map_err(|e| pi_agent_core::AgentError::ToolExecution {
                            tool_name: self.tool_name.clone(),
                            message: format!("Failed to write to binary stdin: {e}"),
                        })?;
                    stdin.write_all(b"\n").await.map_err(|e| {
                        pi_agent_core::AgentError::ToolExecution {
                            tool_name: self.tool_name.clone(),
                            message: format!("Failed to write newline to binary stdin: {e}"),
                        }
                    })?;
                }

                execute_child(
                    child,
                    timeout_secs,
                    &ctx.abort,
                    &self.tool_name,
                    OutputMode::BinaryJsonRpc,
                )
                .await
            }
            RuntimeExecutor::Wasm { path } => {
                execute_wasm(path, args, timeout_secs, &ctx.abort, &self.tool_name).await
            }
        }
    }
    
    fn clone_boxed(&self) -> Box<dyn AgentTool> {
        Box::new(RuntimeExtensionTool {
            tool_name: self.tool_name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            declared_tool_name: self.declared_tool_name.clone(),
            executor: self.executor.clone(),
        })
    }
}

async fn execute_wasm(
    path: &str,
    args: Value,
    timeout_secs: u64,
    abort: &tokio::sync::watch::Receiver<bool>,
    _tool_name: &str,
) -> pi_agent_core::Result<ToolResult> {
    let path_buf = PathBuf::from(path);
    let mut abort_rx = abort.clone();
    
    // Load and execute with timeout
    let result = tokio::select! {
        result = async {
            let cache = get_wasm_cache();
            cache.execute_file(&path_buf, args).await
        } => result,
        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)) => {
            return Ok(ToolResult::error(format!("WASM execution timed out after {timeout_secs}s")));
        }
        result = abort_rx.changed() => {
            if let Ok(()) = result {
                if *abort_rx.borrow() {
                    return Ok(ToolResult::error("WASM execution aborted"));
                }
            }
            return Ok(ToolResult::error("WASM execution aborted (watch error)"));
        }
    };
    
    match result {
        Ok(output) => Ok(ToolResult::success(output.to_string())),
        Err(e) => Ok(ToolResult::error(format!("WASM execution failed: {e}"))),
    }
}

#[derive(Clone, Copy)]
enum OutputMode {
    Plain,
    BinaryJsonRpc,
}

async fn execute_child(
    mut child: Child,
    timeout_secs: u64,
    abort: &tokio::sync::watch::Receiver<bool>,
    tool_name: &str,
    mode: OutputMode,
) -> pi_agent_core::Result<ToolResult> {
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();
    let timeout_dur = std::time::Duration::from_secs(timeout_secs);
    let mut abort_rx = abort.clone();

    let status = tokio::select! {
        result = tokio::time::timeout(timeout_dur, child.wait()) => {
            match result {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => return Ok(ToolResult::error(format!("Failed to wait for process: {e}"))),
                Err(_) => {
                    let _ = child.kill().await;
                    return Ok(ToolResult::error(format!("Command timed out after {timeout_secs}s")));
                }
            }
        }
        result = abort_rx.changed() => {
            let _ = result;
            if *abort_rx.borrow() {
                let _ = child.kill().await;
                return Ok(ToolResult::error("Command aborted"));
            }
            child.wait().await.map_err(|e| pi_agent_core::AgentError::ToolExecution {
                tool_name: tool_name.to_string(),
                message: format!("Failed to wait for process: {e}"),
            })?
        }
    };

    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    if let Some(ref mut h) = stdout_handle {
        let _ = h.read_to_end(&mut stdout_bytes).await;
    }
    if let Some(ref mut h) = stderr_handle {
        let _ = h.read_to_end(&mut stderr_bytes).await;
    }

    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

    if !status.success() {
        let mut out = if stdout.trim().is_empty() {
            String::new()
        } else {
            stdout
        };
        if !stderr.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("STDERR:\n");
            out.push_str(&stderr);
        }
        if out.is_empty() {
            out = "(no output)".to_string();
        }
        out.push_str(&format!("\nExit code: {}", status.code().unwrap_or(-1)));
        return Ok(ToolResult::error(out));
    }

    let result = match mode {
        OutputMode::Plain => build_plain_result(&stdout, &stderr),
        OutputMode::BinaryJsonRpc => build_binary_jsonrpc_result(&stdout, &stderr),
    };
    Ok(result)
}

fn build_plain_result(stdout: &str, stderr: &str) -> ToolResult {
    let mut result = String::new();
    if !stdout.trim().is_empty() {
        result.push_str(stdout);
    }
    if !stderr.trim().is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(stderr);
    }
    if result.is_empty() {
        result = "(no output)".to_string();
    }
    ToolResult::success(result)
}

fn build_binary_jsonrpc_result(stdout: &str, stderr: &str) -> ToolResult {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return build_plain_result(stdout, stderr);
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(err) = value.get("error") {
            return ToolResult::error(format!("Binary tool error: {}", err));
        }
        if let Some(result) = value.get("result") {
            if let Some(s) = result.as_str() {
                return ToolResult::success(s.to_string());
            }
            return ToolResult::success(result.to_string());
        }
        if let Some(output) = value.get("output").and_then(Value::as_str) {
            let success = value
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if success {
                return ToolResult::success(output.to_string());
            }
            return ToolResult::error(output.to_string());
        }
    }

    build_plain_result(stdout, stderr)
}

fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;

    for ch in name.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };

        if normalized == '_' {
            if !prev_underscore && !out.is_empty() {
                out.push('_');
            }
            prev_underscore = true;
        } else {
            out.push(normalized);
            prev_underscore = false;
        }
    }

    while out.ends_with('_') {
        out.pop();
    }

    if out.is_empty() {
        "extension".to_string()
    } else {
        out
    }
}

// -----------------------------------------------------------------------
// Tool Wrapping
// -----------------------------------------------------------------------

use types::ToolWrapperDef;

/// Registry for tool wrappers that can intercept and modify tool execution.
pub struct ToolWrapperRegistry {
    /// Map from tool name (or "*" for all) to wrapper definitions
    wrappers: HashMap<String, Vec<ToolWrapperDef>>,
}

impl ToolWrapperRegistry {
    /// Create an empty wrapper registry.
    pub fn new() -> Self {
        Self {
            wrappers: HashMap::new(),
        }
    }

    /// Register a tool wrapper.
    pub fn register(&mut self, wrapper: ToolWrapperDef) {
        let tool_name = wrapper.tool_name.clone();
        self.wrappers
            .entry(tool_name)
            .or_default()
            .push(wrapper);
    }

    /// Get all wrappers for a specific tool.
    pub fn get_wrappers(&self, tool_name: &str) -> Vec<&ToolWrapperDef> {
        let mut result = Vec::new();
        // Add global wrappers first
        if let Some(global) = self.wrappers.get("*") {
            result.extend(global.iter());
        }
        // Add tool-specific wrappers
        if let Some(specific) = self.wrappers.get(tool_name) {
            result.extend(specific.iter());
        }
        result
    }

    /// Check if a tool has any registered wrappers.
    pub fn has_wrappers(&self, tool_name: &str) -> bool {
        self.wrappers.contains_key("*") || self.wrappers.contains_key(tool_name)
    }

    /// Remove all wrappers for a specific tool.
    pub fn unregister(&mut self, tool_name: &str) {
        self.wrappers.remove(tool_name);
    }

    /// Clear all registered wrappers.
    pub fn clear(&mut self) {
        self.wrappers.clear();
    }
}

impl Default for ToolWrapperRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a wrapper hook script with the given context.
///
/// Returns (should_continue, modified_args, modified_result)
/// - should_continue: whether to proceed with the actual tool execution
/// - modified_args: potentially modified arguments for the tool
/// - modified_result: if set, this result should be used instead of executing
pub async fn execute_wrapper_hook(
    wrapper: &ToolWrapperDef,
    tool_name: &str,
    args: &Value,
    extension_path: &Path,
) -> anyhow::Result<(bool, Option<Value>, Option<ToolResult>)> {
    use std::process::Stdio;

    let hook_path = wrapper
        .before_hook
        .as_ref()
        .or(wrapper.after_hook.as_ref())
        .ok_or_else(|| anyhow::anyhow!("No hook script specified"))?;

    let full_hook_path = extension_path.join(hook_path);

    let context = serde_json::json!({
        "tool_name": tool_name,
        "args": args,
        "wrapper_type": "before",
    });

    let output = tokio::process::Command::new(&full_hook_path)
        .arg(context.to_string())
        .current_dir(extension_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed to execute wrapper hook: {}", full_hook_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Wrapper hook failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: WrapperHookResponse = serde_json::from_str(&stdout)
        .with_context(|| "Invalid wrapper hook response (expected JSON)")?;

    let should_continue = response.allow_continue.unwrap_or(true);
    let modified_args = response.modified_args;
    let modified_result = response.result.map(|r| ToolResult {
        content: r.output.unwrap_or_default(),
        is_error: !r.success,
        metadata: None,
    });

    Ok((should_continue, modified_args, modified_result))
}

/// Response format expected from wrapper hook scripts.
#[derive(Debug, Clone, Deserialize)]
struct WrapperHookResponse {
    /// Whether to continue with normal tool execution
    #[serde(default = "default_true_opt")]
    allow_continue: Option<bool>,
    /// Modified arguments to pass to the tool (if continuing)
    modified_args: Option<Value>,
    /// Pre-computed result to return instead of executing (if not continuing)
    result: Option<WrapperResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct WrapperResult {
    success: bool,
    output: Option<String>,
}

fn default_true_opt() -> Option<bool> {
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn discover_extensions_loads_manifests() {
        let tmp = TempDir::new().expect("tempdir");
        let project = tmp.path().join("project");
        let ext_dir = project.join(".pi").join("extensions").join("my-ext");
        std::fs::create_dir_all(&ext_dir).expect("mkdir");
        std::fs::write(
            ext_dir.join("extension.json"),
            r#"{
  "name": "my-ext",
  "version": "0.1.0",
  "description": "test extension",
  "tools": [
    {
      "name": "echo",
      "description": "Echo tool",
      "parameters": {"type":"object"},
      "executor": "shell",
      "command": "echo ok"
    }
  ],
  "commands": []
}"#,
        )
        .expect("write manifest");

        let extensions = discover_extensions(&project).expect("discover");
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].manifest.name, "my-ext");
        assert_eq!(extensions[0].manifest.tools.len(), 1);
    }

    #[test]
    fn runtime_tool_skips_wasm_executor_without_binary() {
        let extension = Extension {
            manifest: ExtensionManifest {
                name: "ext".to_string(),
                version: "0.1.0".to_string(),
                description: String::new(),
                tools: vec![],
                commands: vec![],
            },
            path: PathBuf::from("/tmp/ext"),
        };
        let wasm_tool = ExtensionToolDef {
            name: "w".to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type":"object"}),
            executor: ExecutorType::Wasm,
            command: Some("echo".to_string()),
            binary: None,
        };
        assert!(RuntimeExtensionTool::new(&extension, &wasm_tool).is_none());
    }

    #[test]
    fn runtime_tool_accepts_binary_executor() {
        let extension = Extension {
            manifest: ExtensionManifest {
                name: "ext".to_string(),
                version: "0.1.0".to_string(),
                description: String::new(),
                tools: vec![],
                commands: vec![],
            },
            path: PathBuf::from("/tmp/ext"),
        };
        let binary_tool = ExtensionToolDef {
            name: "b".to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type":"object"}),
            executor: ExecutorType::Binary,
            command: None,
            binary: Some("/usr/bin/true".to_string()),
        };
        assert!(RuntimeExtensionTool::new(&extension, &binary_tool).is_some());
    }

    #[test]
    fn binary_jsonrpc_result_parses_result() {
        let result =
            build_binary_jsonrpc_result(r#"{"jsonrpc":"2.0","id":"1","result":{"ok":true}}"#, "");
        assert!(!result.is_error);
        assert_eq!(result.content, r#"{"ok":true}"#);
    }

    #[test]
    fn command_dispatcher_registers_and_executes() {
        struct TestHandler;
        impl CommandHandler for TestHandler {
            fn execute(&self, args: &[String]) -> Result<String> {
                Ok(format!("executed with {} args", args.len()))
            }
        }

        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register("test", Box::new(TestHandler));
        
        assert!(dispatcher.has("test"));
        assert!(!dispatcher.has("unknown"));
        
        let result = dispatcher.execute("test", &["arg1".to_string()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "executed with 1 args");
    }

    #[test]
    fn tool_wrapper_registry_registers_and_retrieves() {
        let mut registry = ToolWrapperRegistry::new();
        
        let wrapper = ToolWrapperDef {
            tool_name: "read".to_string(),
            description: "Log all read calls".to_string(),
            wrapper_type: types::WrapperType::Shell,
            before_hook: Some("wrap.sh".to_string()),
            after_hook: None,
        };
        
        registry.register(wrapper);
        
        assert!(registry.has_wrappers("read"));
        assert!(!registry.has_wrappers("write"));
        
        let wrappers = registry.get_wrappers("read");
        assert_eq!(wrappers.len(), 1);
        assert_eq!(wrappers[0].tool_name, "read");
    }

    #[test]
    fn tool_wrapper_registry_global_wildcard() {
        let mut registry = ToolWrapperRegistry::new();
        
        let global = ToolWrapperDef {
            tool_name: "*".to_string(),
            description: "Log all tool calls".to_string(),
            wrapper_type: types::WrapperType::Shell,
            before_hook: Some("global_wrap.sh".to_string()),
            after_hook: None,
        };
        
        registry.register(global);
        
        // Any tool should have the global wrapper
        assert!(registry.has_wrappers("read"));
        assert!(registry.has_wrappers("write"));
        assert!(registry.has_wrappers("any_tool"));
    }

    #[test]
    fn tool_wrapper_registry_combines_global_and_specific() {
        let mut registry = ToolWrapperRegistry::new();
        
        let global = ToolWrapperDef {
            tool_name: "*".to_string(),
            description: "Global".to_string(),
            wrapper_type: types::WrapperType::Shell,
            before_hook: Some("global.sh".to_string()),
            after_hook: None,
        };
        
        let specific = ToolWrapperDef {
            tool_name: "read".to_string(),
            description: "Specific".to_string(),
            wrapper_type: types::WrapperType::Shell,
            before_hook: Some("read.sh".to_string()),
            after_hook: None,
        };
        
        registry.register(global);
        registry.register(specific);
        
        let wrappers = registry.get_wrappers("read");
        assert_eq!(wrappers.len(), 2);
        // Global should come first
        assert_eq!(wrappers[0].tool_name, "*");
        assert_eq!(wrappers[1].tool_name, "read");
    }
}
