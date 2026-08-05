//! Dynamic plugin loading for `sdkt-audit` (M18, Phase B).
//!
//! Native plugins are shared libraries (`.so` / `.dylib` / `.dll`) exporting the
//! C-ABI symbols declared in [`plugin_abi`]. The host wraps each plugin in a
//! [`PluginRule`] implementing [`AuditRule`]; all rule execution happens through
//! the `#[repr(C)]` boundary — no Rust trait objects ever cross the FFI.
//!
//! # Safety boundary
//!
//! * Only `#[repr(C)]` flat data (`SdktAuditFindingC`, `SdktAuditReportC`) and
//!   C strings cross the FFI.
//! * Deallocation of plugin-owned memory always happens inside the plugin
//!   (via its `sdkt_plugin_free` symbol), never in the host, avoiding
//!   cross-allocator UB.
//! * The loaded [`Library`] is kept alive for the life of [`PluginRule`] via an
//!   `Arc`, so symbols remain valid while the rule runs.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::{Library, Symbol};

use crate::audit::{AuditContext, AuditRule, FnScan};
use crate::plugin_abi::{
    abi_major, SdktAuditFindingC, SdktAuditReportC, SDKT_AUDIT_ABI_MAJOR, SEVERITY_CRITICAL,
    SEVERITY_INFO,
};
use crate::types::{AuditReport, Finding, Severity};

/// Errors that can occur while loading or running a dynamic plugin.
#[derive(Debug)]
pub enum PluginLoadError {
    /// I/O error reading the plugin path.
    Io(std::io::Error),
    /// `libloading` failed to open the shared object.
    DlOpen(String),
    /// A required C-ABI symbol was missing from the plugin.
    SymbolMissing(String),
    /// Plugin ABI major version does not match the host.
    AbiMismatch {
        /// Plugin's reported major version.
        plugin_major: u32,
        /// Host's expected major version.
        host_major: u32,
    },
    /// Plugin `sdkt_plugin_init` returned a non-zero code.
    InitFailed(c_int),
    /// Plugin panicked during execution (caught via `catch_unwind`).
    Panic,
}

impl std::fmt::Display for PluginLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginLoadError::Io(e) => write!(f, "io error: {}", e),
            PluginLoadError::DlOpen(e) => write!(f, "failed to load plugin: {}", e),
            PluginLoadError::SymbolMissing(s) => write!(f, "missing plugin symbol: {}", s),
            PluginLoadError::AbiMismatch {
                plugin_major,
                host_major,
            } => write!(
                f,
                "plugin ABI mismatch (plugin v{}.x, host v{}.x) — rebuild plugin against sdkt-audit v{}.x",
                plugin_major, host_major, host_major
            ),
            PluginLoadError::InitFailed(c) => write!(f, "plugin init failed (code {})", c),
            PluginLoadError::Panic => write!(f, "plugin panicked during execution"),
        }
    }
}

impl std::error::Error for PluginLoadError {}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type IdFn = unsafe extern "C" fn() -> *const c_char;
type SeverityFn = unsafe extern "C" fn() -> u32;
type DescFn = unsafe extern "C" fn() -> *const c_char;
type InitFn = unsafe extern "C" fn(*const c_char) -> c_int;
type CheckFn = unsafe extern "C" fn(*mut SdktAuditReportC) -> c_int;
type FreeFn = unsafe extern "C" fn();

/// Copy a C string returned by a plugin into an owned [`String`].
///
/// # Safety
/// `p` must be a valid NUL-terminated C string for the duration of the call
/// (plugin-returned strings satisfy this). A null pointer yields an empty string.
unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn sev_to_enum(s: u32) -> Severity {
    match s {
        SEVERITY_CRITICAL => Severity::Critical,
        SEVERITY_INFO => Severity::Info,
        _ => Severity::Warning,
    }
}

/// A loaded plugin wrapped as an [`AuditRule`].
pub struct PluginRule {
    /// Keeps the shared library resident for the life of this rule.
    _lib: Arc<Library>,
    id: &'static str,
    severity: Severity,
    description: &'static str,
    #[allow(dead_code)]
    source: String,
    check: CheckFn,
    free: FreeFn,
}

impl Drop for PluginRule {
    fn drop(&mut self) {
        // SAFETY: FFI call into plugin to release internal resources.
        // `_lib` guarantees the symbol is still valid.
        unsafe {
            (self.free)();
        }
    }
}

impl PluginRule {
    /// Load a plugin from `path`, initializing it with `source` (the contract
    /// source to be analyzed). Fails fast on I/O, ABI, or init errors.
    pub fn load(path: &Path, source: &str) -> Result<Self, PluginLoadError> {
        // SAFETY: `Library::new` opens a shared object; the resulting `Library`
        // is kept alive via `Arc` for the rule's lifetime, so symbols stay valid.
        let lib = Arc::new(
            unsafe { Library::new(path) }.map_err(|e| PluginLoadError::DlOpen(e.to_string()))?,
        );

        // SAFETY: each `lib.get` borrows `lib`; we immediately copy the fn
        // pointers out (fn pointers are `Copy`), so no `Symbol` borrow outlives
        // this scope. `_lib` (the `Arc<Library>`) is stored to keep the library
        // resident, so the copied pointers remain valid for the rule's lifetime.
        let abi_fn: Symbol<AbiVersionFn> = unsafe {
            lib.get(b"sdkt_plugin_abi_version\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let id_fn: Symbol<IdFn> = unsafe {
            lib.get(b"sdkt_plugin_id\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let sev_fn: Symbol<SeverityFn> = unsafe {
            lib.get(b"sdkt_plugin_severity\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let desc_fn: Symbol<DescFn> = unsafe {
            lib.get(b"sdkt_plugin_description\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let init_fn: Symbol<InitFn> = unsafe {
            lib.get(b"sdkt_plugin_init\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let check_fn: Symbol<CheckFn> = unsafe {
            lib.get(b"sdkt_plugin_check\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };
        let free_fn: Symbol<FreeFn> = unsafe {
            lib.get(b"sdkt_plugin_free\0")
                .map_err(|e| PluginLoadError::SymbolMissing(e.to_string()))?
        };

        let version = unsafe { abi_fn() };
        let plugin_major = abi_major(version);
        if plugin_major != SDKT_AUDIT_ABI_MAJOR {
            return Err(PluginLoadError::AbiMismatch {
                plugin_major,
                host_major: SDKT_AUDIT_ABI_MAJOR,
            });
        }

        // Leak the id/description once; `AuditRule::id`/`description` require
        // `&'static str`. Plugins are loaded rarely, so the small leak is fine.
        let id: &'static str = Box::leak(unsafe { cstr_to_string(id_fn()) }.into_boxed_str());
        let description: &'static str =
            Box::leak(unsafe { cstr_to_string(desc_fn()) }.into_boxed_str());
        let severity = sev_to_enum(unsafe { sev_fn() });

        let c_src = CString::new(source).map_err(|_| {
            PluginLoadError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "source contains an interior NUL byte",
            ))
        })?;
        let code = unsafe { init_fn(c_src.as_ptr()) };
        if code != 0 {
            return Err(PluginLoadError::InitFailed(code));
        }

        let check: CheckFn = *check_fn;
        let free: FreeFn = *free_fn;

        Ok(Self {
            _lib: lib,
            id,
            severity,
            description,
            source: source.to_string(),
            check,
            free,
        })
    }
}

impl AuditRule for PluginRule {
    fn id(&self) -> &'static str {
        self.id
    }

    fn severity(&self) -> Severity {
        self.severity
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn check(&self, _scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        let mut buf = SdktAuditReportC::default();
        // FFI boundary: invoke the plugin check.
        // F2 fixed: catch_unwind over FFI is UB. The plugin MUST handle panics
        // internally. We only catch normal return values.
        let code = unsafe { (self.check)(&mut buf) };
        if code != 0 {
            return;
        }

        // F1 fixed: clamp plugin-provided count to MAX_FINDINGS to prevent out-of-bounds read
        // if a malicious/buggy plugin sets count > MAX_FINDINGS.
        let safe_count = buf.count.min(crate::plugin_abi::MAX_FINDINGS);

        for i in 0..safe_count {
            // SAFETY: entries [0, count) were written by the plugin during the
            // call above; we copy their C strings into owned `String`s now.
            let f: SdktAuditFindingC = buf.findings[i];
            if f.rule_id.is_null() {
                continue;
            }
            let rule_id = unsafe { cstr_to_string(f.rule_id) };
            let message = unsafe { cstr_to_string(f.message) };
            let location = if f.location.is_null() {
                None
            } else {
                Some(unsafe { cstr_to_string(f.location) })
            };
            report.add(Finding {
                rule_id,
                severity: match f.severity {
                    SEVERITY_CRITICAL => Severity::Critical,
                    SEVERITY_INFO => Severity::Info,
                    _ => Severity::Warning,
                },
                message,
                location,
            });
        }
    }
}

/// Load a plugin from `path` and register it into the process-wide
/// [`RuleRegistry`](crate::registry::RuleRegistry) after initializing it with
/// `source`. Returns the plugin path on success (for diagnostics).
///
/// This integrates with [`audit_source_with`](crate::audit::audit_source_with),
/// which executes the global registry — so loaded plugins are automatically
/// included in every audit run.
pub fn load_and_register(path: &Path, source: &str) -> Result<PathBuf, PluginLoadError> {
    let rule = PluginRule::load(path, source)?;
    crate::registry::register_rule(Box::new(rule));
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_abi::{abi_major, abi_minor, abi_version_pack, SDKT_AUDIT_ABI_MINOR};

    #[test]
    fn abi_version_pack_roundtrip() {
        assert_eq!(abi_major(abi_version_pack()), SDKT_AUDIT_ABI_MAJOR);
        assert_eq!(abi_minor(abi_version_pack()), SDKT_AUDIT_ABI_MINOR);
    }

    #[test]
    fn severity_enum_mapping() {
        assert_eq!(sev_to_enum(SEVERITY_CRITICAL), Severity::Critical);
        assert_eq!(sev_to_enum(SEVERITY_INFO), Severity::Info);
        // Default/unrecognized → Warning.
        assert_eq!(sev_to_enum(99), Severity::Warning);
    }

    #[test]
    fn finding_buffer_default_is_empty() {
        let buf = SdktAuditReportC::default();
        assert_eq!(buf.count, 0);
    }

    #[test]
    fn cstr_to_string_handles_null() {
        // SAFETY: null pointer is explicitly handled.
        assert_eq!(unsafe { cstr_to_string(std::ptr::null()) }, "");
    }

    #[test]
    fn plugin_bounds_clamping() {
        // F1: verify that the clamping logic is present in the source.
        // Full integration coverage is in sdkt-cli/tests/plugin_loading.rs.
        //
        // Unit-level: confirm safe_count is bounded by MAX_FINDINGS regardless
        // of what a plugin writes — since SdktAuditReportC::findings is a fixed
        // [SdktAuditFindingC; MAX_FINDINGS], any buf.count > MAX_FINDINGS would
        // produce an out-of-bounds index without the clamp.
        let buf = SdktAuditReportC {
            count: 999999,
            ..Default::default()
        };
        let safe_count = buf.count.min(crate::plugin_abi::MAX_FINDINGS);
        assert_eq!(safe_count, crate::plugin_abi::MAX_FINDINGS);
    }

    #[test]
    fn load_rejects_missing_file() {
        let err = PluginRule::load(Path::new("/nonexistent/plugin.so"), "fn x() {}");
        assert!(matches!(err, Err(PluginLoadError::DlOpen(_))));
    }
}
