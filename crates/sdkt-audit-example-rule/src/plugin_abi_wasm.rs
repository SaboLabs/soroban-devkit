//! Extism PDK exports for the M19 WASM plugin architecture.
//!
//! When built for a WASM target (`wasm32-unknown-unknown`), this module exports
//! the required ABI functions so the `sdkt-audit` host can load it.
//!
//! Deliberately self-contained: we do NOT import from `sdkt-audit` to keep
//! the dependency graph free of host-side crates (syn, sdkt-wasm, etc.) that
//! do not compile cleanly for `wasm32-unknown-unknown`.

use extism_pdk::*;
use serde::{Deserialize, Serialize};

// Mirror of host-side ABI constants (kept local to avoid importing sdkt-audit).
const SDKT_AUDIT_WASM_ABI_MAJOR: i64 = 1;
const SEVERITY_INFO: u32 = 2;

// ── Wire types (must match host's WasmCheckInput / WasmFinding) ────────────

/// Minimal projection of `FnScan` — only what the rule needs.
#[derive(Deserialize)]
pub struct FnScanInput {
    pub fn_name: String,
}

#[derive(Deserialize)]
pub struct WasmCheckInput {
    pub scans: Vec<FnScanInput>,
}

#[derive(Serialize)]
pub struct WasmFinding {
    pub rule_id: String,
    pub severity: u32,
    pub message: String,
    pub location: Option<String>,
}

// ── ABI exports ─────────────────────────────────────────────────────────────

#[plugin_fn]
pub fn sdkt_plugin_abi_version() -> FnResult<i64> {
    Ok(SDKT_AUDIT_WASM_ABI_MAJOR)
}

#[plugin_fn]
pub fn sdkt_plugin_id() -> FnResult<String> {
    Ok("EXAMPLE-001".to_string())
}

#[plugin_fn]
pub fn sdkt_plugin_severity() -> FnResult<i64> {
    Ok(SEVERITY_INFO as i64)
}

#[plugin_fn]
pub fn sdkt_plugin_description() -> FnResult<String> {
    Ok("Example plugin rule that flags any function starting with `sdkt_example_trigger`".into())
}

#[plugin_fn]
pub fn sdkt_plugin_check(input_json: String) -> FnResult<String> {
    let input: WasmCheckInput = serde_json::from_str(&input_json)?;

    let findings: Vec<WasmFinding> = input
        .scans
        .iter()
        .filter(|s| s.fn_name.contains("sdkt_example_trigger"))
        .map(|s| WasmFinding {
            rule_id: "EXAMPLE-001".to_string(),
            severity: SEVERITY_INFO,
            message: format!("Example rule matched trigger function `{}`", s.fn_name),
            location: Some(s.fn_name.clone()),
        })
        .collect();

    Ok(serde_json::to_string(&findings)?)
}
