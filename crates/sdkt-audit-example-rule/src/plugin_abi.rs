//! C-ABI exports for the example plugin (M18, Phase B dynamic loading).
//!
//! This module is compiled only when the `plugins` feature is enabled, turning
//! the example crate into a loadable shared library (`.so`/`.dylib`/`.dll`).
//! It reuses the already-validated `EXAMPLE-001` rule logic through the public
//! `sdkt_audit` API — no rule duplication.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;

use sdkt_audit::plugin_abi::{
    abi_version_pack, SdktAuditFindingC, SdktAuditReportC, MAX_FINDINGS, SEVERITY_INFO,
};
use sdkt_audit::{AuditReport, AuditRule};

use crate::ExampleRule;

/// Source to analyze, set once via [`sdkt_plugin_init`].
static SOURCE: OnceLock<String> = OnceLock::new();

/// Plugin ABI version (must match the host's `SDKT_AUDIT_ABI_MAJOR`).
///
/// # Safety
/// Standard C-ABI export; returns a packed `u32`. No pointers touched.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_abi_version() -> u32 {
    abi_version_pack()
}

/// Rule id `EXAMPLE-001`.
///
/// # Safety
/// Returns a static C string with static lifetime — valid for the program.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_id() -> *const c_char {
    static ID: &[u8] = b"EXAMPLE-001\0";
    ID.as_ptr() as *const c_char
}

/// Severity: info.
///
/// # Safety
/// Returns a constant `u32`.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_severity() -> u32 {
    SEVERITY_INFO
}

/// Human-readable description.
///
/// # Safety
/// Returns a static C string with static lifetime.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_description() -> *const c_char {
    static DESC: &[u8] = b"Example rule: detects functions named sdkt_example_trigger\0";
    DESC.as_ptr() as *const c_char
}

/// Cache the contract source to be analyzed.
///
/// # Safety
/// `src` must be a valid NUL-terminated C string for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_init(src: *const c_char) -> i32 {
    if src.is_null() {
        return 1;
    }
    match CStr::from_ptr(src).to_str() {
        Ok(s) => {
            if SOURCE.set(s.to_string()).is_ok() {
                0
            } else {
                1
            }
        }
        Err(_) => 1,
    }
}

/// Run the example rule over the cached source and write findings into `report`.
///
/// # Safety
/// `report` must point to a valid, mutable `SdktAuditReportC`.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_check(report: *mut SdktAuditReportC) -> i32 {
    if report.is_null() {
        return 1;
    }
    let src = match SOURCE.get() {
        Some(s) => s,
        None => return 2,
    };
    // Run ONLY this plugin's own rule (via the in-crate `ExampleRule`), never the
    // global registry, to avoid re-entrant recursion when the host invokes this
    // symbol during an `audit_source_with` run.
    let scans = match sdkt_audit::scan_all_functions_str(src) {
        Some(s) => s,
        None => return 3,
    };
    let ctx = sdkt_audit::AuditContext { spec: None };
    let mut local = AuditReport::default();
    ExampleRule.check(&scans, &ctx, &mut local);

    let out = &mut *report;
    out.count = 0;
    for f in local.findings {
        if out.count >= MAX_FINDINGS {
            break;
        }
        let rule_id = match CString::new(f.rule_id) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let message = match CString::new(f.message) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let location = f
            .location
            .as_ref()
            .and_then(|l| CString::new(l.clone()).ok());
        let slot: &mut SdktAuditFindingC = &mut out.findings[out.count];
        slot.rule_id = rule_id.into_raw() as *const c_char;
        slot.severity = match f.severity {
            sdkt_audit::Severity::Critical => 0,
            sdkt_audit::Severity::Info => 2,
            sdkt_audit::Severity::Warning => 1,
        };
        slot.message = message.into_raw() as *const c_char;
        slot.location = location
            .map(|c| c.into_raw() as *const c_char)
            .unwrap_or(std::ptr::null());
        out.count += 1;
    }
    0
}

/// Optional cleanup (no-op: `SOURCE` is dropped at process exit).
///
/// # Safety
/// Standard C-ABI export.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_free() {}
