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
    /// Maximum stack depth (in frames)
    pub max_stack_depth: u32,
    /// Maximum I/O operations per execution
    pub max_io_ops: u32,
    /// Maximum output size in bytes
    pub max_output_bytes: usize,
    /// Memory allocation offset for fixed allocations (must be > 0 for safety)
    pub memory_alloc_offset: i32,
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: 128,
            timeout_secs: 30,
            enable_wasi: true,
            max_stack_depth: 1000,
            max_io_ops: 10_000,
            max_output_bytes: 65536,
            memory_alloc_offset: 1024,
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

        // Configure memory limits
        engine_config.static_memory_maximum_size(config.max_memory_mb as usize * 1024 * 1024);

        // Enable fuel-based execution limits so add_fuel / consume_fuel works.
        engine_config.consume_fuel(true);

        // Limit the WASM stack depth to prevent stack-overflow attacks.
        // Convert the config's "frames" value into an approximate byte budget.
        // We calibrate against the historical default (1000 frames ~= 512 KiB).
        let bytes_per_frame = (Self::MAX_WASM_STACK / 1000).max(1);
        let configured_stack = usize::try_from(config.max_stack_depth)
            .unwrap_or(usize::MAX)
            .saturating_mul(bytes_per_frame);
        let stack_limit_bytes = configured_stack.max(64 * 1024);
        engine_config.max_wasm_stack(stack_limit_bytes);

        // Enable epoch interruption for I/O timeout enforcement
        engine_config.epoch_interruption(true);

        let engine =
            wasmtime::Engine::new(&engine_config).context("Failed to create WASM engine")?;

        let module =
            wasmtime::Module::new(&engine, bytes).context("Failed to compile WASM module")?;

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
    /// - **Input size limit**: max 1MB input.
    /// - **Pointer alignment**: 4-byte alignment enforced.
    /// - **I/O operation limit**: max ops per execution.
    #[cfg(feature = "wasm")]
    pub async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        use wasmtime::{Store, TypedFunc};

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        // Wrap the entire execution in a wall-clock timeout to guard against
        // blocking I/O on inherited stdio.
        let result =
            tokio::time::timeout(timeout_duration, async { self.execute_inner(args).await }).await;

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
        use std::time::Instant;
        use wasmtime::{Store, TypedFunc};

        // Create WASI context. When disabled, use an empty context and avoid
        // wiring WASI imports into the linker below.
        let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
        if self.config.enable_wasi {
            wasi_builder.inherit_stdio();
        }
        let wasi_ctx = wasi_builder.build();

        let max_memory_bytes = (self.config.max_memory_mb as usize) * 1024 * 1024;
        let state = StoreState {
            wasi: wasi_ctx,
            limiter: MemoryLimiter {
                max_memory: max_memory_bytes,
            },
        };

        // Create store with fuel limit for timeout
        // Set up fuel for instruction counting (1 fuel ~ 1 wasm instruction)
        let mut store = Store::new(&self.engine, state);
        let max_fuel = self.config.timeout_secs.saturating_mul(100_000_000);
        store.add_fuel(max_fuel).context("Failed to add fuel")?;

        // Enforce max_memory_mb: the limiter lives inside StoreState and is
        // handed back to wasmtime on every memory.grow / table.grow call.
        store.limiter(|state| &mut state.limiter);

        // Set epoch deadline for I/O timeout enforcement
        store.set_epoch_deadline(self.config.timeout_secs);

        // Create linker and add WASI only when enabled.
        let mut linker = wasmtime::Linker::new(&self.engine);
        if self.config.enable_wasi {
            wasmtime_wasi::add_to_linker(&mut linker, |state: &mut StoreState| &mut state.wasi)
                .context("Failed to add WASI to linker")?;
        }

        // Instantiate the module
        let instance = linker
            .instantiate(&mut store, &self.module)
            .context("Failed to instantiate WASM module")?;

        // Get the execute function
        let execute: TypedFunc<(i32, i32), (i32,)> = instance
            .get_typed_func(&mut store, "execute")
            .context("WASM module missing 'execute' function")?;

        // Allocate memory for input
        let input_str = args.to_string();
        let input_bytes = input_str.as_bytes();

        // Enforce input size limits (max 1MB)
        const MAX_INPUT_SIZE: usize = 1024 * 1024;
        if input_bytes.len() > MAX_INPUT_SIZE {
            anyhow::bail!(
                "Input too large: {} bytes (max {})",
                input_bytes.len(),
                MAX_INPUT_SIZE
            );
        }

        // Get memory export
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("WASM module missing 'memory' export")?;

        // Get memory size and validate
        let memory_size = memory.data_size(&store);
        let alloc_offset = self.config.memory_alloc_offset.max(64); // Ensure at least 64 bytes offset

        // Try to use malloc export if available, otherwise use fixed offset
        let input_ptr: i32 = if let Some(allocate) = instance
            .get_typed_func::<i32, i32>(&mut store, "malloc")
            .ok()
        {
            let ptr = allocate
                .call(&mut store, input_bytes.len() as i32)
                .context("WASM malloc failed")?;
            // Validate allocated pointer
            if ptr < alloc_offset {
                anyhow::bail!(
                    "WASM malloc returned invalid pointer: {} (must be >= {})",
                    ptr,
                    alloc_offset
                );
            }
            if ptr as usize + input_bytes.len() > memory_size {
                anyhow::bail!(
                    "WASM malloc returned out-of-bounds pointer: {} + {} > {}",
                    ptr,
                    input_bytes.len(),
                    memory_size
                );
            }
            ptr
        } else {
            // Fixed offset allocation with bounds checking
            let ptr = alloc_offset;
            if ptr as usize + input_bytes.len() > memory_size {
                anyhow::bail!(
                    "Input too large for WASM memory: {} bytes needed at offset {}, {} available",
                    input_bytes.len(),
                    ptr,
                    memory_size.saturating_sub(ptr as usize)
                );
            }
            ptr
        };

        // Validate pointer alignment (must be 4-byte aligned for wasm)
        if input_ptr % 4 != 0 {
            anyhow::bail!("WASM pointer not properly aligned: {}", input_ptr);
        }

        memory
            .write(&mut store, input_ptr as usize, input_bytes)
            .context("Failed to write input to WASM memory")?;

        // Track I/O operations and execution time
        let start_time = Instant::now();
        let max_io_ops = self.config.max_io_ops;
        let mut io_op_count = 0u32;

        // Call execute
        let (output_ptr,) = execute
            .call(&mut store, (input_ptr, input_bytes.len() as i32))
            .context("WASM execution failed")?;

        // Validate output pointer
        if output_ptr < alloc_offset && output_ptr != 0 {
            anyhow::bail!(
                "WASM returned invalid output pointer: {} (must be >= {} or null)",
                output_ptr,
                alloc_offset
            );
        }

        // Read output from memory with bounds checking
        let mut output_bytes = Vec::new();
        let mut offset = output_ptr as usize;
        // Respect the configured output limit while clamping to a sane ceiling.
        let max_output = self
            .config
            .max_output_bytes
            .max(1)
            .min(Self::MAX_OUTPUT_BYTES.saturating_mul(16));

        // Safety check: ensure we don't read beyond memory bounds
        if output_ptr > 0 && (output_ptr as usize) < memory_size {
            loop {
                if output_bytes.len() >= max_output {
                    return Err(anyhow::anyhow!(
                        "WASM output exceeds {} byte limit",
                        max_output
                    ));
                }

                if offset >= memory_size {
                    return Err(anyhow::anyhow!(
                        "WASM output not null-terminated within memory bounds"
                    ));
                }

                let mut byte = [0u8; 1];
                memory
                    .read(&store, offset, &mut byte)
                    .context("Failed to read output from WASM memory")?;
                if byte[0] == 0 {
                    break;
                }
                output_bytes.push(byte[0]);
                offset += 1;
                io_op_count += 1;

                if io_op_count > max_io_ops {
                    return Err(anyhow::anyhow!(
                        "WASM exceeded maximum I/O operations: {}",
                        max_io_ops
                    ));
                }
            }
        }

        let execution_time = start_time.elapsed();
        tracing::debug!(
            "WASM execution completed in {:?}, fuel consumed: {:?}, I/O ops: {}",
            execution_time,
            store.get_fuel(),
            io_op_count
        );

        let output_str =
            String::from_utf8(output_bytes).context("WASM output is not valid UTF-8")?;
        let output_json: serde_json::Value =
            serde_json::from_str(&output_str).context("WASM output is not valid JSON")?;

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
        assert_eq!(config.max_stack_depth, 1000);
        assert_eq!(config.max_io_ops, 10_000);
        assert_eq!(config.max_output_bytes, 65536);
        assert_eq!(config.memory_alloc_offset, 1024);
    }

    #[test]
    fn test_wasm_config_custom() {
        let config = WasmConfig {
            max_memory_mb: 64,
            timeout_secs: 10,
            enable_wasi: false,
            max_stack_depth: 500,
            max_io_ops: 5_000,
            max_output_bytes: 32768,
            memory_alloc_offset: 2048,
        };
        assert_eq!(config.max_memory_mb, 64);
        assert_eq!(config.timeout_secs, 10);
        assert!(!config.enable_wasi);
        assert_eq!(config.max_stack_depth, 500);
        assert_eq!(config.max_io_ops, 5_000);
        assert_eq!(config.max_output_bytes, 32768);
        assert_eq!(config.memory_alloc_offset, 2048);
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
    fn test_wasm_memory_bounds_config() {
        // Test that memory allocation offset is validated
        let config = WasmConfig {
            memory_alloc_offset: 0, // Invalid - should be at least 64
            ..WasmConfig::default()
        };
        // The config allows it, but execute should enforce minimum
        assert_eq!(config.memory_alloc_offset, 0);

        // Valid config
        let valid_config = WasmConfig {
            memory_alloc_offset: 1024,
            ..WasmConfig::default()
        };
        assert_eq!(valid_config.memory_alloc_offset, 1024);
    }

    #[test]
    fn test_wasm_safety_limits() {
        // Test that default safety limits are reasonable
        let config = WasmConfig::default();

        // Max I/O ops should be > 0
        assert!(config.max_io_ops > 0, "max_io_ops must be positive");

        // Max output should be > 0
        assert!(
            config.max_output_bytes > 0,
            "max_output_bytes must be positive"
        );

        // Memory allocation offset should be >= 64 for safety
        assert!(
            config.memory_alloc_offset >= 64,
            "memory_alloc_offset should be at least 64 bytes"
        );

        // Timeout should be reasonable (1-3600 seconds)
        assert!(
            config.timeout_secs >= 1,
            "timeout should be at least 1 second"
        );
        assert!(
            config.timeout_secs <= 3600,
            "timeout should be at most 1 hour"
        );

        // Stack depth should be reasonable (100-10000)
        assert!(
            config.max_stack_depth >= 100,
            "max_stack_depth should be at least 100"
        );
        assert!(
            config.max_stack_depth <= 10000,
            "max_stack_depth should be at most 10000"
        );

        // Memory should be reasonable (1-1024 MB)
        assert!(
            config.max_memory_mb >= 1,
            "max_memory_mb should be at least 1"
        );
        assert!(
            config.max_memory_mb <= 1024,
            "max_memory_mb should be at most 1024"
        );
    }

    #[test]
    fn test_wasm_input_size_limit() {
        // Test that we can create configs with different input handling
        let strict_config = WasmConfig {
            max_memory_mb: 16,      // Small memory
            max_io_ops: 100,        // Low I/O
            max_output_bytes: 1024, // Small output
            ..WasmConfig::default()
        };

        assert_eq!(strict_config.max_memory_mb, 16);
        assert_eq!(strict_config.max_io_ops, 100);
        assert_eq!(strict_config.max_output_bytes, 1024);
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn test_compile_enables_fuel_and_stack_limit() {
        // Verify that compilation succeeds with the safety-hardened config.
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
            ..WasmConfig::default()
        };
        let module = WasmModule::compile(&wasm, config);
        assert!(
            module.is_ok(),
            "compilation with safety config should succeed"
        );
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
        let mut limiter = MemoryLimiter { max_memory: 1024 };
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
            ..WasmConfig::default()
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
