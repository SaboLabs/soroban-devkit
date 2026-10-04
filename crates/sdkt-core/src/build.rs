use crate::config::DevKitConfig;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default Soroban contract target supported by current soroban-sdk releases.
pub const WASM_BUILD_TARGET: &str = "wasm32v1-none";

/// Minimum `stellar-cli` for the canonical build path.
///
/// `soroban-sdk` v28 turns spec shaking v2 on unconditionally: the SDK emits a
/// spec entry and a marker for every type, and the *build system* is expected to
/// strip the entries unreachable from the contract boundary. Building with plain
/// `cargo build` therefore exits 101 ("soroban-sdk requires stellar-cli
/// v25.2.0+ to build a contract") on wasm targets, and bypassing that guard with
/// the raw env var yields an **unstripped, oversized** spec — an artifact that is
/// not what v28 defines. `stellar contract build` is that shaking build system;
/// v25.2.0 is the first release that provides it.
pub const STELLAR_CLI_MIN_VERSION: (u64, u64) = (25, 2);

#[derive(Debug)]
pub enum BuildError {
    MissingConfig,
    PathNotFound(String),
    CargoFailed {
        path: String,
        stderr: String,
    },
    ArtifactNotFound(String),
    InvalidProject(String),
    /// The contract needs the canonical v28 shaking build system
    /// (`stellar contract build`), but `stellar` is not installed.
    StellarCliMissing,
    /// `stellar` is installed but older than [`STELLAR_CLI_MIN_VERSION`].
    StellarCliUnsupported {
        version: String,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::MissingConfig => write!(f, "No [contracts] configured in .sdkt.toml"),
            BuildError::PathNotFound(path) => write!(f, "Contract path does not exist: {}", path),
            BuildError::CargoFailed { path, stderr } => {
                write!(f, "Cargo build failed in {}:\n{}", path, stderr)
            }
            BuildError::ArtifactNotFound(path) => {
                write!(f, "Expected WASM artifact not found at: {}", path)
            }
            BuildError::InvalidProject(msg) => {
                write!(f, "Invalid project dependency graph: {}", msg)
            }
            BuildError::StellarCliMissing => write!(
                f,
                "This contract requires `stellar contract build` (soroban-sdk v28 or later \
                 builds its spec with the build system), but the `stellar` CLI was not found. \
                 Install stellar-cli v{}.{} (https://developers.stellar.org/docs/building \
                 or `cargo install --locked stellar-cli`) and retry.",
                STELLAR_CLI_MIN_VERSION.0, STELLAR_CLI_MIN_VERSION.1
            ),
            BuildError::StellarCliUnsupported { version } => write!(
                f,
                "Installed stellar-cli {} is too old for soroban-sdk v28 contract builds. \
                 Spec shaking v2 requires `stellar contract build` from stellar-cli v{}.{} \
                 (found {}).",
                version, STELLAR_CLI_MIN_VERSION.0, STELLAR_CLI_MIN_VERSION.1, version
            ),
        }
    }
}

impl std::error::Error for BuildError {}

/// Result of building a single contract.
#[derive(Debug, PartialEq)]
pub struct BuildResult {
    pub alias: String,
    pub path: String,
    pub wasm_artifact: PathBuf,
}

/// Parse the leading `major.minor` out of a semver requirement such as
/// `"28.0.0"`, `"=27.0.6"` or `"^28"`. Returns `None` when no digits lead.
fn parse_leading_version(req: &str) -> Option<(u64, u64)> {
    let req = req
        .trim()
        .trim_start_matches(['=', '^', '~', '>', '<', 'v', ' ']);
    let mut parts = req.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Extract a declared version requirement from one dependency line.
///
/// Handles both manifest shapes:
/// - `soroban-sdk = { version = "28.0.0", features = [...] }`
/// - `soroban-sdk = "=27.0.6"`
fn declared_version(line: &str) -> Option<(u64, u64)> {
    let quoted = |from: &str| -> Option<(u64, u64)> {
        let q1 = from.find('"')?;
        let rest = &from[q1 + 1..];
        let q2 = rest.find('"')?;
        parse_leading_version(&rest[..q2])
    };
    if let Some(vpos) = line.find("version") {
        if let Some(v) = quoted(&line[vpos + "version".len()..]) {
            return Some(v);
        }
    }
    // Plain-string requirement: `soroban-sdk = "=27.0.6"`.
    if let Some(eq) = line.find('=') {
        return quoted(&line[eq + 1..]);
    }
    None
}

/// True when the manifest's *own* dependency tables declare `soroban-sdk >= 28`.
///
/// Only `[dependencies]` / `[dependencies.<name>]` sections are scanned: dev,
/// build, `[patch]` and `[replace]` sections, doc comments, and workspace-
/// inherited requirements must not silently flip the build path. The table
/// form carries the package name in the section header, so it is tracked
/// separately from inline `soroban-sdk = { ... }` lines.
fn requires_sdk_28(manifest: &str) -> bool {
    // Package named by the current `[dependencies.<name>]` header, if any.
    let mut table_dep: Option<String> = None;
    let mut in_deps = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]" || t.starts_with("[dependencies.");
            table_dep = if in_deps {
                t.strip_prefix("[dependencies.")
                    .map(|name| name.trim_end_matches(']').to_string())
            } else {
                None
            };
            continue;
        }
        if !in_deps {
            continue;
        }
        if line.contains("soroban-sdk") || table_dep.as_deref() == Some("soroban-sdk") {
            if let Some((major, _)) = declared_version(line) {
                return major >= 28;
            }
        }
    }
    false
}

/// Does this contract need the canonical Protocol 28 build system
/// (`stellar contract build`), i.e. does it declare soroban-sdk >= 28?
///
/// `pub(crate)` so the scaffold tests can prove a generated Protocol 28
/// project routes through the canonical build path.
pub(crate) fn requires_v28_build_system(path: &Path) -> bool {
    let Ok(manifest) = std::fs::read_to_string(path.join("Cargo.toml")) else {
        return false;
    };
    requires_sdk_28(&manifest)
}

/// Installed `stellar-cli` version as `(major, minor)`, read from
/// `stellar --version` (first line, e.g. `stellar 28.1.0 (c0f4d0da...)`).
fn stellar_cli_version() -> Option<(u64, u64)> {
    let output = Command::new("stellar").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    parse_leading_version(line.split_whitespace().nth(1)?)
}

/// Builds all contracts defined in the DevKitConfig.
///
/// For each contract, it navigates to the configured `path` and runs:
/// `cargo build --target wasm32v1-none --release`
///
/// The build proceeds in the dependency-resolved deploy order produced by
/// [`crate::project::resolve_deploy_order`], so a malformed graph
/// (unknown/self/duplicate dependency or a cycle) is rejected up front with a
/// clear error before any `cargo` invocation.
pub fn build_workspace(config: &DevKitConfig) -> Result<Vec<BuildResult>, BuildError> {
    if config.contracts.is_empty() {
        return Err(BuildError::MissingConfig);
    }

    // 4.2 — validate + order via the single shared resolver. This ensures
    // build, deploy, and lock generation all use the same resolved graph.
    let ordered = crate::project::resolve_deploy_order(config)
        .map_err(|e| BuildError::InvalidProject(e.to_string()))?;

    let mut results = Vec::new();

    for alias in &ordered {
        let contract_cfg = config
            .contracts
            .get(alias)
            .expect("alias from resolved order");
        let path = Path::new(&contract_cfg.path);

        if !path.exists() || !path.is_dir() {
            return Err(BuildError::PathNotFound(contract_cfg.path.clone()));
        }

        // Protocol 28 (soroban-sdk >= 28): spec shaking v2 is unconditional, so
        // only `stellar contract build` (stellar-cli >= 25.2.0) can produce the
        // final stripped spec. Detect the requirement from the manifest, then
        // either build canonically or fail with an actionable error — plain
        // `cargo build` would exit 101, and forcing the guard env var would ship
        // an unshaken spec (measured: 11 spec entries vs 1 on a boundary-only
        // type), which is an artifact v28 does not define.
        let report_failure = |stderr: &[u8], label: &str| {
            BuildError::CargoFailed {
            path: contract_cfg.path.clone(),
            stderr: format!(
                "{label} for target {WASM_BUILD_TARGET} failed. If this target is not installed, run `rustup target add {WASM_BUILD_TARGET}`.\n{}",
                String::from_utf8_lossy(stderr)
            ),
        }
        };

        if requires_v28_build_system(path) {
            let (major, minor) = stellar_cli_version().ok_or(BuildError::StellarCliMissing)?;
            if (major, minor) < STELLAR_CLI_MIN_VERSION {
                return Err(BuildError::StellarCliUnsupported {
                    version: format!("{major}.{minor}"),
                });
            }
            let output = Command::new("stellar")
                .arg("contract")
                .arg("build")
                .current_dir(path)
                .output()
                .map_err(|e| BuildError::CargoFailed {
                    path: contract_cfg.path.clone(),
                    stderr: e.to_string(),
                })?;
            if !output.status.success() {
                return Err(report_failure(&output.stderr, "`stellar contract build`"));
            }
        } else {
            // Execute cargo build (unchanged path for pre-v28 projects)
            let output = Command::new("cargo")
                .arg("build")
                .arg("--target")
                .arg(WASM_BUILD_TARGET)
                .arg("--release")
                .current_dir(path)
                .output()
                .map_err(|e| BuildError::CargoFailed {
                    path: contract_cfg.path.clone(),
                    stderr: e.to_string(),
                })?;
            if !output.status.success() {
                return Err(report_failure(&output.stderr, "Build"));
            }
        }

        // We assume a standard Soroban project structure where Cargo.toml has a package name.
        // For sdkt build, we will attempt to extract the expected artifact name from Cargo.toml,
        // or just glob the target/<target>/release/*.wasm dir.
        // For stability without adding `cargo-metadata` dependency, we will look for any .wasm file
        // generated in the release directory.
        let target_dir = path.join("target").join(WASM_BUILD_TARGET).join("release");

        let mut found_wasm = None;
        if target_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&target_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().is_some_and(|ext| ext == "wasm") {
                        found_wasm = Some(p);
                        break;
                    }
                }
            }
        }

        let wasm_artifact = found_wasm
            .ok_or_else(|| BuildError::ArtifactNotFound(target_dir.display().to_string()))?;

        results.push(BuildResult {
            alias: alias.clone(),
            path: contract_cfg.path.clone(),
            wasm_artifact,
        });
    }

    // 4.1 — generate `sdkt.lock` next to `.sdkt.toml` recording every built
    // artifact's SHA-256 and the deterministic deploy order. Advisory only:
    // if lock generation fails (e.g. an artifact vanished between build and
    // hashing), surface a warning but do not fail the build.
    if let Ok(lock) = crate::lock::generate_lock(Path::new("."), config) {
        match crate::lock::write_lock(Path::new("."), &lock) {
            Ok(path) => {
                if let Ok(toml) = crate::lock::lock_to_toml(&lock) {
                    // Advisory lock report goes to stderr, not stdout, so
                    // `sdkt build --format json` emits the JSON document alone
                    // and stays machine-parseable. The information is
                    // unchanged for a human running the pretty form.
                    eprintln!("✓ Wrote {}", path.display());
                    eprintln!("{}", toml);
                }
            }
            Err(e) => eprintln!("Warning: could not write sdkt.lock: {}", e),
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContractConfig;
    use std::collections::HashMap;

    #[test]
    fn test_build_empty_config() {
        let config = DevKitConfig::default();
        let res = build_workspace(&config);
        assert!(matches!(res, Err(BuildError::MissingConfig)));
    }

    #[test]
    fn test_build_missing_path() {
        let mut config = DevKitConfig::default();
        let mut contracts = HashMap::new();
        contracts.insert(
            "token".to_string(),
            ContractConfig {
                path: "does_not_exist_xyz".to_string(),
                deploy_after: vec![],
                depends_on: vec![],
            },
        );
        config.contracts = contracts;

        let res = build_workspace(&config);
        match res {
            Err(BuildError::PathNotFound(p)) => assert_eq!(p, "does_not_exist_xyz"),
            _ => panic!("Expected PathNotFound"),
        }
    }

    // ---- Protocol 28 build-system detection ----

    #[test]
    fn parses_leading_version_from_requirement_styles() {
        assert_eq!(parse_leading_version("28.0.0"), Some((28, 0)));
        assert_eq!(parse_leading_version("=27.0.6"), Some((27, 0)));
        assert_eq!(parse_leading_version("^28"), Some((28, 0)));
        assert_eq!(parse_leading_version("v25.2.1"), Some((25, 2)));
        assert_eq!(parse_leading_version(">=28.0.0"), Some((28, 0)));
        assert_eq!(parse_leading_version("no-digits-here"), None);
    }

    #[test]
    fn detects_sdk_28_requirement_from_manifest() {
        // v28-only project (the soroban-examples shape)
        assert!(requires_sdk_28(
            "[dependencies]\nsoroban-sdk = { version = \"28.0.0\" }\n"
        ));
        // pre-28 projects keep the legacy cargo path
        assert!(!requires_sdk_28(
            "[dependencies]\nsoroban-sdk = \"=27.0.6\"\n\n[dev-dependencies]\nsoroban-sdk = { version = \"=27.0.6\", features = [\"testutils\"] }\n"
        ));
        // table form
        assert!(requires_sdk_28(
            "[dependencies.soroban-sdk]\nversion = \"28.1.0\"\n"
        ));
        // unreadable requirement -> conservative legacy path
        assert!(!requires_sdk_28(
            "[dependencies]\nsoroban-sdk.workspace = true\n"
        ));
        assert!(!requires_sdk_28("[dependencies]\nserde = \"1\"\n"));
    }

    #[test]
    fn reports_actionable_error_when_stellar_cli_missing() {
        let msg = BuildError::StellarCliMissing.to_string();
        assert!(msg.contains("stellar contract build"));
        assert!(msg.contains("25.2"), "must name the minimum version: {msg}");
    }

    #[test]
    fn reports_actionable_error_when_stellar_cli_too_old() {
        let msg = BuildError::StellarCliUnsupported {
            version: "24.0.0".to_string(),
        }
        .to_string();
        assert!(msg.contains("24.0.0"));
        assert!(msg.contains("25.2"), "must name the minimum version: {msg}");
    }
}
