//! End-to-end diagnostic and self-check runner for `sdkt-audit` plugins.
//!
//! Orchestrates the multi-stage validation lifecycle:
//! 1. Metadata: `plugin.toml` presence, valid TOML, basic invariant checks.
//! 2. Artifact: file existence, kind-extension match.
//! 3. Integrity: bundle digest / manifest verification, signature checks.
//! 4. Compatibility: ABI major version comparison against the host.
//! 5. Load: loader availability and symbol verification.
//! 6. Self-check: controlled dry-run analysis on an embedded sample contract.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditContext, AuditRule};
use crate::plugin_abi::SDKT_AUDIT_ABI_MAJOR;
#[cfg(feature = "wasm-plugins")]
use crate::plugin_abi_wasm::SDKT_AUDIT_WASM_ABI_MAJOR;
#[cfg(not(feature = "wasm-plugins"))]
const SDKT_AUDIT_WASM_ABI_MAJOR: u32 = 1;

use crate::plugin_store::{self, PluginMeta};

/// Embedded sample contract source used for Stage 6 self-check.
pub const DOCTOR_SAMPLE_CONTRACT: &str = "pub fn sdkt_example_trigger_admin() {}\n";

/// Status of an individual doctor diagnostic stage.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DoctorStageStatus {
    /// Stage completed successfully.
    Passed,
    /// Stage failed; subsequent stages were aborted.
    Failed,
}

/// A single stage in the doctor execution pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorStage {
    /// Stage identifier: "metadata", "artifact", "integrity", "compatibility", "load", "self-check".
    pub name: String,
    /// Status: "passed" or "failed".
    pub status: DoctorStageStatus,
    /// Human-readable detail or error explanation.
    pub detail: String,
}

/// Structured diagnostic report returned by `sdkt plugin doctor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorReport {
    /// The target name or path evaluated.
    pub target: String,
    /// Whether all executed stages passed.
    pub healthy: bool,
    /// Ordered list of stages executed up to the first failure (or all stages if healthy).
    pub stages: Vec<DoctorStage>,
}

impl DoctorReport {
    /// Create a new report for `target`.
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            healthy: true,
            stages: Vec::new(),
        }
    }

    /// Record a stage result. If `status` is `Failed`, marks `healthy` as false.
    pub fn add_stage(
        &mut self,
        name: impl Into<String>,
        status: DoctorStageStatus,
        detail: impl Into<String>,
    ) {
        if status == DoctorStageStatus::Failed {
            self.healthy = false;
        }
        self.stages.push(DoctorStage {
            name: name.into(),
            status,
            detail: detail.into(),
        });
    }

    /// The first failing stage, if any.
    pub fn failed_stage(&self) -> Option<&DoctorStage> {
        self.stages
            .iter()
            .find(|s| s.status == DoctorStageStatus::Failed)
    }

    /// Exit code: 0 if healthy, or the 1-based index of the first failing stage.
    pub fn exit_code(&self) -> i32 {
        if self.healthy {
            0
        } else {
            for (idx, stage) in self.stages.iter().enumerate() {
                if stage.status == DoctorStageStatus::Failed {
                    return (idx + 1) as i32;
                }
            }
            1
        }
    }
}

/// RAII guard to clean up a temporary directory upon drop.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run plugin doctor diagnostics for `target` using default store resolution.
pub fn doctor(target: &str) -> DoctorReport {
    let root = plugin_store::resolve_store_root();
    doctor_with_root(&root, target)
}

/// Run plugin doctor diagnostics for `target` with an explicit store `root`.
pub fn doctor_with_root(root: &Path, target: &str) -> DoctorReport {
    let mut report = DoctorReport::new(target);
    let target_path = Path::new(target);

    // Resolve target to either a bundle file or a directory
    let is_local_file = target_path.is_file();
    let is_local_dir = target_path.is_dir();

    if is_local_file {
        run_bundle_doctor(&mut report, target_path);
    } else if is_local_dir {
        run_directory_doctor(&mut report, target_path);
    } else {
        // Not a direct local path; resolve via store root
        let store_dir = plugin_store::plugin_dir(root, target);
        if store_dir.exists() {
            run_directory_doctor(&mut report, &store_dir);
        } else {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                format!(
                    "target '{}' not found: not a valid file/directory and not installed in store ({})",
                    target,
                    root.display()
                ),
            );
        }
    }

    report
}

/// Run plugin doctor diagnostics strictly for an installed plugin `id` in the default store.
pub fn doctor_installed(id: &str) -> DoctorReport {
    let root = plugin_store::resolve_store_root();
    doctor_installed_with_root(&root, id)
}

/// Run plugin doctor diagnostics strictly for an installed plugin `id` with an explicit store `root`.
pub fn doctor_installed_with_root(root: &Path, id: &str) -> DoctorReport {
    let mut report = DoctorReport::new(id);
    let store_dir = plugin_store::plugin_dir(root, id);
    if store_dir.exists() {
        run_directory_doctor(&mut report, &store_dir);
    } else {
        report.add_stage(
            "metadata",
            DoctorStageStatus::Failed,
            format!("plugin '{}' not found in store ({})", id, root.display()),
        );
    }
    report
}

/// Execute doctor stages against a packed `.sdktplugin` bundle file.
fn run_bundle_doctor(report: &mut DoctorReport, bundle_path: &Path) {
    let file = match std::fs::File::open(bundle_path) {
        Ok(f) => f,
        Err(e) => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                format!("failed to open bundle: {e}"),
            );
            return;
        }
    };

    let mut archive = tar::Archive::new(file);
    let mut entries = BTreeMap::<String, Vec<u8>>::new();
    let read_res = (|| -> Result<(), String> {
        let items = archive.entries().map_err(|e| e.to_string())?;
        for item in items {
            let mut entry = item.map_err(|e| e.to_string())?;
            if entry.header().entry_type().is_file() {
                let path = entry
                    .path()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                entries.insert(path, bytes);
            }
        }
        Ok(())
    })();

    if let Err(e) = read_res {
        report.add_stage(
            "metadata",
            DoctorStageStatus::Failed,
            format!("invalid bundle archive: {e}"),
        );
        return;
    }

    // Stage 1: metadata
    let raw_meta = match entries.get("plugin.toml") {
        Some(b) => b,
        None => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                "missing plugin.toml in bundle",
            );
            return;
        }
    };
    let meta_str = match std::str::from_utf8(raw_meta) {
        Ok(s) => s,
        Err(_) => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                "plugin.toml is not valid UTF-8",
            );
            return;
        }
    };
    let meta: PluginMeta = match toml::from_str(meta_str) {
        Ok(m) => m,
        Err(e) => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                format!("failed to parse plugin.toml: {e}"),
            );
            return;
        }
    };
    if let Err(e) = meta.validate_basic() {
        report.add_stage(
            "metadata",
            DoctorStageStatus::Failed,
            format!("invalid plugin metadata: {e}"),
        );
        return;
    }
    report.add_stage(
        "metadata",
        DoctorStageStatus::Passed,
        format!(
            "valid plugin metadata for '{}' (v{}, {})",
            meta.id, meta.version, meta.kind
        ),
    );

    // Stage 2: artifact
    if !entries.contains_key(&meta.artifact) {
        report.add_stage(
            "artifact",
            DoctorStageStatus::Failed,
            format!(
                "artifact '{}' declared in plugin.toml not found in bundle",
                meta.artifact
            ),
        );
        return;
    }
    if let Err(e) = plugin_store::validate_kind_ext(&meta, Path::new(&meta.artifact)) {
        report.add_stage("artifact", DoctorStageStatus::Failed, e.to_string());
        return;
    }
    report.add_stage(
        "artifact",
        DoctorStageStatus::Passed,
        format!(
            "artifact '{}' present and matches kind '{}'",
            meta.artifact, meta.kind
        ),
    );

    // Stage 3: integrity (reuse verify_bundle)
    let staging = std::env::temp_dir().join(format!(
        "sdkt-doctor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _guard = TempDirGuard(staging.clone());

    match plugin_store::verify_bundle(bundle_path, &staging, None) {
        Ok(verification) => {
            let detail = if verification.signed {
                "bundle integrity verified (signed)"
            } else {
                "bundle integrity verified (unsigned)"
            };
            report.add_stage("integrity", DoctorStageStatus::Passed, detail);
        }
        Err(e) => {
            report.add_stage("integrity", DoctorStageStatus::Failed, e.to_string());
            return;
        }
    }

    let artifact_path = staging.join(&meta.artifact);

    // Stages 4, 5, 6
    run_compatibility_load_and_check(report, &meta, &artifact_path);
}

/// Execute doctor stages against an unpacked plugin directory.
fn run_directory_doctor(report: &mut DoctorReport, dir: &Path) {
    // Stage 1: metadata
    let toml_path = dir.join("plugin.toml");
    let raw_meta = match std::fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                format!("plugin.toml not found at '{}': {e}", toml_path.display()),
            );
            return;
        }
    };
    let meta: PluginMeta = match toml::from_str(&raw_meta) {
        Ok(m) => m,
        Err(e) => {
            report.add_stage(
                "metadata",
                DoctorStageStatus::Failed,
                format!("failed to parse plugin.toml: {e}"),
            );
            return;
        }
    };
    if let Err(e) = meta.validate_basic() {
        report.add_stage(
            "metadata",
            DoctorStageStatus::Failed,
            format!("invalid plugin metadata: {e}"),
        );
        return;
    }
    report.add_stage(
        "metadata",
        DoctorStageStatus::Passed,
        format!(
            "valid plugin metadata for '{}' (v{}, {})",
            meta.id, meta.version, meta.kind
        ),
    );

    // Stage 2: artifact
    let artifact_path = dir.join(&meta.artifact);
    if !artifact_path.exists() {
        report.add_stage(
            "artifact",
            DoctorStageStatus::Failed,
            format!("artifact not found at '{}'", artifact_path.display()),
        );
        return;
    }
    if let Err(e) = plugin_store::validate_kind_ext(&meta, &artifact_path) {
        report.add_stage("artifact", DoctorStageStatus::Failed, e.to_string());
        return;
    }
    report.add_stage(
        "artifact",
        DoctorStageStatus::Passed,
        format!("artifact found at '{}'", artifact_path.display()),
    );

    // Stage 3: integrity
    let manifest_path = dir.join("manifest.sha256");
    if manifest_path.exists() {
        match verify_dir_manifest(dir, &manifest_path, &meta.artifact) {
            Ok(signed) => {
                let detail = if signed {
                    "manifest verified (signed)"
                } else {
                    "manifest verified (unsigned)"
                };
                report.add_stage("integrity", DoctorStageStatus::Passed, detail);
            }
            Err(e) => {
                report.add_stage("integrity", DoctorStageStatus::Failed, e);
                return;
            }
        }
    } else {
        report.add_stage(
            "integrity",
            DoctorStageStatus::Passed,
            "directory target (unbundled)",
        );
    }

    // Stages 4, 5, 6
    run_compatibility_load_and_check(report, &meta, &artifact_path);
}

/// Verify directory integrity against `manifest.sha256`, checking digest matches,
/// preventing path traversal, ensuring required files (`plugin.toml` and artifact) are covered,
/// and validating signatures if present.
fn verify_dir_manifest(dir: &Path, manifest_path: &Path, artifact: &str) -> Result<bool, String> {
    let manifest_bytes = std::fs::read(manifest_path).map_err(|e| e.to_string())?;
    let manifest_str = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| "manifest.sha256 is not UTF-8".to_string())?;
    let mut covered = std::collections::BTreeSet::new();
    for line in manifest_str.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (expected_digest, rel_path) = if let Some((d, p)) = line.split_once("  ") {
            (d, p)
        } else if let Some((d, p)) = line.split_once(' ') {
            (d, p.trim_start())
        } else {
            return Err("invalid manifest line".to_string());
        };
        if !plugin_store::is_safe_relative_path(Path::new(rel_path)) {
            return Err(format!("unsafe manifest path: {rel_path}"));
        }
        let target_file = dir.join(rel_path);
        let bytes =
            std::fs::read(&target_file).map_err(|e| format!("cannot read {rel_path}: {e}"))?;
        let actual_digest = plugin_store::digest_hex(&bytes);
        if actual_digest != expected_digest {
            return Err(format!("digest mismatch: {rel_path}"));
        }
        covered.insert(rel_path.to_string());
    }
    for required in ["plugin.toml", artifact] {
        if !covered.contains(required) {
            return Err(format!("manifest does not cover '{required}'"));
        }
    }
    let sig_path = dir.join("signature.ed25519");
    let pubkey_path = dir.join("public_key.ed25519");
    if sig_path.exists() != pubkey_path.exists() {
        return Err("incomplete signature: signature and public key must both be present".into());
    }
    if sig_path.exists() && pubkey_path.exists() {
        let sig_bytes = std::fs::read(sig_path).map_err(|e| e.to_string())?;
        let pubkey_bytes = std::fs::read(pubkey_path).map_err(|e| e.to_string())?;
        let sig = Signature::from_slice(&sig_bytes)
            .map_err(|_| "invalid signature format".to_string())?;
        let arr: [u8; 32] = pubkey_bytes
            .as_slice()
            .try_into()
            .map_err(|_| "invalid public key format".to_string())?;
        let pubkey =
            VerifyingKey::from_bytes(&arr).map_err(|_| "invalid public key format".to_string())?;
        pubkey
            .verify(&manifest_bytes, &sig)
            .map_err(|_| "signature verification failed".to_string())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Run Stages 4 (compatibility), 5 (load), and 6 (self-check) on an artifact.
#[allow(unused_variables)]
fn run_compatibility_load_and_check(
    report: &mut DoctorReport,
    meta: &PluginMeta,
    artifact_path: &Path,
) {
    // Stage 4: compatibility
    let host_major = match meta.kind.as_str() {
        "native" => SDKT_AUDIT_ABI_MAJOR,
        "wasm" => SDKT_AUDIT_WASM_ABI_MAJOR,
        _ => SDKT_AUDIT_ABI_MAJOR,
    };

    if meta.abi_major != host_major {
        report.add_stage(
            "compatibility",
            DoctorStageStatus::Failed,
            format!(
                "plugin ABI mismatch (plugin v{}.x, host v{}.x)",
                meta.abi_major, host_major
            ),
        );
        return;
    }

    // Optionally probe the artifact's exported ABI version if available
    #[allow(unused_mut)]
    let mut artifact_abi_mismatch: Option<String> = None;
    #[cfg(feature = "plugins")]
    if meta.kind == "native" {
        if let Ok(lib) = unsafe { libloading::Library::new(artifact_path) } {
            if let Ok(abi_fn) =
                unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"sdkt_plugin_abi_version\0") }
            {
                let version = unsafe { abi_fn() };
                let plugin_major = crate::plugin_abi::abi_major(version);
                if plugin_major != host_major {
                    artifact_abi_mismatch = Some(format!(
                        "plugin ABI mismatch (plugin v{plugin_major}.x, host v{host_major}.x)"
                    ));
                }
            }
        }
    }
    #[cfg(feature = "wasm-plugins")]
    if meta.kind == "wasm" {
        if let Ok(wasm_bytes) = std::fs::read(artifact_path) {
            let manifest = extism::Manifest::new([extism::Wasm::data(wasm_bytes)])
                .with_timeout(std::time::Duration::from_millis(5000));
            if let Ok(mut plugin) = extism::Plugin::new(&manifest, [], true) {
                if let Ok(abi_version_raw) = plugin.call::<(), i64>("sdkt_plugin_abi_version", ()) {
                    let plugin_major = abi_version_raw.unsigned_abs() as u32;
                    if plugin_major != host_major {
                        artifact_abi_mismatch = Some(format!(
                            "plugin ABI mismatch (plugin v{plugin_major}.x, host v{host_major}.x)"
                        ));
                    }
                }
            }
        }
    }

    if let Some(err) = artifact_abi_mismatch {
        report.add_stage("compatibility", DoctorStageStatus::Failed, err);
        return;
    }

    report.add_stage(
        "compatibility",
        DoctorStageStatus::Passed,
        format!(
            "ABI compatible (host v{}.x, plugin v{}.x)",
            host_major, meta.abi_major
        ),
    );

    // Stage 5: load and Stage 6: self-check
    if meta.kind == "native" {
        #[cfg(feature = "plugins")]
        {
            match crate::plugin_loader::PluginRule::load(artifact_path, DOCTOR_SAMPLE_CONTRACT) {
                Ok(rule) => {
                    report.add_stage(
                        "load",
                        DoctorStageStatus::Passed,
                        format!("plugin loaded successfully (id: '{}')", rule.id()),
                    );
                    run_self_check(&rule, report);
                }
                Err(e) => {
                    report.add_stage("load", DoctorStageStatus::Failed, e.to_string());
                }
            }
        }
        #[cfg(not(feature = "plugins"))]
        {
            report.add_stage(
                "load",
                DoctorStageStatus::Failed,
                "loader unavailable (build compiled without `plugins` feature)",
            );
        }
    } else if meta.kind == "wasm" {
        #[cfg(feature = "wasm-plugins")]
        {
            match crate::plugin_loader_wasm::WasmPluginRule::load(
                artifact_path,
                DOCTOR_SAMPLE_CONTRACT,
            ) {
                Ok(rule) => {
                    report.add_stage(
                        "load",
                        DoctorStageStatus::Passed,
                        format!("plugin loaded successfully (id: '{}')", rule.id()),
                    );
                    run_self_check(&rule, report);
                }
                Err(e) => {
                    report.add_stage("load", DoctorStageStatus::Failed, e.to_string());
                }
            }
        }
        #[cfg(not(feature = "wasm-plugins"))]
        {
            report.add_stage(
                "load",
                DoctorStageStatus::Failed,
                "loader unavailable (build compiled without `wasm-plugins` feature)",
            );
        }
    } else {
        report.add_stage(
            "load",
            DoctorStageStatus::Failed,
            format!("unsupported plugin kind '{}'", meta.kind),
        );
    }
}

/// Run Stage 6 self-check against the sample contract source.
#[allow(dead_code)]
fn run_self_check(rule: &dyn AuditRule, report: &mut DoctorReport) {
    let scans = crate::audit::scan_all_functions_str(DOCTOR_SAMPLE_CONTRACT).unwrap_or_default();
    let ctx = AuditContext { spec: None };
    let mut audit_report = crate::types::AuditReport::default();

    let check_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rule.check(&scans, &ctx, &mut audit_report);
    }));

    match check_res {
        Ok(()) => {
            let trap_finding = audit_report.findings.iter().find(|f| {
                f.message.contains("wasm plugin trap") || f.message.contains("mutex poisoned")
            });
            if let Some(f) = trap_finding {
                report.add_stage(
                    "self-check",
                    DoctorStageStatus::Failed,
                    format!("self-check caught error: {}", f.message),
                );
            } else {
                report.add_stage(
                    "self-check",
                    DoctorStageStatus::Passed,
                    format!(
                        "self-check completed ({} finding(s) emitted)",
                        audit_report.findings.len()
                    ),
                );
            }
        }
        Err(_) => {
            report.add_stage(
                "self-check",
                DoctorStageStatus::Failed,
                "plugin panicked during execution",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create dummy plugin directories for testing.
    fn write_plugin(dir: &Path, kind: &str, artifact: &str, abi_major: u32, create_artifact: bool) {
        fs::create_dir_all(dir).unwrap();
        let toml_content = format!(
            r#"id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
author = "SaboLabs"
description = "A test rule"
kind = "{kind}"
artifact = "{artifact}"
abi_major = {abi_major}
abi_minor = 0
"#
        );
        fs::write(dir.join("plugin.toml"), toml_content).unwrap();
        if create_artifact {
            fs::write(dir.join(artifact), b"dummy-bytes").unwrap();
        }
    }

    #[test]
    fn test_target_not_found() {
        let store_root = TempDir::new().unwrap();
        let report = doctor_with_root(store_root.path(), "non-existent-plugin");
        assert!(!report.healthy);
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.stages.len(), 1);
        assert_eq!(report.stages[0].name, "metadata");
        assert_eq!(report.stages[0].status, DoctorStageStatus::Failed);
    }

    #[test]
    fn test_invalid_metadata_bad_kind() {
        let store_root = TempDir::new().unwrap();
        let plugin_dir = store_root.path().join("test-plugin");
        write_plugin(&plugin_dir, "invalid-kind", "rule.so", 1, true);

        let report = doctor_with_root(store_root.path(), "test-plugin");
        assert!(!report.healthy);
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.stages.len(), 1);
        assert_eq!(report.stages[0].name, "metadata");
        assert_eq!(report.stages[0].status, DoctorStageStatus::Failed);
        assert!(report.stages[0]
            .detail
            .contains("kind must be 'native' or 'wasm'"));
    }

    #[test]
    fn test_invalid_metadata_reserved_path() {
        let store_root = TempDir::new().unwrap();
        let plugin_dir = store_root.path().join("test-plugin");
        write_plugin(&plugin_dir, "native", "manifest.sha256", 1, true);

        let report = doctor_with_root(store_root.path(), "test-plugin");
        assert!(!report.healthy);
        assert_eq!(report.exit_code(), 1);
        assert_eq!(report.stages.len(), 1);
        assert_eq!(report.stages[0].name, "metadata");
        assert_eq!(report.stages[0].status, DoctorStageStatus::Failed);
        assert!(report.stages[0].detail.contains("reserved bundle path"));
    }

    #[test]
    fn test_missing_artifact_fails_at_artifact_stage() {
        let store_root = TempDir::new().unwrap();
        let plugin_dir = store_root.path().join("test-plugin");
        write_plugin(&plugin_dir, "native", "rule.so", 1, false);

        let report = doctor_with_root(store_root.path(), "test-plugin");
        assert!(!report.healthy);
        assert_eq!(report.exit_code(), 2);
        assert_eq!(report.stages.len(), 2);
        assert_eq!(report.stages[0].name, "metadata");
        assert_eq!(report.stages[0].status, DoctorStageStatus::Passed);
        assert_eq!(report.stages[1].name, "artifact");
        assert_eq!(report.stages[1].status, DoctorStageStatus::Failed);
        assert!(report.stages[1].detail.contains("rule.so"));
    }

    #[test]
    fn test_abi_mismatch_fails_compatibility_stage() {
        let store_root = TempDir::new().unwrap();
        let plugin_dir = store_root.path().join("test-plugin");
        write_plugin(&plugin_dir, "native", "rule.so", 99, true);

        let report = doctor_with_root(store_root.path(), "test-plugin");
        assert!(!report.healthy);
        assert_eq!(report.exit_code(), 4);
        assert_eq!(report.stages.len(), 4);
        assert_eq!(report.stages[0].name, "metadata");
        assert_eq!(report.stages[0].status, DoctorStageStatus::Passed);
        assert_eq!(report.stages[1].name, "artifact");
        assert_eq!(report.stages[1].status, DoctorStageStatus::Passed);
        assert_eq!(report.stages[2].name, "integrity");
        assert_eq!(report.stages[2].status, DoctorStageStatus::Passed);
        assert_eq!(report.stages[3].name, "compatibility");
        assert_eq!(report.stages[3].status, DoctorStageStatus::Failed);
        assert!(report.stages[3].detail.contains("plugin v99.x, host v1.x"));
    }

    #[test]
    fn test_tampered_bundle_fails_integrity_stage() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("src");
        write_plugin(&src_dir, "wasm", "rule.wasm", 1, true);

        let meta: PluginMeta =
            toml::from_str(&fs::read_to_string(src_dir.join("plugin.toml")).unwrap()).unwrap();
        let bundle_path = tmp.path().join("bundle.sdktplugin");
        plugin_store::pack_bundle(&bundle_path, &meta, &src_dir.join("rule.wasm"), None).unwrap();

        // Doctor on healthy bundle up to Stage 3 or 5
        let report_before = doctor_with_root(tmp.path(), bundle_path.to_str().unwrap());
        assert_eq!(report_before.stages[2].name, "integrity");
        assert_eq!(report_before.stages[2].status, DoctorStageStatus::Passed);

        // Tamper with the manifest digest inside the bundle
        let mut bundle_bytes = fs::read(&bundle_path).unwrap();
        let digest = plugin_store::digest_hex(b"dummy-bytes").into_bytes();
        let offset = bundle_bytes
            .windows(digest.len())
            .position(|window| window == digest)
            .unwrap();
        bundle_bytes[offset] = if bundle_bytes[offset] == b'0' {
            b'1'
        } else {
            b'0'
        };
        fs::write(&bundle_path, bundle_bytes).unwrap();

        let report_after = doctor_with_root(tmp.path(), bundle_path.to_str().unwrap());
        assert!(!report_after.healthy);
        assert_eq!(report_after.exit_code(), 3);
        assert_eq!(report_after.stages.len(), 3);
        assert_eq!(report_after.stages[0].name, "metadata");
        assert_eq!(report_after.stages[0].status, DoctorStageStatus::Passed);
        assert_eq!(report_after.stages[1].name, "artifact");
        assert_eq!(report_after.stages[1].status, DoctorStageStatus::Passed);
        assert_eq!(report_after.stages[2].name, "integrity");
        assert_eq!(report_after.stages[2].status, DoctorStageStatus::Failed);
        assert!(report_after.stages[2].detail.contains("digest mismatch"));
    }

    #[test]
    fn test_dir_manifest_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plugin");
        write_plugin(&dir, "wasm", "rule.wasm", 1, true);

        // Write a manifest with path traversal entry
        let manifest =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  ../outside.txt\n";
        fs::write(dir.join("manifest.sha256"), manifest).unwrap();

        let report = doctor_with_root(tmp.path(), dir.to_str().unwrap());
        assert!(!report.healthy);
        assert_eq!(report.stages[2].name, "integrity");
        assert_eq!(report.stages[2].status, DoctorStageStatus::Failed);
        assert!(report.stages[2].detail.contains("unsafe manifest path"));
    }

    #[test]
    fn test_dir_manifest_missing_artifact_coverage_fails() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plugin");
        write_plugin(&dir, "wasm", "rule.wasm", 1, true);

        // Manifest only covers plugin.toml, not rule.wasm
        let toml_bytes = fs::read(dir.join("plugin.toml")).unwrap();
        let toml_digest = plugin_store::digest_hex(&toml_bytes);
        let manifest = format!("{toml_digest}  plugin.toml\n");
        fs::write(dir.join("manifest.sha256"), manifest).unwrap();

        let report = doctor_with_root(tmp.path(), dir.to_str().unwrap());
        assert!(!report.healthy);
        assert_eq!(report.stages[2].name, "integrity");
        assert_eq!(report.stages[2].status, DoctorStageStatus::Failed);
        assert!(report.stages[2]
            .detail
            .contains("manifest does not cover 'rule.wasm'"));
    }

    #[test]
    fn test_dir_manifest_mismatched_signature_files_fails() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plugin");
        write_plugin(&dir, "wasm", "rule.wasm", 1, true);

        let toml_bytes = fs::read(dir.join("plugin.toml")).unwrap();
        let toml_digest = plugin_store::digest_hex(&toml_bytes);
        let wasm_bytes = fs::read(dir.join("rule.wasm")).unwrap();
        let wasm_digest = plugin_store::digest_hex(&wasm_bytes);
        let manifest = format!("{toml_digest}  plugin.toml\n{wasm_digest}  rule.wasm\n");
        fs::write(dir.join("manifest.sha256"), manifest).unwrap();

        // Only write signature.ed25519 without public_key.ed25519
        fs::write(dir.join("signature.ed25519"), b"fake-signature").unwrap();

        let report = doctor_with_root(tmp.path(), dir.to_str().unwrap());
        assert!(!report.healthy);
        assert_eq!(report.stages[2].name, "integrity");
        assert_eq!(report.stages[2].status, DoctorStageStatus::Failed);
        assert!(report.stages[2].detail.contains("incomplete signature"));
    }
}
