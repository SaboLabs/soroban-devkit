//! WebAssembly Plugin ABI (M19, Phase C).
//!
//! Unlike the M18 native C-ABI (`plugin_abi.rs`), which relies on raw memory
//! pointers and fixed-size buffers, the WASM ABI relies on a higher-level
//! JSON-over-memory boundary (implementation details of the memory exchange
//! depend on the chosen runtime, e.g., Extism or manual Wasmtime exports).
//!
//! # Required Guest Exports
//! WASM plugins must export the following functions to the host:
//!
//! - `sdkt_plugin_abi_version() -> i32`
//!   Returns the major ABI version. Must match `SDKT_AUDIT_WASM_ABI_MAJOR`.
//!
//! - `sdkt_plugin_id() -> String` (or string-passing equivalent)
//!   Returns the stable rule id, e.g., "EXAMPLE-001".
//!
//! - `sdkt_plugin_severity() -> i32`
//!   Returns the severity level (0 = Critical, 1 = Warning, 2 = Info).
//!
//! - `sdkt_plugin_description() -> String` (or equivalent)
//!   Returns a human-readable description of the rule.
//!
//! - `sdkt_plugin_check(source_json: String) -> String` (or equivalent)
//!   Receives the audit context/source as a JSON string and returns a JSON
//!   array of findings.

/// Plugin WASM ABI major version. BREAKING changes bump this.
pub const SDKT_AUDIT_WASM_ABI_MAJOR: u32 = 1;
/// Plugin WASM ABI minor version. Additive/backward-compatible changes bump this.
pub const SDKT_AUDIT_WASM_ABI_MINOR: u32 = 0;

/// Severity encoding for the WASM boundary (matches C-ABI).
pub const SEVERITY_CRITICAL: u32 = 0;
/// See [`SEVERITY_CRITICAL`].
pub const SEVERITY_WARNING: u32 = 1;
/// See [`SEVERITY_CRITICAL`].
pub const SEVERITY_INFO: u32 = 2;
