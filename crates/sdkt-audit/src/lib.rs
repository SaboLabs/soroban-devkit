//! `sdkt-audit` — static security analysis for Soroban contracts (Gap C).
//!
//! Offline, source-based heuristics for Soroban-specific logic/security bugs
//! the Rust compiler does not catch (missing auth, unauthenticated cross-contract
//! calls, unguarded initialize, suspicious move-after-use). No networking.

pub mod audit;
pub mod error;
pub mod rules;
pub mod types;

pub use audit::{
    all_rules, audit_source, audit_source_with, audit_source_with_spec, scan_all_functions,
    AuditContext, AuditRule, FnScan,
};
pub use error::AuditError;
pub use types::{AuditReport, AuditSummary, Finding, Severity};
