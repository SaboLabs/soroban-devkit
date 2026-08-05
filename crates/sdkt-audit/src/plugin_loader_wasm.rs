//! WebAssembly dynamic plugin loading for `sdkt-audit` (M19, Phase C).
//!
//! WASM plugins are sandboxed, platform-independent alternatives to native
//! `.so` plugins. This module is the scaffolding layer; runtime integration
//! (Extism or Wasmtime) is deferred to a future stage.

use std::path::{Path, PathBuf};

use crate::audit::{AuditContext, AuditRule, FnScan};
use crate::types::{AuditReport, Severity};

/// Errors that can occur while loading or running a WASM plugin.
#[derive(Debug)]
pub enum WasmPluginLoadError {
    /// I/O error reading the plugin path.
    Io(std::io::Error),
    /// Runtime failed to load or compile the WASM module.
    Runtime(String),
    /// A required ABI symbol was missing from the plugin exports.
    SymbolMissing(String),
    /// Plugin ABI major version does not match the host.
    AbiMismatch {
        /// Plugin's reported major version.
        plugin_major: u32,
        /// Host's expected major version.
        host_major: u32,
    },
    /// Plugin execution panicked, trapped, or exhausted resources.
    Trap(String),
}

impl std::fmt::Display for WasmPluginLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WasmPluginLoadError::Io(e) => write!(f, "io error: {}", e),
            WasmPluginLoadError::Runtime(e) => write!(f, "wasm runtime error: {}", e),
            WasmPluginLoadError::SymbolMissing(s) => write!(f, "missing wasm export: {}", s),
            WasmPluginLoadError::AbiMismatch {
                plugin_major,
                host_major,
            } => write!(
                f,
                "wasm plugin ABI mismatch (plugin v{}.x, host v{}.x)",
                plugin_major, host_major
            ),
            WasmPluginLoadError::Trap(e) => write!(f, "wasm trap/panic: {}", e),
        }
    }
}

impl std::error::Error for WasmPluginLoadError {}

/// A loaded WASM plugin wrapped as an [`AuditRule`].
pub struct WasmPluginRule {
    // TODO: Hold the compiled module/instance/runtime state here once
    // Extism or Wasmtime is integrated.
    id: String,
    severity: Severity,
    description: String,
    #[allow(dead_code)]
    source: String,
}

impl WasmPluginRule {
    /// Load a WASM plugin from `path`, initializing it with `source` (the contract
    /// source to be analyzed).
    pub fn load(_path: &Path, _source: &str) -> Result<Self, WasmPluginLoadError> {
        // TODO: Instantiate WASM runtime, compile module, check ABI version,
        // read id/severity/description from exports, and initialize plugin state.

        Err(WasmPluginLoadError::Runtime(
            "WASM runtime integration is not yet implemented (M19 Stage 1 stub)".to_string(),
        ))
    }
}

impl AuditRule for WasmPluginRule {
    fn id(&self) -> &'static str {
        // Safe to leak during this stub phase. In Stage 2, plugins are loaded exactly once
        // per process execution and live until the CLI exits. A few bytes of leaked memory
        // per rule is bounded, safe, and avoids complex lifetime annotations on AuditRule.
        Box::leak(self.id.clone().into_boxed_str())
    }

    fn severity(&self) -> Severity {
        self.severity
    }

    fn description(&self) -> &'static str {
        // Safe to leak for the same process-lifetime reasons as `id()`.
        Box::leak(self.description.clone().into_boxed_str())
    }

    fn check(&self, _scans: &[FnScan], _ctx: &AuditContext, _report: &mut AuditReport) {
        // TODO: Serialize scans/ctx to JSON, call WASM `sdkt_plugin_check` export,
        // deserialize JSON response back into `report`.
    }
}

/// Load a WASM plugin from `path` and register it into the process-wide
/// [`RuleRegistry`](crate::registry::RuleRegistry) after initializing it with
/// `source`. Returns the plugin path on success (for diagnostics).
pub fn load_and_register_wasm(path: &Path, source: &str) -> Result<PathBuf, WasmPluginLoadError> {
    let rule = WasmPluginRule::load(path, source)?;
    crate::registry::register_rule(Box::new(rule));
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_stub_error_until_runtime_integrated() {
        let err = WasmPluginRule::load(Path::new("/any/plugin.wasm"), "fn x() {}");
        assert!(
            matches!(err, Err(WasmPluginLoadError::Runtime(_))),
            "stub must return Runtime error until M19 Stage 2 integrates a WASM runtime"
        );
    }

    #[test]
    fn wasm_load_error_display() {
        let e = WasmPluginLoadError::Runtime("test".to_string());
        assert!(e.to_string().contains("wasm runtime error"));

        let e = WasmPluginLoadError::SymbolMissing("sdkt_plugin_check".to_string());
        assert!(e.to_string().contains("missing wasm export"));

        let e = WasmPluginLoadError::AbiMismatch {
            plugin_major: 2,
            host_major: 1,
        };
        assert!(e.to_string().contains("ABI mismatch"));

        let e = WasmPluginLoadError::Trap("oom".to_string());
        assert!(e.to_string().contains("wasm trap"));

        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = WasmPluginLoadError::Io(io_err);
        assert!(e.to_string().contains("io error"));
    }
}
