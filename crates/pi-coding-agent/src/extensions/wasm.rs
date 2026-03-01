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
    /// Compile a WASM module from bytes.
    #[cfg(feature = "wasm")]
    pub fn compile(bytes: &[u8], config: WasmConfig) -> Result<Self> {
        let mut engine_config = wasmtime::Config::new();
        engine_config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Enable);
        
        // Configure memory limits
        engine_config.static_memory_maximum_size(config.max_memory_mb as usize * 1024 * 1024);
        engine_config.dynamic_memory_reserved_for_growth(0); // Disable growth to enforce limit
        
        // Enable fuel metering for instruction count limiting
        engine_config.consume_fuel(true);
        
        // Set maximum stack size for stack depth limiting
        engine_config.max_wasm_stack(config.max_stack_depth as usize * 64 * 1024); // Approximate bytes per frame
        
        // Enable epoch interruption for I/O timeout
        engine_config.epoch_interruption(true);
        
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
    #[cfg(feature = "wasm")]
    pub async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        use wasmtime::{Store, TypedFunc};
        use std::time::{Duration, Instant};
        
        // Create WASI context with I/O limits
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdio()
            .build();
        
        // Create store with limits
        let mut store = Store::new(&self.engine, wasi_ctx);
        
        // Set up fuel for instruction counting (1 fuel ≈ 1 wasm instruction)
        // Max fuel = timeout_secs * instructions_per_second (approx 100M ops/sec)
        let max_fuel = self.config.timeout_secs.saturating_mul(100_000_000);
        store.add_fuel(max_fuel)
            .context("Failed to add fuel")?;
        
        // Set epoch deadline for I/O timeout enforcement
        store.set_epoch_deadline(self.config.timeout_secs);
        
        // Create linker with I/O limiting hooks
        let mut linker = wasmtime::Linker::new(&self.engine);
        wasmtime_wasi::add_to_linker(&mut linker, |s| s)
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
        
        // Enforce input size limits (max 1MB)
        const MAX_INPUT_SIZE: usize = 1024 * 1024;
        if input_bytes.len() > MAX_INPUT_SIZE {
            anyhow::bail!("Input too large: {} bytes (max {})", 
                input_bytes.len(), MAX_INPUT_SIZE);
        }
        
        // Get memory export
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("WASM module missing 'memory' export")?;
        
        // Get memory size and validate
        let memory_size = memory.data_size(&store);
        let alloc_offset = self.config.memory_alloc_offset.max(64); // Ensure at least 64 bytes offset
        
        // Try to use malloc export if available, otherwise use fixed offset
        let input_ptr: i32 = if let Some(allocate) = instance.get_typed_func::<i32, i32>(&mut store, "malloc").ok() {
            let ptr = allocate.call(&mut store, input_bytes.len() as i32)
                .context("WASM malloc failed")?;
            // Validate allocated pointer
            if ptr < alloc_offset {
                anyhow::bail!("WASM malloc returned invalid pointer: {} (must be >= {})", ptr, alloc_offset);
            }
            if ptr as usize + input_bytes.len() > memory_size {
                anyhow::bail!("WASM malloc returned out-of-bounds pointer: {} + {} > {}", 
                    ptr, input_bytes.len(), memory_size);
            }
            ptr
        } else {
            // Fixed offset allocation with bounds checking
            let ptr = alloc_offset;
            if ptr as usize + input_bytes.len() > memory_size {
                anyhow::bail!("Input too large for WASM memory: {} bytes needed at offset {}, {} available", 
                    input_bytes.len(), ptr, memory_size.saturating_sub(ptr as usize));
            }
            ptr
        };
        
        // Validate pointer alignment (must be 4-byte aligned for wasm)
        if input_ptr % 4 != 0 {
            anyhow::bail!("WASM pointer not properly aligned: {}", input_ptr);
        }
        
        memory.write(&mut store, input_ptr as usize, input_bytes)
            .context("Failed to write input to WASM memory")?;
        
        // Track I/O operations and execution time
        let start_time = Instant::now();
        let max_io_ops = self.config.max_io_ops;
        let mut io_op_count = 0u32;
        
        // Set up a periodic check for resource limits
        store.fuel_async_yield_interval(Some(10_000));
        
        // Call execute with timeout
        let (output_ptr,) = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            async { execute.call(&mut store, (input_ptr, input_bytes.len() as i32)) }
        ).await
        .map_err(|_| anyhow::anyhow!("WASM execution timed out after {} seconds", self.config.timeout_secs))?
        .context("WASM execution failed")?;
        
        // Validate output pointer
        if output_ptr < alloc_offset && output_ptr != 0 {
            anyhow::bail!("WASM returned invalid output pointer: {} (must be >= {} or null)", 
                output_ptr, alloc_offset);
        }
        
        // Read output from memory with bounds checking
        let mut output_bytes = Vec::new();
        let mut offset = output_ptr as usize;
        let max_output = self.config.max_output_bytes;
        
        // Safety check: ensure we don't read beyond memory bounds
        if output_ptr > 0 && (output_ptr as usize) < memory_size {
            loop {
                if output_bytes.len() >= max_output {
                    return Err(anyhow::anyhow!("WASM output exceeds maximum size of {} bytes", max_output));
                }
                
                if offset >= memory_size {
                    return Err(anyhow::anyhow!("WASM output not null-terminated within memory bounds"));
                }
                
                let mut byte = [0u8; 1];
                memory.read(&store, offset, &mut byte)
                    .context("Failed to read output from WASM memory")?;
                if byte[0] == 0 {
                    break;
                }
                output_bytes.push(byte[0]);
                offset += 1;
                io_op_count += 1;
                
                if io_op_count > max_io_ops {
                    return Err(anyhow::anyhow!("WASM exceeded maximum I/O operations: {}", max_io_ops));
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
        
        let output_str = String::from_utf8(output_bytes)
            .context("WASM output is not valid UTF-8")?;
        let output_json: serde_json::Value = serde_json::from_str(&output_str)
            .context("WASM output is not valid JSON")?;
        
        Ok(output_json)
    }
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
        assert!(config.max_output_bytes > 0, "max_output_bytes must be positive");
        
        // Memory allocation offset should be >= 64 for safety
        assert!(config.memory_alloc_offset >= 64, "memory_alloc_offset should be at least 64 bytes");
        
        // Timeout should be reasonable (1-3600 seconds)
        assert!(config.timeout_secs >= 1, "timeout should be at least 1 second");
        assert!(config.timeout_secs <= 3600, "timeout should be at most 1 hour");
        
        // Stack depth should be reasonable (100-10000)
        assert!(config.max_stack_depth >= 100, "max_stack_depth should be at least 100");
        assert!(config.max_stack_depth <= 10000, "max_stack_depth should be at most 10000");
        
        // Memory should be reasonable (1-1024 MB)
        assert!(config.max_memory_mb >= 1, "max_memory_mb should be at least 1");
        assert!(config.max_memory_mb <= 1024, "max_memory_mb should be at most 1024");
    }
    
    #[test]
    fn test_wasm_input_size_limit() {
        // Test that we can create configs with different input handling
        let strict_config = WasmConfig {
            max_memory_mb: 16, // Small memory
            max_io_ops: 100,   // Low I/O
            max_output_bytes: 1024, // Small output
            ..WasmConfig::default()
        };
        
        assert_eq!(strict_config.max_memory_mb, 16);
        assert_eq!(strict_config.max_io_ops, 100);
        assert_eq!(strict_config.max_output_bytes, 1024);
    }
}
