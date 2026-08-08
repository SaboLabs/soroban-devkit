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
//! feature is compiled in). Signature/checksum verification is an explicit
//! NON-GOAL for M40.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::plugin_abi::SDKT_AUDIT_ABI_MAJOR;

/// Metadata stored in each plugin's `plugin.toml`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
