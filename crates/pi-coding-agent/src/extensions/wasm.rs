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
    /// Compile a WASM module from bytes.
    #[cfg(feature = "wasm")]
    pub fn compile(bytes: &[u8], config: WasmConfig) -> Result<Self> {
        let mut engine_config = wasmtime::Config::new();
        engine_config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Enable);
        
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
        
        // Create WASI context
        let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
            .inherit_stdio()
            .build();
        
        // Create store with fuel limit for timeout
        let mut store = Store::new(&self.engine, wasi_ctx);
        store.add_fuel(self.config.timeout_secs * 1_000_000)
            .context("Failed to add fuel")?;
        
        // Create linker and add WASI
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
            if output_bytes.len() > 65536 {
                return Err(anyhow::anyhow!("Output too large"));
            }
        }
        
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
}
