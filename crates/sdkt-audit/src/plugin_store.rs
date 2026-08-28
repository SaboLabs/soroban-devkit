//! Local, offline-first plugin store for `sdkt-audit` (M40).
//!
//! This module manages *plugin metadata and lifecycle only*. It never replaces
//! or duplicates the existing plugin loaders ([`crate::plugin_loader`] for native
//! `.so`/`.dylib`/`.dll`, [`crate::plugin_loader_wasm`] for `.wasm`). Loading a
//! plugin into the [`crate::registry::RuleRegistry`] is still the job of those
//! loaders; this store merely resolves a stable plugin `id` to an artifact path
//! and validates install metadata.
//!
//! # Store layout
//!
//! ```text
//! <store-root>/<plugin-id>/plugin.toml   # metadata
//! <store-root>/<plugin-id>/<artifact>    # the .so/.dylib/.dll/.wasm file
//! ```
//!
//! # Root precedence (lowest → highest)
//!
//! 1. `<cwd>/.sdkt/plugins`
//! 2. `<config-dir>/sdkt/plugins`  (XDG/config per `dirs`)
//! 3. `$SDKT_PLUGIN_DIR` (environment override)
//!
//! # Trust model
//!
//! Provenance-by-path: the user installed the artifact from a local file they
//! obtained out-of-band. No third-party trust is assumed. Install validates the
//! metadata, the `abi_major` constant, and the kind/extension match, and performs
//! an optional dry-run load via the existing loader (when the corresponding
//! feature is compiled in). `.sdktplugin` bundles add deterministic digest and
//! optional signature verification without requiring a hosted registry.

use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;

use crate::plugin_abi::SDKT_AUDIT_ABI_MAJOR;

/// Metadata stored in each plugin's `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginMeta {
    /// Stable, namespaced plugin id (e.g. `author/name`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Semver version of the plugin.
    pub version: String,
    /// Plugin author / maintainer.
    pub author: String,
    /// Short description.
    pub description: String,
    /// Artifact kind: `native` (`.so`/`.dylib`/`.dll`) or `wasm` (`.wasm`).
    pub kind: String,
    /// Artifact filename inside the plugin directory.
    pub artifact: String,
    /// Plugin ABI major version. Must equal the host [`SDKT_AUDIT_ABI_MAJOR`].
    pub abi_major: u32,
    /// Plugin ABI minor version (informational).
    pub abi_minor: u32,
}

impl PluginMeta {
    /// Validate the metadata invariants that do not require touching the
    /// artifact: id non-empty, kind recognized, abi_major matches the host.
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.id.trim().is_empty() {
            return Err(StoreError::InvalidMetadata("id must not be empty".into()));
        }
        if !matches!(self.kind.as_str(), "native" | "wasm") {
            return Err(StoreError::InvalidMetadata(format!(
                "kind must be 'native' or 'wasm', got '{}'",
                self.kind
            )));
        }
        if self.artifact.trim().is_empty() {
            return Err(StoreError::InvalidMetadata(
                "artifact must not be empty".into(),
            ));
        }
        if !is_safe_relative_path(Path::new(&self.artifact)) {
            return Err(StoreError::InvalidMetadata(
                "artifact must be a safe relative path".into(),
            ));
        }
        if matches!(
            self.artifact.as_str(),
            "plugin.toml" | "manifest.sha256" | "signature.ed25519" | "public_key.ed25519"
        ) {
            return Err(StoreError::InvalidMetadata(
                "artifact uses a reserved bundle path".into(),
            ));
        }
        if self.abi_major != SDKT_AUDIT_ABI_MAJOR {
            return Err(StoreError::AbiMismatch {
                plugin_major: self.abi_major,
                host_major: SDKT_AUDIT_ABI_MAJOR,
            });
        }
        Ok(())
    }

    /// Expected artifact extension for this kind.
    pub fn expected_ext(&self) -> &'static str {
        match self.kind.as_str() {
            "native" => "so", // validated loosely; see validate_kind_ext
            "wasm" => "wasm",
            _ => "",
        }
    }
}

/// Errors produced by the plugin store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse plugin.toml: {0}")]
    Toml(String),
    #[error("invalid plugin metadata: {0}")]
    InvalidMetadata(String),
    #[error("plugin ABI mismatch (plugin v{plugin_major}.x, host v{host_major}.x)")]
    AbiMismatch { plugin_major: u32, host_major: u32 },
    #[error("artifact extension '{actual}' does not match plugin kind '{kind}'")]
    KindExtMismatch { kind: String, actual: String },
    #[error("plugin '{0}' is not installed")]
    NotInstalled(String),
    #[error("plugin '{0}' already installed (use --force to overwrite)")]
    AlreadyInstalled(String),
    #[error("dry-run load of plugin failed: {0}")]
    DryRunLoad(String),
    #[error("remote sources are not supported in M40 (local paths only)")]
    RemoteUnsupported,
    #[error("invalid plugin bundle: {0}")]
    InvalidBundle(String),
    #[error("plugin bundle signature verification failed")]
    InvalidSignature,
}

/// Result returned after a bundle has been verified. `signed` is false for a
/// deliberately unsigned local bundle; callers should report that fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleVerification {
    pub metadata: PluginMeta,
    pub signed: bool,
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_header(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_size(size);
    h.set_mode(0o644);
    h.set_mtime(0);
    h.set_cksum();
    h
}

/// Pack one plugin artifact and its metadata into a deterministic tar-based
/// `.sdktplugin` bundle. The archive contains `plugin.toml`, the artifact, and
/// `manifest.sha256`; optional Ed25519 signature files are also included.
pub fn pack_bundle(
    output: &Path,
    meta: &PluginMeta,
    artifact: &Path,
    signing_key: Option<&SigningKey>,
) -> Result<(), StoreError> {
    meta.validate()?;
    validate_kind_ext(meta, artifact)?;
    let artifact_bytes = std::fs::read(artifact)?;
    let metadata = toml::to_string(meta).map_err(|e| StoreError::InvalidBundle(e.to_string()))?;
    let mut manifest = BTreeMap::new();
    manifest.insert("plugin.toml", digest_hex(metadata.as_bytes()));
    manifest.insert(meta.artifact.as_str(), digest_hex(&artifact_bytes));
    let manifest_bytes = manifest
        .iter()
        .map(|(p, d)| format!("{}  {}\n", d, p))
        .collect::<String>()
        .into_bytes();
    let mut file = std::fs::File::create(output)?;
    let mut archive = tar::Builder::new(&mut file);
    for (name, bytes) in [
        ("plugin.toml", metadata.into_bytes()),
        ("manifest.sha256", manifest_bytes.clone()),
        (meta.artifact.as_str(), artifact_bytes),
    ] {
        let mut h = stable_header(bytes.len() as u64);
        archive.append_data(&mut h, name, bytes.as_slice())?;
    }
    if let Some(key) = signing_key {
        let sig = key.sign(&manifest_bytes).to_bytes().to_vec();
        let pubkey = key.verifying_key().to_bytes().to_vec();
        let mut h = stable_header(sig.len() as u64);
        archive.append_data(&mut h, "signature.ed25519", sig.as_slice())?;
        let mut h = stable_header(pubkey.len() as u64);
        archive.append_data(&mut h, "public_key.ed25519", pubkey.as_slice())?;
    }
    archive.finish()?;
    Ok(())
}

/// Verify a bundle and extract it only after all metadata, digest, signature,
/// and path-safety checks pass. `verifying_key` is optional only for unsigned
/// bundles; signed bundles always verify against the embedded public key and,
/// when supplied, the caller's key must match it.
pub fn verify_bundle(
    bundle: &Path,
    destination: &Path,
    verifying_key: Option<&VerifyingKey>,
) -> Result<BundleVerification, StoreError> {
    let file = std::fs::File::open(bundle)?;
    let mut archive = tar::Archive::new(file);
    let mut entries = BTreeMap::<String, Vec<u8>>::new();
    for item in archive.entries()? {
        let mut entry = item?;
        if !entry.header().entry_type().is_file() {
            return Err(StoreError::InvalidBundle(
                "bundle contains a non-file entry".into(),
            ));
        }
        let path = entry.path()?.to_path_buf();
        if !is_safe_relative_path(&path) {
            return Err(StoreError::InvalidBundle("path traversal rejected".into()));
        }
        let name = path.to_string_lossy().into_owned();
        if entries.contains_key(&name) {
            return Err(StoreError::InvalidBundle("duplicate archive entry".into()));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        entries.insert(name, bytes);
    }
    let raw_meta = entries
        .get("plugin.toml")
        .ok_or_else(|| StoreError::InvalidBundle("missing plugin.toml".into()))?;
    let meta = parse_meta(
        std::str::from_utf8(raw_meta)
            .map_err(|_| StoreError::InvalidBundle("plugin.toml is not UTF-8".into()))?,
    )?;
    let manifest = entries
        .get("manifest.sha256")
        .ok_or_else(|| StoreError::InvalidBundle("missing manifest.sha256".into()))?;
    let text = std::str::from_utf8(manifest)
        .map_err(|_| StoreError::InvalidBundle("manifest is not UTF-8".into()))?;
    let mut expected = BTreeMap::new();
    for line in text.lines() {
        let (digest, path) = line
            .split_once("  ")
            .ok_or_else(|| StoreError::InvalidBundle("invalid manifest line".into()))?;
        if digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || !is_safe_relative_path(Path::new(path))
        {
            return Err(StoreError::InvalidBundle(
                "invalid manifest path or digest".into(),
            ));
        }
        if expected
            .insert(path.to_string(), digest.to_string())
            .is_some()
        {
            return Err(StoreError::InvalidBundle("duplicate manifest entry".into()));
        }
    }
    if expected.get("plugin.toml").is_none() || expected.get(meta.artifact.as_str()).is_none() {
        return Err(StoreError::InvalidBundle(
            "manifest does not cover metadata and artifact".into(),
        ));
    }
    for (path, digest) in &expected {
        let bytes = entries
            .get(path)
            .ok_or_else(|| StoreError::InvalidBundle(format!("manifest entry missing: {path}")))?;
        if digest != &digest_hex(bytes) {
            return Err(StoreError::InvalidBundle(format!(
                "digest mismatch: {path}"
            )));
        }
    }
    for name in entries.keys() {
        if !expected.contains_key(name)
            && !matches!(
                name.as_str(),
                "manifest.sha256" | "signature.ed25519" | "public_key.ed25519"
            )
        {
            return Err(StoreError::InvalidBundle(format!(
                "unlisted bundle entry: {name}"
            )));
        }
    }
    let signed = entries.contains_key("signature.ed25519");
    if signed {
        let sig = Signature::from_slice(entries.get("signature.ed25519").unwrap())
            .map_err(|_| StoreError::InvalidSignature)?;
        let embedded = VerifyingKey::from_bytes(
            entries
                .get("public_key.ed25519")
                .ok_or(StoreError::InvalidSignature)?
                .as_slice()
                .try_into()
                .map_err(|_| StoreError::InvalidSignature)?,
        )
        .map_err(|_| StoreError::InvalidSignature)?;
        if let Some(key) = verifying_key {
            if key != &embedded {
                return Err(StoreError::InvalidSignature);
            }
        }
        embedded
            .verify(manifest, &sig)
            .map_err(|_| StoreError::InvalidSignature)?;
    }
    std::fs::create_dir_all(destination)?;
    for (name, bytes) in entries {
        let path = destination.join(&name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(BundleVerification {
        metadata: meta,
        signed,
    })
}

/// Verify a `.sdktplugin` bundle before installing its artifact into the local
/// store. Unsigned bundles are accepted for local use, but the returned
/// metadata lets callers report that the bundle was unsigned.
pub fn install_bundle(bundle: &Path, opts: &InstallOpts) -> Result<BundleVerification, StoreError> {
    let staging = std::env::temp_dir().join(format!(
        "sdkt-plugin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let verified = verify_bundle(bundle, &staging, None)?;
        let source = staging.join(&verified.metadata.artifact);
        install(&source, opts)?;
        Ok(verified)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Resolve the plugin store root using the documented precedence.
///
/// 1. `$SDKT_PLUGIN_DIR` (highest)
/// 2. `<config-dir>/sdkt/plugins`
/// 3. `<cwd>/.sdkt/plugins` (lowest)
pub fn resolve_store_root() -> PathBuf {
    if let Ok(env) = std::env::var("SDKT_PLUGIN_DIR") {
        if !env.trim().is_empty() {
            return PathBuf::from(env);
        }
    }
    if let Some(config) = dirs::config_dir() {
        let cfg = config.join("sdkt").join("plugins");
        if cfg.exists() {
            return cfg;
        }
    }
    // Fallback to cwd/.sdkt/plugins (always computable, may not exist yet).
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".sdkt")
        .join("plugins")
}

/// Directory for a specific plugin id under the store root.
fn plugin_dir(root: &Path, id: &str) -> PathBuf {
    root.join(sanitize_id(id))
}

/// Prevent path traversal in plugin ids (they become directory names).
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '/' | '\\' | '.' | ':' | ' ' => '_',
            _ => c,
        })
        .collect()
}

/// Read and validate a plugin's metadata from its directory.
pub fn read_meta(root: &Path, id: &str) -> Result<PluginMeta, StoreError> {
    let toml_path = plugin_dir(root, id).join("plugin.toml");
    let raw =
        std::fs::read_to_string(&toml_path).map_err(|_| StoreError::NotInstalled(id.into()))?;
    parse_meta(&raw)
}

/// Parse `plugin.toml` content (exposed for unit testing).
pub fn parse_meta(raw: &str) -> Result<PluginMeta, StoreError> {
    let meta: PluginMeta = toml::from_str(raw).map_err(|e| StoreError::Toml(e.to_string()))?;
    meta.validate()?;
    Ok(meta)
}

/// List all installed plugins (ids + metadata).
pub fn list() -> Vec<PluginMeta> {
    let root = resolve_store_root();
    list_in(&root)
}

/// List installed plugins under a specific root (testable).
pub fn list_in(root: &Path) -> Vec<PluginMeta> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        if let Ok(meta) = read_meta(root, &e.file_name().to_string_lossy()) {
            out.push(meta);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Show a single plugin's metadata by id.
pub fn show(id: &str) -> Option<PluginMeta> {
    let root = resolve_store_root();
    read_meta(&root, id).ok()
}

/// Resolve a plugin id to its artifact path, if installed.
pub fn resolve(id: &str) -> Option<PathBuf> {
    let root = resolve_store_root();
    let dir = plugin_dir(&root, id);
    let meta = read_meta(&root, id).ok()?;
    let artifact = dir.join(&meta.artifact);
    if artifact.exists() {
        Some(artifact)
    } else {
        None
    }
}

/// Validate that the artifact extension matches the declared kind.
fn validate_kind_ext(meta: &PluginMeta, artifact_path: &Path) -> Result<(), StoreError> {
    let ext = artifact_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let ok = match meta.kind.as_str() {
        "native" => matches!(ext.as_str(), "so" | "dylib" | "dll"),
        "wasm" => ext == "wasm",
        _ => false,
    };
    if !ok {
        return Err(StoreError::KindExtMismatch {
            kind: meta.kind.clone(),
            actual: ext,
        });
    }
    Ok(())
}

/// Options for [`install`].
#[derive(Debug, Default, Clone)]
pub struct InstallOpts {
    /// Override the plugin id from metadata (rarely needed).
    pub id: Option<String>,
    /// Overwrite an existing install of the same id.
    pub force: bool,
}

/// Install a plugin from a local artifact path.
///
/// Steps: parse+validate metadata (from a sibling/provided `plugin.toml` or the
/// artifact's directory), check `abi_major`, check kind/extension, optionally
/// dry-run load via the existing loader, then copy the artifact + manifest into
/// the store. Nothing is committed to the store until all checks pass.
pub fn install(local_source: &Path, opts: &InstallOpts) -> Result<PluginMeta, StoreError> {
    if local_source.to_string_lossy().starts_with("http://")
        || local_source.to_string_lossy().starts_with("https://")
    {
        return Err(StoreError::RemoteUnsupported);
    }

    // Locate the plugin.toml: either next to the artifact, or the artifact
    // itself is the manifest? No — artifact is the binary; manifest is separate.
    let source_dir = local_source
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let toml_path = source_dir.join("plugin.toml");
    let raw = std::fs::read_to_string(&toml_path).map_err(|_| {
        StoreError::InvalidMetadata("plugin.toml not found next to the artifact".into())
    })?;
    let meta = parse_meta(&raw)?;

    let id = opts.id.clone().unwrap_or_else(|| meta.id.clone());
    validate_kind_ext(&meta, local_source)?;

    let root = resolve_store_root();
    let dir = plugin_dir(&root, &id);
    if dir.exists() && !opts.force {
        return Err(StoreError::AlreadyInstalled(id));
    }

    // Optional dry-run load using the EXISTING loaders (feature-gated). This
    // reuses, never replaces, the M18/M19 loading paths.
    #[cfg(feature = "plugins")]
    if meta.kind == "native" {
        crate::plugin_loader::PluginRule::load(local_source, "")
            .map_err(|e| StoreError::DryRunLoad(e.to_string()))?;
    }
    #[cfg(feature = "wasm-plugins")]
    if meta.kind == "wasm" {
        crate::plugin_loader_wasm::WasmPluginRule::load(local_source, "")
            .map_err(|e| StoreError::DryRunLoad(e.to_string()))?;
    }

    // Commit: create dir, copy artifact + manifest.
    std::fs::create_dir_all(&dir)?;
    let dest_artifact = dir.join(&meta.artifact);
    std::fs::copy(local_source, &dest_artifact)?;
    std::fs::write(dir.join("plugin.toml"), raw)?;
    Ok(meta)
}

/// Remove an installed plugin by id. Idempotent: removing an absent plugin is a
/// no-op success.
pub fn remove(id: &str) -> Result<(), StoreError> {
    let root = resolve_store_root();
    let dir = plugin_dir(&root, id);
    if !dir.exists() {
        return Ok(()); // idempotent
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Update an installed plugin from a new local artifact (local-only).
pub fn update(id: &str, local_source: &Path) -> Result<PluginMeta, StoreError> {
    if local_source.to_string_lossy().starts_with("http://")
        || local_source.to_string_lossy().starts_with("https://")
    {
        return Err(StoreError::RemoteUnsupported);
    }
    // Reuse install with force semantics.
    let opts = InstallOpts {
        id: Some(id.to_string()),
        force: true,
    };
    install(local_source, &opts)
}

#[cfg(test)]
mod bundle_tests {
    use super::*;
    use std::fs;

    fn meta() -> PluginMeta {
        PluginMeta {
            id: "example-rule".into(),
            name: "Example Rule".into(),
            version: "1.0.0".into(),
            author: "SaboLabs".into(),
            description: "test".into(),
            kind: "wasm".into(),
            artifact: "rule.wasm".into(),
            abi_major: SDKT_AUDIT_ABI_MAJOR,
            abi_minor: 0,
        }
    }

    #[test]
    fn bundle_is_reproducible_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("rule.wasm");
        fs::write(&artifact, b"wasm").unwrap();
        let a = dir.path().join("a.sdktplugin");
        let b = dir.path().join("b.sdktplugin");
        pack_bundle(&a, &meta(), &artifact, None).unwrap();
        pack_bundle(&b, &meta(), &artifact, None).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        let out = dir.path().join("out");
        let verified = verify_bundle(&a, &out, None).unwrap();
        assert!(!verified.signed);
        assert_eq!(fs::read(out.join("rule.wasm")).unwrap(), b"wasm");
    }

    #[test]
    fn tampered_bundle_fails_before_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("rule.wasm");
        fs::write(&artifact, b"wasm").unwrap();
        let bundle = dir.path().join("plugin.sdktplugin");
        pack_bundle(&bundle, &meta(), &artifact, None).unwrap();
        let mut bytes = fs::read(&bundle).unwrap();
        let digest = digest_hex(b"wasm").into_bytes();
        let offset = bytes
            .windows(digest.len())
            .position(|window| window == digest)
            .unwrap();
        bytes[offset] = if bytes[offset] == b'0' { b'1' } else { b'0' };
        fs::write(&bundle, bytes).unwrap();
        let out = dir.path().join("out");
        assert!(verify_bundle(&bundle, &out, None).is_err());
        assert!(!out.exists());
    }

    #[test]
    fn signed_bundle_verifies_and_wrong_key_fails() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("rule.wasm");
        fs::write(&artifact, b"wasm").unwrap();
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let bundle = dir.path().join("plugin.sdktplugin");
        pack_bundle(&bundle, &meta(), &artifact, Some(&key)).unwrap();
        assert!(
            verify_bundle(&bundle, &dir.path().join("out"), Some(&key.verifying_key()))
                .unwrap()
                .signed
        );
        let wrong = SigningKey::from_bytes(&[8u8; 32]);
        assert!(matches!(
            verify_bundle(
                &bundle,
                &dir.path().join("wrong"),
                Some(&wrong.verifying_key())
            ),
            Err(StoreError::InvalidSignature)
        ));
    }
}
