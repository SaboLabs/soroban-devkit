//! C-ABI boundary types for dynamic audit plugins (M18, Phase B).
//!
//! Only flat `#[repr(C)]` data crosses the FFI. No Rust trait objects, `Box`,
//! `Vec`, or `FnScan` are passed across the boundary — the plugin performs its
//! own scanning internally and returns only flat [`Finding`](crate::types::Finding)
//! records via a fixed-capacity buffer.

use std::os::raw::{c_char, c_int};

/// Plugin ABI major version. BREAKING changes bump this; the host rejects any
/// plugin whose major does not match.
pub const SDKT_AUDIT_ABI_MAJOR: u32 = 1;
/// Plugin ABI minor version. Additive/backward-compatible changes bump this.
///
/// The host DOES NOT reject plugins with a lower minor (older plugin, newer host).
/// Plugins with a higher minor (newer plugin, older host) may expose symbols the
/// host does not call — that is safe. Only a major-version mismatch causes a
/// load failure.
pub const SDKT_AUDIT_ABI_MINOR: u32 = 0;

/// Pack `(major, minor)` into the single `u32` returned by
/// `sdkt_plugin_abi_version`.
pub fn abi_version_pack() -> u32 {
    (SDKT_AUDIT_ABI_MAJOR << 16) | (SDKT_AUDIT_ABI_MINOR & 0xFFFF)
}

/// Extract the major component of a packed ABI version.
pub fn abi_major(v: u32) -> u32 {
    v >> 16
}

/// Extract the minor component of a packed ABI version.
pub fn abi_minor(v: u32) -> u32 {
    v & 0xFFFF
}

/// Severity encoding shared with the C-ABI boundary (must match
/// [`Severity`](crate::types::Severity) ordering).
pub const SEVERITY_CRITICAL: u32 = 0;
/// See [`SEVERITY_CRITICAL`].
pub const SEVERITY_WARNING: u32 = 1;
/// See [`SEVERITY_CRITICAL`].
pub const SEVERITY_INFO: u32 = 2;

/// Maximum findings a plugin may emit in a single `sdkt_plugin_check` call.
pub const MAX_FINDINGS: usize = 64;

/// A single finding as seen across the FFI. `rule_id`, `message`, and
/// `location` are NUL-terminated C strings owned by the plugin and valid only
/// for the duration of the `sdkt_plugin_check` call. The host copies them into
/// owned `String`s before returning.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SdktAuditFindingC {
    /// Stable rule id, e.g. `EXAMPLE-001`.
    pub rule_id: *const c_char,
    /// One of [`SEVERITY_CRITICAL`], [`SEVERITY_WARNING`], [`SEVERITY_INFO`].
    pub severity: u32,
    /// Human-readable message.
    pub message: *const c_char,
    /// Optional location (function or binding name); NUL ptr if absent.
    pub location: *const c_char,
}

/// Fixed-capacity report buffer the plugin writes into during
/// `sdkt_plugin_check`. The host reads only the first `count` entries.
#[repr(C)]
pub struct SdktAuditReportC {
    /// Finding slots.
    pub findings: [SdktAuditFindingC; MAX_FINDINGS],
    /// Number of valid entries in `findings`.
    pub count: usize,
}

impl Default for SdktAuditReportC {
    fn default() -> Self {
        // SAFETY: zeroed pointers are valid (null) and `count = 0` means the
        // host reads nothing. This type contains only POD fields.
        unsafe { std::mem::zeroed() }
    }
}

// Re-export the raw int type for plugin authors' convenience.
#[allow(missing_docs)]
pub type CInt = c_int;
