//! `sdkt-audit` — static security analysis for Soroban contracts (Gap C).
//!
//! Offline, source-based heuristics for Soroban-specific logic/security bugs
//! the Rust compiler does not catch (missing auth, unauthenticated cross-contract
//! calls, unguarded initialize, suspicious move-after-use). No networking.
//!
//! ## Plugin authoring
//!
//! Implement [`AuditRule`], then register your rule with [`register_rule`] (or the
//! [`register_rule!`](crate::register_rule) macro). Built-in rules register
//! through the same [`RuleRegistry`]. See `docs/plugin-authoring.md`.

pub mod audit;
pub mod error;
pub mod plugin_abi;
pub mod plugin_store;
pub mod registry;
pub mod rules;
pub mod types;

#[cfg(feature = "plugins")]
pub mod plugin_loader;

#[cfg(feature = "wasm-plugins")]
pub mod plugin_abi_wasm;
#[cfg(feature = "wasm-plugins")]
pub mod plugin_loader_wasm;

pub use audit::{
    all_rules, audit_source, audit_source_with, audit_source_with_spec, scan_all_functions,
    scan_all_functions_str, AuditContext, AuditRule, FnScan,
};
pub use error::AuditError;
pub use plugin_store::{
    install_bundle, pack_bundle, verify_bundle, BundleVerification, InstallOpts, PluginMeta,
    StoreError,
};
pub use registry::{
    register_builtin_rules, register_rule, run_registered, BoxedRule, RuleRegistry,
};
pub use rules::{Auth001, Auth002, Auth003, Move001};
pub use types::{AuditReport, AuditSummary, Finding, Severity};

#[cfg(feature = "plugins")]
pub use plugin_loader::{load_and_register, PluginLoadError, PluginRule};

#[cfg(feature = "wasm-plugins")]
pub use plugin_loader_wasm::{load_and_register_wasm, WasmPluginLoadError, WasmPluginRule};

/// Register a rule into the process-wide [`RuleRegistry`].
///
/// # Example
/// ```ignore
/// sdkt_audit::register_rule!(MyRule);
/// ```
#[macro_export]
macro_rules! register_rule {
    ($rule:expr) => {
        $crate::registry::register_rule(::std::boxed::Box::new($rule))
    };
}
