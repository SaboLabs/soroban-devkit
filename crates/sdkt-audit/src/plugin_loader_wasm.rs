//! WebAssembly dynamic plugin loading for `sdkt-audit` (M19, Phase C).
//!
//! # Runtime: Extism v1.x (over Wasmtime)
//!
//! Extism was chosen over raw Wasmtime for three reasons:
//! 1. String/memory passing is handled by the Extism PDK — plugin authors
//!    write idiomatic Rust (or Go, TypeScript, etc.) with no manual memory
//!    management at the FFI boundary, which eliminates the F7-class of leaks
//!    that affect the native C-ABI.
//! 2. Capability model is deny-by-default: filesystem, network, and env access
//!    are all off unless explicitly granted by the host. We never grant any.
//! 3. Multi-language PDK support is required for the roadmap's plugin
//!    marketplace (Phase D), where third-party plugins in Go, TS, or Python
//!    must interoperate without a bespoke serialization layer.
//!
//! # Security model
//!
//! - WASM plugins run in an Extism/Wasmtime sandbox.
//! - No filesystem, network, or host-process access is granted.
//! - No `unsafe` Rust in this module.
//! - Findings returned by the plugin are strictly clamped and schema-validated
//!   before being pushed to the report.
//!
//! # ABI
//!
//! See [`crate::plugin_abi_wasm`] for the full contract. Briefly:
//!
//! | Export | Input | Output |
//! |--------|-------|--------|
//! | `sdkt_plugin_abi_version` | — | i64 major version |
//! | `sdkt_plugin_id` | — | UTF-8 string |
//! | `sdkt_plugin_severity` | — | i64 severity code |
//! | `sdkt_plugin_description` | — | UTF-8 string |
//! | `sdkt_plugin_check` | JSON-encoded [`WasmCheckInput`] | JSON-encoded `Vec<`[`WasmFinding`]`>` |

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::audit::{AuditContext, AuditRule, FnScan};
use crate::plugin_abi_wasm::{
    SDKT_AUDIT_WASM_ABI_MAJOR, SEVERITY_CRITICAL, SEVERITY_INFO, SEVERITY_WARNING,
};
use crate::types::{AuditReport, Finding, Severity};

// ── MAX_FINDINGS mirrors the native ABI cap. Never iterate past this. ─────────
const MAX_FINDINGS: usize = 64;

// ── Wire-format types (WASM ↔ host) ─────────────────────────────────────────

/// What the host serialises and sends into `sdkt_plugin_check`.
#[derive(Debug, Serialize)]
pub struct WasmCheckInput<'a> {
    /// Pre-scanned functions from the audit pipeline.
    pub scans: &'a [FnScan],
}

/// A single finding as returned by the WASM plugin over JSON.
#[derive(Debug, Deserialize)]
pub struct WasmFinding {
    pub rule_id: String,
    pub severity: u32,
    pub message: String,
    pub location: Option<String>,
}

impl WasmFinding {
    fn severity_enum(&self) -> Severity {
        match self.severity {
            s if s == SEVERITY_CRITICAL => Severity::Critical,
            s if s == SEVERITY_WARNING => Severity::Warning,
            s if s == SEVERITY_INFO => Severity::Info,
            // Unknown values fall back to Info (least alarming safe default).
            _ => Severity::Info,
        }
    }
}

// ── Error type ───────────────────────────────────────────────────────────────

/// Errors that can occur while loading or running a WASM plugin.
#[derive(Debug)]
pub enum WasmPluginLoadError {
    /// I/O error reading the plugin path.
    Io(std::io::Error),
    /// Runtime failed to load or compile the WASM module.
    Runtime(String),
    /// A required ABI symbol / export was missing from the plugin.
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

impl From<std::io::Error> for WasmPluginLoadError {
    fn from(e: std::io::Error) -> Self {
        WasmPluginLoadError::Io(e)
    }
}

// ── Plugin handle ─────────────────────────────────────────────────────────────

/// A loaded WASM plugin wrapped as an [`AuditRule`].
///
/// The inner [`extism::Plugin`] is wrapped in a `Mutex` because `AuditRule`
/// requires `Send + Sync`, and Extism plugins hold raw pointers to the
/// Wasmtime store which are not `Sync` on their own.
pub struct WasmPluginRule {
    id: String,
    severity: Severity,
    description: String,
    // The compiled + instantiated plugin, ready to call.
    plugin: Mutex<extism::Plugin>,
}

impl WasmPluginRule {
    /// Load and validate a WASM plugin from `path`.
    ///
    /// Steps:
    /// 1. Read WASM bytes from disk.
    /// 2. Build a capability-free Extism manifest (no fs, no net).
    /// 3. Instantiate the plugin.
    /// 4. Verify `sdkt_plugin_abi_version` matches [`SDKT_AUDIT_WASM_ABI_MAJOR`].
    /// 5. Read `id`, `severity`, `description` from named exports.
    pub fn load(path: &Path, _source: &str) -> Result<Self, WasmPluginLoadError> {
        // 1. Read bytes.
        let wasm_bytes = std::fs::read(path)?;

        // 2. Build a capability-free manifest. Extism's default denies all
        //    host capabilities (filesystem, network, environment variables),
        //    so we just need to supply the module bytes.
        let manifest = extism::Manifest::new([extism::Wasm::data(wasm_bytes)]);

        // 3. Instantiate. `with_wasi = true` is required because the plugin is compiled
        //    for `wasm32-wasip1` (to satisfy standard library dependencies like `getrandom`),
        //    but Extism will restrict WASI access according to the empty manifest
        //    (no filesystem, network, or env vars).
        let mut plugin = extism::Plugin::new(&manifest, [], true)
            .map_err(|e| WasmPluginLoadError::Runtime(e.to_string()))?;

        // 4. Verify ABI version.
        let abi_version_raw: i64 = plugin
            .call::<(), i64>("sdkt_plugin_abi_version", ())
            .map_err(|_| WasmPluginLoadError::SymbolMissing("sdkt_plugin_abi_version".into()))?;

        let plugin_major = abi_version_raw.unsigned_abs() as u32;
        if plugin_major != SDKT_AUDIT_WASM_ABI_MAJOR {
            return Err(WasmPluginLoadError::AbiMismatch {
                plugin_major,
                host_major: SDKT_AUDIT_WASM_ABI_MAJOR,
            });
        }

        // 5. Read metadata exports.
        let id: String = plugin
            .call::<(), String>("sdkt_plugin_id", ())
            .map_err(|_| WasmPluginLoadError::SymbolMissing("sdkt_plugin_id".into()))?;

        let severity_raw: i64 = plugin
            .call::<(), i64>("sdkt_plugin_severity", ())
            .map_err(|_| WasmPluginLoadError::SymbolMissing("sdkt_plugin_severity".into()))?;

        let severity = match severity_raw as u32 {
            s if s == SEVERITY_CRITICAL => Severity::Critical,
            s if s == SEVERITY_WARNING => Severity::Warning,
            // Default unknown severity to Info (safe fallback).
            _ => Severity::Info,
        };

        let description: String = plugin
            .call::<(), String>("sdkt_plugin_description", ())
            .map_err(|_| WasmPluginLoadError::SymbolMissing("sdkt_plugin_description".into()))?;

        Ok(WasmPluginRule {
            id,
            severity,
            description,
            plugin: Mutex::new(plugin),
        })
    }
}

impl AuditRule for WasmPluginRule {
    fn id(&self) -> &'static str {
        // Plugins are loaded exactly once per process execution and live until
        // the CLI exits (plugin lifetime == process lifetime). Leaking a few
        // bytes per rule is bounded, safe, and avoids adding generic lifetime
        // parameters to the `AuditRule` trait.
        Box::leak(self.id.clone().into_boxed_str())
    }

    fn severity(&self) -> Severity {
        self.severity
    }

    fn description(&self) -> &'static str {
        // Same process-lifetime reasoning as `id()`.
        Box::leak(self.description.clone().into_boxed_str())
    }

    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        // Build the JSON input payload.
        let input = WasmCheckInput { scans };
        let input_json = match serde_json::to_string(&input) {
            Ok(j) => j,
            Err(e) => {
                // This is an internal failure (serialising our own types), not a
                // plugin failure. Emit one finding so the operator knows.
                report.findings.push(Finding {
                    rule_id: self.id.clone(),
                    severity: Severity::Warning,
                    message: format!("internal: failed to serialise check input: {e}"),
                    location: None,
                });
                return;
            }
        };

        // Call the plugin. Lock is always released before we push findings.
        let output_json = {
            let mut guard = match self.plugin.lock() {
                Ok(g) => g,
                Err(_) => {
                    report.findings.push(Finding {
                        rule_id: self.id.clone(),
                        severity: Severity::Warning,
                        message: "internal: wasm plugin mutex poisoned".into(),
                        location: None,
                    });
                    return;
                }
            };
            match guard.call::<String, String>("sdkt_plugin_check", input_json) {
                Ok(s) => s,
                Err(e) => {
                    report.findings.push(Finding {
                        rule_id: self.id.clone(),
                        severity: Severity::Warning,
                        message: format!("wasm plugin trap during check: {e}"),
                        location: None,
                    });
                    return;
                }
            }
        };

        // Parse and validate plugin output. Never trust plugin-supplied data.
        let raw_findings: Vec<WasmFinding> = match serde_json::from_str(&output_json) {
            Ok(v) => v,
            Err(e) => {
                report.findings.push(Finding {
                    rule_id: self.id.clone(),
                    severity: Severity::Warning,
                    message: format!("wasm plugin returned malformed JSON: {e}"),
                    location: None,
                });
                return;
            }
        };

        // Clamp: never process more than MAX_FINDINGS, regardless of what the
        // plugin reports. Mirrors the F1 fix from the M18 native ABI audit.
        let safe_count = raw_findings.len().min(MAX_FINDINGS);
        for wf in &raw_findings[..safe_count] {
            report.findings.push(Finding {
                rule_id: wf.rule_id.clone(),
                severity: wf.severity_enum(),
                message: wf.message.clone(),
                location: wf.location.clone(),
            });
        }
    }
}

/// Load a WASM plugin from `path` and register it into the process-wide
/// [`RuleRegistry`](crate::registry::RuleRegistry) after validating it.
/// Returns the resolved plugin path on success (for diagnostics).
pub fn load_and_register_wasm(path: &Path, source: &str) -> Result<PathBuf, WasmPluginLoadError> {
    let rule = WasmPluginRule::load(path, source)?;
    crate::registry::register_rule(Box::new(rule));
    Ok(path.to_path_buf())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn wasm_finding_severity_mapping() {
        let f = WasmFinding {
            rule_id: "X".into(),
            severity: SEVERITY_CRITICAL,
            message: "m".into(),
            location: None,
        };
        assert_eq!(f.severity_enum(), Severity::Critical);

        let f = WasmFinding {
            severity: SEVERITY_WARNING,
            ..f
        };
        assert_eq!(f.severity_enum(), Severity::Warning);

        let f = WasmFinding {
            severity: SEVERITY_INFO,
            ..f
        };
        assert_eq!(f.severity_enum(), Severity::Info);

        // Unknown code → Info (safe fallback).
        let f = WasmFinding { severity: 99, ..f };
        assert_eq!(f.severity_enum(), Severity::Info);
    }

    #[test]
    fn load_missing_file_returns_io_error() {
        let err = WasmPluginRule::load(Path::new("/nonexistent/plugin.wasm"), "");
        assert!(matches!(err, Err(WasmPluginLoadError::Io(_))));
    }

    #[test]
    fn load_invalid_wasm_bytes_returns_runtime_error() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"this is not wasm").unwrap();
        let err = WasmPluginRule::load(tmp.path(), "");
        assert!(matches!(err, Err(WasmPluginLoadError::Runtime(_))));
    }
}
