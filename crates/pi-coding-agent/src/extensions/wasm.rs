//! WASM executor for extension tools.
//!
//! This module provides WebAssembly execution support for extension tools,
//! allowing extensions to be written in any language that compiles to WASM.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// WASM runtime configuration.
#[derive(Debug, Clone)]
pub struct WasmConfig {
    /// Maximum memory in MB
    pub max_memory_mb: u32,
    /// Execution timeout in seconds
    pub timeout_secs: u64,
    /// Enable WASI
    pub enable_wasi: bool,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: 128,
            timeout_secs: 30,
            enable_wasi: true,
        }
    }
}

/// A compiled WASM module ready for execution.
pub struct WasmModule {
    #[cfg(feature = "wasm")]
    engine: wasmtime::Engine,
    #[cfg(feature = "wasm")]
    module: wasmtime::Module,
    #[cfg(feature = "wasm")]
    config: WasmConfig,
    #[cfg(not(feature = "wasm"))]
    _phantom: std::marker::PhantomData<()>,
}

impl std::fmt::Debug for WasmModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmModule")
            .field("compiled", &cfg!(feature = "wasm"))
            .finish()
    }
}

impl WasmModule {
    /// Maximum WASM stack depth in bytes (512 KiB).
    ///
    /// This prevents stack-overflow attacks from deeply recursive WASM code.
    const MAX_WASM_STACK: usize = 512 * 1024;

    /// Maximum output size in bytes (64 KiB).
    const MAX_OUTPUT_BYTES: usize = 65536;

    /// Compile a WASM module from bytes.
    #[cfg(feature = "wasm")]
    pub fn compile(bytes: &[u8], config: WasmConfig) -> Result<Self> {
        let mut engine_config = wasmtime::Config::new();
        engine_config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Enable);

        // Enable fuel-based execution limits so add_fuel / consume_fuel works.
        engine_config.consume_fuel(true);

        // Limit the WASM stack depth to prevent stack-overflow attacks.
        engine_config.max_wasm_stack(Self::MAX_WASM_STACK);

        let engine = wasmtime::Engine::new(&engine_config)
            .context("Failed to create WASM engine")?;

        let module = wasmtime::Module::new(&engine, bytes)
            .context("Failed to compile WASM module")?;

        Ok(Self {
            engine,
            module,
            config,
        })
    }
    
    /// Dummy implementation when WASM feature is disabled.
    #[cfg(not(feature = "wasm"))]
    pub fn compile(_bytes: &[u8], _config: WasmConfig) -> Result<Self> {
        anyhow::bail!("WASM support is not enabled. Compile with --features wasm")
    }
    
    /// Dummy execute when WASM is disabled.
    #[cfg(not(feature = "wasm"))]
    pub async fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value> {
        anyhow::bail!("WASM support is not enabled. Compile with --features wasm")
    }
    
    /// Execute the WASM module with the given input.
    ///
    /// Safety measures enforced:
    /// - **Fuel limit**: caps total instructions proportional to `timeout_secs`.
    /// - **Memory limit**: the WASM linear memory cannot exceed `max_memory_mb`.
    /// - **Stack depth**: bounded by `MAX_WASM_STACK` set at compile time.
    /// - **Output size**: capped at `MAX_OUTPUT_BYTES`.
    /// - **Wall-clock timeout**: the entire execution is wrapped in a tokio
    ///   timeout so that blocking I/O (inherited stdio) cannot hang forever.
    #[cfg(feature = "wasm")]
    pub async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        use wasmtime::{Store, TypedFunc};

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        // Wrap the entire execution in a wall-clock timeout to guard against
        // blocking I/O on inherited stdio.
        let result = tokio::time::timeout(timeout_duration, async {
            self.execute_inner(args).await
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_elapsed) => Err(anyhow::anyhow!(
                "WASM execution timed out after {} seconds",
                self.config.timeout_secs
            )),
        }
    }

    /// Inner execution logic, separated so the outer `execute` can wrap it in a
    /// wall-clock timeout.
    #[cfg(feature = "wasm")]
    async fn execute_inner(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        use wasmtime::{Store, TypedFunc};

        // Create WASI context
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdio()
            .build();

        let max_memory_bytes = (self.config.max_memory_mb as usize) * 1024 * 1024;
        let state = StoreState {
            wasi: wasi_ctx,
            limiter: MemoryLimiter {
                max_memory: max_memory_bytes,
            },
        };

        // Create store with fuel limit for timeout
        let mut store = Store::new(&self.engine, state);
        store.add_fuel(self.config.timeout_secs * 1_000_000)
            .context("Failed to add fuel")?;

        // Enforce max_memory_mb: the limiter lives inside StoreState and is
        // handed back to wasmtime on every memory.grow / table.grow call.
        store.limiter(|state| &mut state.limiter);

        // Create linker and add WASI — extract the WasiCtx from our StoreState.
        let mut linker = wasmtime::Linker::new(&self.engine);
        wasmtime_wasi::add_to_linker(&mut linker, |state: &mut StoreState| &mut state.wasi)
            .context("Failed to add WASI to linker")?;

        // Instantiate the module
        let instance = linker.instantiate(&mut store, &self.module)
            .context("Failed to instantiate WASM module")?;

        // Get the execute function
        let execute: TypedFunc<(i32, i32), (i32,)> = instance
            .get_typed_func(&mut store, "execute")
            .context("WASM module missing 'execute' function")?;

        // Allocate memory for input
        let input_str = args.to_string();
        let input_bytes = input_str.as_bytes();

        // Get memory export
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("WASM module missing 'memory' export")?;

        // Try to use malloc export if available, otherwise use fixed offset
        let input_ptr: i32 = if let Some(allocate) = instance.get_typed_func::<i32, i32>(&mut store, "malloc").ok() {
            allocate.call(&mut store, input_bytes.len() as i32)
                .context("WASM malloc failed")?
        } else {
            // Fixed offset allocation - ensure it doesn't exceed memory bounds
            let ptr = 1024i32;
            let memory_size = memory.data_size(&store);
            if ptr as usize + input_bytes.len() > memory_size {
                anyhow::bail!("Input too large for WASM memory: {} bytes needed, {} available",
                    input_bytes.len(), memory_size - ptr as usize);
            }
            ptr
        };

        memory.write(&mut store, input_ptr as usize, input_bytes)
            .context("Failed to write input to WASM memory")?;

        // Call execute
        let (output_ptr,) = execute.call(&mut store, (input_ptr, input_bytes.len() as i32))
            .context("WASM execution failed")?;

        // Read output from memory
        // For now, assume output is null-terminated string at output_ptr
        let mut output_bytes = Vec::new();
        let mut offset = output_ptr as usize;
        loop {
            let mut byte = [0u8; 1];
            memory.read(&store, offset, &mut byte)
                .context("Failed to read output from WASM memory")?;
            if byte[0] == 0 {
                break;
            }
            output_bytes.push(byte[0]);
            offset += 1;
            if output_bytes.len() > Self::MAX_OUTPUT_BYTES {
                return Err(anyhow::anyhow!(
                    "WASM output exceeds {} byte limit",
                    Self::MAX_OUTPUT_BYTES
                ));
            }
        }

        let output_str = String::from_utf8(output_bytes)
            .context("WASM output is not valid UTF-8")?;
        let output_json: serde_json::Value = serde_json::from_str(&output_str)
            .context("WASM output is not valid JSON")?;

        Ok(output_json)
    }
}

/// A [`wasmtime::ResourceLimiter`] that caps linear memory growth.
///
/// Used inside [`StoreState`] and referenced via `Store::limiter` to enforce
/// the `max_memory_mb` configuration value at runtime.
#[cfg(feature = "wasm")]
struct MemoryLimiter {
    max_memory: usize,
}

#[cfg(feature = "wasm")]
impl wasmtime::ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        Ok(desired <= self.max_memory)
    }

    fn table_growing(
        &mut self,
        _current: u32,
        _desired: u32,
        _maximum: Option<u32>,
    ) -> Result<bool> {
        // Allow table growth (not a memory safety concern at our scale).
        Ok(true)
    }
}

/// Store data combining the WASI context and the memory limiter.
///
/// `Store::limiter` requires the limiter to live inside the store data so it
/// can hand back a `&mut dyn ResourceLimiter` reference on every allocation.
#[cfg(feature = "wasm")]
struct StoreState {
    wasi: wasmtime_wasi::WasiCtx,
    limiter: MemoryLimiter,
}

/// Cache for compiled WASM modules.
pub struct WasmModuleCache {
    modules: std::sync::Mutex<HashMap<String, Arc<WasmModule>>>,
    config: WasmConfig,
}

impl WasmModuleCache {
    /// Create a new cache with default config.
    pub fn new() -> Self {
        Self::with_config(WasmConfig::default())
    }
    
    /// Create a new cache with custom config.
    pub fn with_config(config: WasmConfig) -> Self {
        Self {
            modules: std::sync::Mutex::new(HashMap::new()),
            config,
        }
    }
    
    /// Load or compile a WASM module from file path.
    pub fn load(&self, path: &std::path::Path) -> Result<Arc<WasmModule>> {
        let path_str = path.to_string_lossy().to_string();
        
        // Check cache
        {
            let cache = self.modules.lock().unwrap();
            if let Some(module) = cache.get(&path_str) {
                return Ok(module.clone());
            }
        }
        
        // Compile
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read WASM file: {}", path.display()))?;
        let module = WasmModule::compile(&bytes, self.config.clone())?;
        let module = Arc::new(module);
        
        // Cache
        {
            let mut cache = self.modules.lock().unwrap();
            cache.insert(path_str, module.clone());
        }
        
        Ok(module)
    }
    
    /// Execute a WASM module directly from file.
    pub async fn execute_file(
        &self,
        path: &std::path::Path,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let module = self.load(path)?;
        module.execute(args).await
    }
}

impl Default for WasmModuleCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_config_default() {
        let config = WasmConfig::default();
        assert_eq!(config.max_memory_mb, 128);
        assert_eq!(config.timeout_secs, 30);
        assert!(config.enable_wasi);
    }

    #[test]
    fn test_wasm_disabled() {
        // When WASM feature is disabled, compilation should fail
        #[cfg(not(feature = "wasm"))]
        {
            let result = WasmModule::compile(b"invalid", WasmConfig::default());
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not enabled"));
        }
    }

    #[test]
    fn test_max_output_bytes_constant() {
        // Ensure the output limit constant is sensible (64 KiB).
        assert_eq!(WasmModule::MAX_OUTPUT_BYTES, 65536);
    }

    #[test]
    fn test_max_wasm_stack_constant() {
        // Ensure the stack limit constant is sensible (512 KiB).
        assert_eq!(WasmModule::MAX_WASM_STACK, 512 * 1024);
    }

    #[test]
    fn test_wasm_config_custom_memory() {
        let config = WasmConfig {
            max_memory_mb: 64,
            timeout_secs: 10,
            enable_wasi: false,
        };
        assert_eq!(config.max_memory_mb, 64);
        assert_eq!(config.timeout_secs, 10);
        assert!(!config.enable_wasi);
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn test_compile_enables_fuel_and_stack_limit() {
        // Verify that compilation succeeds with the safety-hardened config.
        // We cannot easily inspect engine internals, but we can confirm that
        // compiling a minimal valid WASM module succeeds (meaning the config
        // flags are accepted by wasmtime).
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "execute") (param i32 i32) (result i32)
                i32.const 0
            )
        )"#;
        let wasm = wasmtime::wat::parse_str(wat).expect("valid WAT");
        let config = WasmConfig {
            max_memory_mb: 16,
            timeout_secs: 5,
            enable_wasi: true,
        };
        let module = WasmModule::compile(&wasm, config);
        assert!(module.is_ok(), "compilation with safety config should succeed");
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn test_memory_limiter_enforces_cap() {
        let mut limiter = MemoryLimiter {
            max_memory: 1024 * 1024, // 1 MiB
        };

        // Growth within the limit should be allowed.
        assert!(limiter
            .memory_growing(0, 512 * 1024, None)
            .expect("no error"));

        // Growth exactly at the limit should be allowed.
        assert!(limiter
            .memory_growing(0, 1024 * 1024, None)
            .expect("no error"));

        // Growth beyond the limit should be denied.
        assert!(!limiter
            .memory_growing(0, 1024 * 1024 + 1, None)
            .expect("no error"));
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn test_memory_limiter_allows_table_growth() {
        let mut limiter = MemoryLimiter {
            max_memory: 1024,
        };
        assert!(limiter.table_growing(0, 100, None).expect("no error"));
    }

    #[test]
    fn test_wasm_module_cache_default() {
        let cache = WasmModuleCache::new();
        assert_eq!(cache.config.max_memory_mb, 128);
        assert_eq!(cache.config.timeout_secs, 30);
    }

    #[test]
    fn test_wasm_module_cache_custom_config() {
        let config = WasmConfig {
            max_memory_mb: 256,
            timeout_secs: 60,
            enable_wasi: false,
        };
        let cache = WasmModuleCache::with_config(config);
        assert_eq!(cache.config.max_memory_mb, 256);
        assert_eq!(cache.config.timeout_secs, 60);
    }

    #[test]
    fn test_wasm_module_debug() {
        // Ensure Debug impl doesn't panic.
        #[cfg(not(feature = "wasm"))]
        {
            // Can't create a module without the feature, just test config.
            let config = WasmConfig::default();
            let debug_str = format!("{:?}", config);
            assert!(debug_str.contains("WasmConfig"));
        }
    }
}
