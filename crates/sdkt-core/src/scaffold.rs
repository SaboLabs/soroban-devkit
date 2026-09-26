use std::fs;
use std::io;
use std::path::Path;

/// Configuration for project scaffolding.
#[derive(Debug, Clone)]
pub struct ScaffoldConfig {
    /// Project name (also used as directory name).
    pub name: String,
    /// If true, generate only Cargo.toml, src/lib.rs, .sdkt.toml.
    pub minimal: bool,
    /// If true, overwrite existing directory.
    pub force: bool,
}

/// Result of a successful scaffold operation.
#[derive(Debug)]
pub struct ScaffoldResult {
    /// Files that were created.
    pub files_created: Vec<String>,
}

/// Generate a new Soroban contract project.
///
/// Creates the directory structure and all template files.
/// Returns an error if the directory exists and `force` is false.
pub fn generate_project(config: &ScaffoldConfig) -> io::Result<ScaffoldResult> {
    let root = Path::new(&config.name);

    // Extract just the directory name for the Rust package name
    let package_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid project name"))?
        .to_string();

    if root.exists() && !config.force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "directory '{}' already exists (use --force to overwrite)",
                config.name
            ),
        ));
    }

    fs::create_dir_all(root.join("src"))?;

    let mut created = Vec::new();

    // --- Always generated ---

    let crate_name = package_name.replace('-', "_");

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
soroban-sdk = "21.0.0"

[dev-dependencies]
soroban-sdk = {{ version = "21.0.0", features = ["testutils"] }}

[lib]
crate-type = ["cdylib"]

[profile.release]
opt-level = "z"
overflow-checks = true
debug = 0
strip = "symbols"
debug-assertions = false
panic = "abort"
codegen-units = 1
lto = true
"#,
        name = package_name,
    );
    write_template(root, "Cargo.toml", &cargo_toml, &mut created)?;

    let lib_rs = r#"#![no_std]
use soroban_sdk::{contract, contractimpl, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
 /// Returns a greeting number.
    pub fn hello(_env: Env) -> u32 {
        42
    }
}
"#;
    write_template(root, "src/lib.rs", lib_rs, &mut created)?;

    let sdkt_toml = r#"[network]
default = "testnet"
rpc_url = "https://soroban-testnet.stellar.org"

[build]
target = "wasm32-unknown-unknown"
"#;
    write_template(root, ".sdkt.toml", sdkt_toml, &mut created)?;

    // --- Full mode only ---

    if !config.minimal {
        let readme = format!(
            "# {name}\n\nA Soroban smart contract project.\n\n## Build\n\n```\nsdkt build\n```\n\n## Test\n\n```\ncargo test\n```\n",
            name = package_name,
        );
        write_template(root, "README.md", &readme, &mut created)?;

        write_template(root, ".gitignore", "/target\n", &mut created)?;

        fs::create_dir_all(root.join("tests"))?;

        let basic_test = format!(
            r#"#![cfg(test)]

use {crate_name}::{{Contract, ContractClient}};
use soroban_sdk::Env;

#[test]
fn test_hello() {{
    let env = Env::default();
    let contract_id = env.register_contract(None, Contract);
    let client = ContractClient::new(&env, &contract_id);
    assert_eq!(client.hello(), 42);
}}
"#,
            crate_name = crate_name,
        );
        write_template(root, "tests/basic.rs", &basic_test, &mut created)?;
    }

    Ok(ScaffoldResult {
        files_created: created,
    })
}

/// Configuration for plugin-rule scaffolding.
#[derive(Debug, Clone)]
pub struct PluginScaffoldConfig {
    /// Plugin project name (also used as directory name).
    pub name: String,
    /// If true, overwrite existing directory.
    pub force: bool,
}

/// The exact set of files `sdkt plugin init` generates. Pinned by a test so the
/// documented layout cannot silently drift.
pub const PLUGIN_SCAFFOLD_FILES: &[&str] = &[
    "Cargo.toml",
    "src/lib.rs",
    "src/plugin_abi.rs",
    "src/plugin_abi_wasm.rs",
    "plugin/plugin.toml",
    "plugin-wasm/plugin.toml",
    "README.md",
    "tests/rule_test.rs",
    ".gitignore",
];

/// The version of `sdkt-audit` a scaffolded plugin depends on.
///
/// The scaffolded crate is standalone (outside this workspace), so it cannot
/// use a `path` dependency; it consumes the published crate instead. This is
/// forwarded from the workspace version so it tracks release bumps; the crate
/// must be published before the workspace version moves past it.
const PLUGIN_AUDIT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sanitize a crate name into a valid Rust library identifier
/// (`-` → `_`). Used for the `[lib] name` and derived identifiers.
fn plugin_lib_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Derive a unique rule id from a project name, e.g. `my-rule` → `MY-RULE-001`
/// (mirrors the reference rule `EXAMPLE-001`).
fn plugin_rule_id(name: &str) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("{}-001", base)
}

/// Upper-camel identifier for the generated rule struct, e.g.
/// `my-rule` → `MyRule`.
fn plugin_struct_name(name: &str) -> String {
    let identifier: String = name
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut it = seg.chars();
            let head = it.next().map(|c| c.to_ascii_uppercase()).unwrap_or('X');
            head.to_string() + &it.collect::<String>()
        })
        .collect();
    // `Self` is a reserved Rust identifier, so a project named `self` must not
    // emit `pub struct Self;`. Prefix it; all other names stay unchanged.
    if identifier == "Self" {
        format!("Plugin{identifier}")
    } else {
        identifier
    }
}

/// Human-readable plugin name, e.g. `my-rule` → `My Rule`.
fn plugin_display_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut it = seg.chars();
            let head = it.next().map(|c| c.to_ascii_uppercase()).unwrap_or('X');
            head.to_string() + &it.collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_plugin_name(name: &str) -> io::Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "invalid plugin rule name '{}': use only letters, digits, '-' and '_', \
                 starting with a letter",
                name
            ),
        ));
    }
    Ok(())
}

/// Generate a new standalone `sdkt-audit` plugin rule crate.
///
/// The scaffold delivers a buildable rule derived from the in-tree reference
/// implementation (`crates/sdkt-audit-example-rule`): a `Cargo.toml` with the
/// correct `plugins`/`wasm-plugins` feature wiring, a placeholder `AuditRule`
/// whose id is derived from the project name, the native C-ABI and WASM ABI
/// exports, a `plugin/plugin.toml` for the pack/install flow, and a README
/// documenting build → pack → install → audit.
///
/// The crate is intentionally standalone (empty `[workspace]`, like
/// `crates/sdkt-playground`) so `cargo build --release --features plugins`
/// works out of the box wherever it is placed.
pub fn generate_plugin_project(config: &PluginScaffoldConfig) -> io::Result<ScaffoldResult> {
    let root = Path::new(&config.name);

    let package_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid project name"))?
        .to_string();
    validate_plugin_name(&package_name)?;

    if root.exists() && !config.force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "directory '{}' already exists (use --force to overwrite)",
                config.name
            ),
        ));
    }

    let lib_name = plugin_lib_name(&package_name);
    let struct_name = plugin_struct_name(&package_name);
    let display_name = plugin_display_name(&package_name);
    let rule_id = plugin_rule_id(&package_name);
    let trigger = format!("sdkt_{lib_name}_trigger");
    let description = format!(
        "{rule_id}: TODO - replace the placeholder logic. Placeholder fires on functions named `{trigger}`."
    );
    let audit_version = PLUGIN_AUDIT_VERSION;

    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("plugin"))?;
    fs::create_dir_all(root.join("plugin-wasm"))?;
    fs::create_dir_all(root.join("tests"))?;

    let mut created = Vec::new();

    let cargo_toml = format!(
        r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2021"
description = "{description}"
publish = false

[dependencies]
extism-pdk = {{ version = "1.4.1", optional = true }}
serde = {{ version = "1", optional = true, features = ["derive"] }}
serde_json = {{ version = "1", optional = true }}

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
sdkt-audit = {{ version = "{audit_version}" }}

[target.'cfg(not(target_arch = "wasm32"))'.dev-dependencies]
sdkt-audit = {{ version = "{audit_version}" }}

[features]
# Native shared-library plugin (Phase B): builds a loadable .so/.dylib/.dll.
plugins = ["sdkt-audit/plugins"]

# WebAssembly plugin (Phase C): builds a sandboxed .wasm module.
wasm-plugins = ["dep:extism-pdk", "dep:serde", "dep:serde_json", "sdkt-audit/wasm-plugins"]

[lib]
name = "{lib_name}"
# rlib for the compiled-in register() path, cdylib for the loadable artifact.
crate-type = ["rlib", "cdylib"]

# Standalone crate: opts out of any enclosing workspace so `cargo build` works
# from anywhere the project is placed (mirrors crates/sdkt-playground).
[workspace]
"#,
        package_name = package_name,
        description = description,
        lib_name = lib_name,
        audit_version = audit_version,
    );
    write_template(root, "Cargo.toml", &cargo_toml, &mut created)?;

    let lib_rs = format!(
        r#"//! {struct_name} — an `sdkt-audit` plugin rule scaffolded by `sdkt plugin init`.
//!
//! Demonstrates the plugin author workflow from `docs/plugins/plugin-authoring.md`:
//! 1. Implement the [`sdkt_audit::AuditRule`] trait.
//! 2. Register the rule via [`register`] (or the `sdkt_audit::register_rule!` macro).
//! 3. Emit a [`sdkt_audit::Finding`] when the rule's condition holds.
//!
//! Build the loadable native artifact with:
//!
//! ```bash
//! cargo build --release --features plugins
//! ```

#[cfg(not(target_arch = "wasm32"))]
use sdkt_audit::{{
    register_rule, AuditContext, AuditReport, AuditRule, BoxedRule, Finding, FnScan, Severity,
}};

/// {rule_id} — placeholder rule.
///
/// TODO: replace the scaffolded detection below with your real rule once the
/// crate builds and loads.
#[cfg(not(target_arch = "wasm32"))]
pub struct {struct_name};

#[cfg(not(target_arch = "wasm32"))]
impl AuditRule for {struct_name} {{
    fn id(&self) -> &'static str {{
        "{rule_id}"
    }}
    fn severity(&self) -> Severity {{
        Severity::Info
    }}
    fn description(&self) -> &'static str {{
        "{description}"
    }}

    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {{
        // TODO: implement your real detection logic here. The placeholder below
        // deliberately fires on any function whose name contains "{trigger}" so
        // the C-ABI / WASM wiring can be validated end-to-end before you replace
        // it with a real check.
        for s in scans {{
            if s.fn_name.contains("{trigger}") {{
                report.add(Finding {{
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!("{{}} matched trigger function `{{}}`", "{rule_id}", s.fn_name),
                    location: Some(s.fn_name.clone()),
                }});
            }}
        }}
    }}
}}

/// Register this plugin's rule into the process-wide registry (Phase A).
#[cfg(not(target_arch = "wasm32"))]
pub fn register() {{
    register_rule(Box::new({struct_name}) as BoxedRule);
}}

/// C-ABI exports for native dynamic loading (Phase B).
#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
mod plugin_abi;

/// JSON-ABI exports for sandboxed WASM loading (Phase C).
#[cfg(all(feature = "wasm-plugins", target_arch = "wasm32"))]
mod plugin_abi_wasm;
"#,
        struct_name = struct_name,
        rule_id = rule_id,
        description = description,
        trigger = trigger,
    );
    write_template(root, "src/lib.rs", &lib_rs, &mut created)?;

    let plugin_abi_rs = format!(
        r#"//! C-ABI exports for the {struct_name} plugin (Phase B dynamic loading).
//!
//! Compiled only with the `plugins` feature, turning this crate into a loadable
//! shared library (.so/.dylib/.dll). Reuses the in-crate rule logic through the
//! public `sdkt_audit` API — no rule duplication.

use std::ffi::{{CStr, CString}};
use std::os::raw::c_char;
use std::sync::Mutex;

use sdkt_audit::plugin_abi::{{
    abi_version_pack, SdktAuditFindingC, SdktAuditReportC, MAX_FINDINGS, SEVERITY_INFO,
}};
use sdkt_audit::{{AuditReport, AuditRule}};

use crate::{struct_name};

/// Source to analyze, refreshed on every [`sdkt_plugin_init`] call.
///
/// `Mutex<Option<String>>` instead of `OnceLock` so the plugin always operates
/// on the source supplied for the *current* audit execution, not a stale value
/// from a previous run.
static SOURCE: Mutex<Option<String>> = Mutex::new(None);

/// Plugin ABI version (must match the host's `SDKT_AUDIT_ABI_MAJOR`).
///
/// # Safety
/// Standard C-ABI export; returns a packed `u32`. No pointers touched.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_abi_version() -> u32 {{
    abi_version_pack()
}}

/// Rule id `{rule_id}`.
///
/// # Safety
/// Returns a static C string with static lifetime — valid for the program.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_id() -> *const c_char {{
    static ID: &[u8] = b"{rule_id}\0";
    ID.as_ptr() as *const c_char
}}

/// Severity: info.
///
/// # Safety
/// Returns a constant `u32`.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_severity() -> u32 {{
    SEVERITY_INFO
}}

/// Human-readable description.
///
/// # Safety
/// Returns a static C string with static lifetime.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_description() -> *const c_char {{
    static DESC: &[u8] = b"{description}\0";
    DESC.as_ptr() as *const c_char
}}

/// Cache the contract source to be analyzed.
///
/// # Safety
/// `src` must be a valid NUL-terminated C string for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_init(src: *const c_char) -> i32 {{
    if src.is_null() {{
        return 1;
    }}
    match unsafe {{ CStr::from_ptr(src) }}.to_str() {{
        Ok(s) => {{
            if let Ok(mut lock) = SOURCE.lock() {{
                *lock = Some(s.to_string());
                0
            }} else {{
                1
            }}
        }}
        Err(_) => 1,
    }}
}}

/// Run this plugin's rule over the cached source and write findings into `report`.
///
/// # Safety
/// `report` must point to a valid, mutable `SdktAuditReportC`.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_check(report: *mut SdktAuditReportC) -> i32 {{
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {{
        if report.is_null() {{
            return 1;
        }}

        let src = {{
            let lock = match SOURCE.lock() {{
                Ok(l) => l,
                Err(_) => return 2,
            }};
            match &*lock {{
                Some(s) => s.clone(),
                None => return 2,
            }}
        }};

        // Run ONLY this plugin's own rule (via the in-crate `{struct_name}`),
        // never the global registry, to avoid re-entrant recursion when the host
        // invokes this symbol during an `audit_source_with` run.
        let scans = match sdkt_audit::scan_all_functions_str(&src) {{
            Some(s) => s,
            None => return 3,
        }};
        let ctx = sdkt_audit::AuditContext {{ spec: None }};
        let mut local = AuditReport::default();
        {struct_name}.check(&scans, &ctx, &mut local);

        let out = unsafe {{ &mut *report }};
        out.count = 0;
        for f in local.findings {{
            if out.count >= MAX_FINDINGS {{
                break;
            }}
            let rule_id = match CString::new(f.rule_id) {{
                Ok(c) => c,
                Err(_) => continue,
            }};
            let message = match CString::new(f.message) {{
                Ok(c) => c,
                Err(_) => continue,
            }};
            let location = f
                .location
                .as_ref()
                .and_then(|l| CString::new(l.clone()).ok());
            let slot: &mut SdktAuditFindingC = &mut out.findings[out.count];
            slot.rule_id = rule_id.into_raw() as *const c_char;
            slot.severity = match f.severity {{
                sdkt_audit::Severity::Critical => 0,
                sdkt_audit::Severity::Info => 2,
                sdkt_audit::Severity::Warning => 1,
            }};
            slot.message = message.into_raw() as *const c_char;
            slot.location = location
                .map(|c| c.into_raw() as *const c_char)
                .unwrap_or(std::ptr::null());
            out.count += 1;
        }}
        0
    }}));

    res.unwrap_or(4)
}}

/// Optional cleanup (no-op: `SOURCE` is dropped at process exit).
///
/// # Safety
/// Standard C-ABI export.
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_free() {{}}
"#,
        struct_name = struct_name,
        rule_id = rule_id,
        description = description,
    );
    write_template(root, "src/plugin_abi.rs", &plugin_abi_rs, &mut created)?;

    let plugin_abi_wasm_rs = format!(
        r#"//! Extism PDK exports for the WASM plugin architecture (Phase C).
//!
//! When built for wasm32, this module exports the required ABI functions so the
//! `sdkt-audit` host can load it.
//!
//! Deliberately self-contained: it does NOT import from `sdkt-audit` to keep
//! the dependency graph free of host-side crates (syn, sdkt-wasm, etc.) that do
//! not compile cleanly for wasm32-unknown-unknown.

use extism_pdk::*;
use serde::{{Deserialize, Serialize}};

// Mirror of host-side ABI constants (kept local to avoid importing sdkt-audit).
const SDKT_AUDIT_WASM_ABI_MAJOR: i64 = 1;
const SEVERITY_INFO: u32 = 2;

// ── Wire types (must match host's WasmCheckInput / WasmFinding) ────────────

/// Minimal projection of `FnScan` — only what the rule needs.
#[derive(Deserialize)]
pub struct FnScanInput {{
    pub fn_name: String,
}}

#[derive(Deserialize)]
pub struct WasmCheckInput {{
    pub scans: Vec<FnScanInput>,
}}

#[derive(Serialize)]
pub struct WasmFinding {{
    pub rule_id: String,
    pub severity: u32,
    pub message: String,
    pub location: Option<String>,
}}

// ── ABI exports ─────────────────────────────────────────────────────────────

#[plugin_fn]
pub fn sdkt_plugin_abi_version() -> FnResult<i64> {{
    Ok(SDKT_AUDIT_WASM_ABI_MAJOR)
}}

#[plugin_fn]
pub fn sdkt_plugin_id() -> FnResult<String> {{
    Ok("{rule_id}".to_string())
}}

#[plugin_fn]
pub fn sdkt_plugin_severity() -> FnResult<i64> {{
    Ok(SEVERITY_INFO as i64)
}}

#[plugin_fn]
pub fn sdkt_plugin_description() -> FnResult<String> {{
    Ok("{description}".into())
}}

#[plugin_fn]
pub fn sdkt_plugin_check(input_json: String) -> FnResult<String> {{
    let input: WasmCheckInput = serde_json::from_str(&input_json)?;

    let findings: Vec<WasmFinding> = input
        .scans
        .iter()
        .filter(|s| s.fn_name.contains("{trigger}"))
        .map(|s| WasmFinding {{
            rule_id: "{rule_id}".to_string(),
            severity: SEVERITY_INFO,
            message: format!("{{}} matched trigger function `{{}}`", "{rule_id}", s.fn_name),
            location: Some(s.fn_name.clone()),
        }})
        .collect();

    Ok(serde_json::to_string(&findings)?)
}}
"#,
        rule_id = rule_id,
        description = description,
        trigger = trigger,
    );
    write_template(
        root,
        "src/plugin_abi_wasm.rs",
        &plugin_abi_wasm_rs,
        &mut created,
    )?;

    // Name the artifact for the host platform (libmy_rule.so on Linux,
    // libmy_rule.dylib on macOS, my_rule.dll on Windows) so the generated
    // plugin.toml and README point at the file `cargo build` actually produces.
    let artifact = format!(
        "{}{lib_name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let plugin_toml = format!(
        r#"id = "{lib_name}"
name = "{display_name}"
version = "0.1.0"
author = "your-name"
description = "{description}"
kind = "native"
artifact = "{artifact}"
abi_major = 1
abi_minor = 0
"#,
        lib_name = lib_name,
        display_name = display_name,
        description = description,
        artifact = artifact,
    );
    write_template(root, "plugin/plugin.toml", &plugin_toml, &mut created)?;

    // The same rule also targets WASM (Phase C): `--features wasm-plugins`
    // produces a sandboxed .wasm module, so stage a matching manifest with the
    // correct kind/extension. Otherwise pack/install of the .wasm artifact
    // would fail the store's kind-vs-extension validation.
    let wasm_plugin_toml = format!(
        r#"id = "{lib_name}"
name = "{display_name}"
version = "0.1.0"
author = "your-name"
description = "{description}"
kind = "wasm"
artifact = "{lib_name}.wasm"
abi_major = 1
abi_minor = 0
"#,
        lib_name = lib_name,
        display_name = display_name,
        description = description,
    );
    write_template(
        root,
        "plugin-wasm/plugin.toml",
        &wasm_plugin_toml,
        &mut created,
    )?;

    let readme = format!(
        r#"# {display_name}

A scaffolded [`sdkt-audit`](https://crates.io/crates/sdkt-audit) plugin rule
(rule id `{rule_id}`, plugin id `{lib_name}`), generated by `sdkt plugin init`.

This crate builds as:

- an **rlib** by default, for the compiled-in `register()` path (Phase A), and
- a **cdylib** shared library with `--features plugins` (native, Phase B), or
- a **wasm32** module with `--features wasm-plugins` (WASM, Phase C).

## Layout

- `src/lib.rs` — the [`AuditRule`] implementation with a TODO-marked `check()`.
- `src/plugin_abi.rs` — native C-ABI exports (`plugins` feature).
- `src/plugin_abi_wasm.rs` — WASM JSON-ABI exports (`wasm-plugins` feature).
- `plugin/plugin.toml` — metadata for the pack/install flow (native).
- `plugin-wasm/plugin.toml` — metadata for the pack/install flow (WASM).

## Workflow

### 1. Build the rule

```bash
cargo build --release --features plugins
```

Produces `target/release/{artifact}`.

### 2. Stage the artifact next to `plugin.toml`

```bash
mkdir -p plugin
cp target/release/{artifact} plugin/
```

### 3. Pack a portable bundle (optional)

```bash
sdkt plugin pack plugin/ --output {lib_name}-0.1.0.sdktplugin
```

### 4. Install into the local store

```bash
sdkt plugin install plugin/{artifact}
```

### 5. Run the rule against a contract

```bash
sdkt audit path/to/contract.rs --rules {lib_name}
```

`--rules {lib_name}` resolves the plugin id to the installed artifact. You can
also point it at the artifact directly: `--rules plugin/{artifact}`.

### 6. WASM plugin (Phase C, optional)

The same rule also builds as a sandboxed `.wasm` module. Build it and stage the
artifact next to the pre-staged WASM manifest (`plugin-wasm/plugin.toml`,
`kind = "wasm"`):

```bash
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1 --features wasm-plugins
mkdir -p plugin-wasm
cp target/wasm32-wasip1/release/{lib_name}.wasm plugin-wasm/
```

Then pack or install exactly as above, passing `plugin-wasm/` instead of
`plugin/`:

```bash
sdkt plugin pack plugin-wasm/ --output {lib_name}-wasm-0.1.0.sdktplugin
sdkt plugin install plugin-wasm/{lib_name}.wasm
```

## Testing

```bash
cargo test --features plugins
```

The scaffold ships `tests/rule_test.rs` which proves the placeholder rule is
wired: it must produce exactly one finding when a function name contains
`{trigger}`, and stay silent otherwise. Replace that logic with your real check,
then update the test.

See `docs/plugins/plugin-authoring.md` in the Soroban DevKit repository for the full
authoring guide.
"#,
        display_name = display_name,
        rule_id = rule_id,
        lib_name = lib_name,
        trigger = trigger,
        artifact = artifact,
    );
    write_template(root, "README.md", &readme, &mut created)?;

    let rule_test = format!(
        r#"//! Integration test for the scaffolded plugin rule.
//!
//! Proves the placeholder rule is wired end-to-end: a trivially matching
//! function name produces a finding, while a non-matching name stays silent.
//! Run with `cargo test --features plugins`.

use {lib_name}::{struct_name};
use sdkt_audit::{{AuditContext, AuditReport, AuditRule, FnScan}};

/// A function whose name contains the trigger word must produce exactly one
/// finding carrying the rule id.
#[test]
fn rule_fires_on_trigger_function() {{
    let scans = vec![FnScan {{
        fn_name: format!("{{}}_admin", "{trigger}"),
        require_auth: 0,
        invoke_contract: 0,
        bound: Default::default(),
        usage: Default::default(),
    }}];
    let ctx = AuditContext {{ spec: None }};
    let mut report = AuditReport::default();
    {struct_name}.check(&scans, &ctx, &mut report);
    assert_eq!(
        report.summary.total, 1,
        "placeholder rule must fire on a matching function"
    );
    assert_eq!(report.findings[0].rule_id, "{rule_id}");
}}

/// A function whose name does **not** contain the trigger word must produce
/// zero findings.
#[test]
fn rule_silent_on_normal_function() {{
    let scans = vec![FnScan {{
        fn_name: "balance_of".to_string(),
        require_auth: 0,
        invoke_contract: 0,
        bound: Default::default(),
        usage: Default::default(),
    }}];
    let ctx = AuditContext {{ spec: None }};
    let mut report = AuditReport::default();
    {struct_name}.check(&scans, &ctx, &mut report);
    assert!(report.is_clean());
}}
"#,
        lib_name = lib_name,
        struct_name = struct_name,
        rule_id = rule_id,
        trigger = trigger,
    );
    write_template(root, "tests/rule_test.rs", &rule_test, &mut created)?;

    write_template(root, ".gitignore", "/target\n/.sdkt\n/plugin/*.so\n/plugin/*.dylib\n/plugin/*.dll\n/plugin/*.wasm\n/plugin-wasm/*.wasm\n*.sdktplugin\n", &mut created)?;

    Ok(ScaffoldResult {
        files_created: created,
    })
}

fn write_template(
    root: &Path,
    rel_path: &str,
    content: &str,
    created: &mut Vec<String>,
) -> io::Result<()> {
    fs::write(root.join(rel_path), content)?;
    created.push(rel_path.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("sdkt_test_{}", name));
        let _ = fs::remove_dir_all(&p);
        p
    }

    fn cfg(path: &Path, minimal: bool, force: bool) -> ScaffoldConfig {
        ScaffoldConfig {
            name: path.to_string_lossy().to_string(),
            minimal,
            force,
        }
    }

    #[test]
    fn full_scaffold_creates_all_files() {
        let p = tmp_dir("full");
        let res = generate_project(&cfg(&p, false, false)).unwrap();
        assert!(p.join("Cargo.toml").exists());
        assert!(p.join("src/lib.rs").exists());
        assert!(p.join(".sdkt.toml").exists());
        assert!(p.join("README.md").exists());
        assert!(p.join(".gitignore").exists());
        assert!(p.join("tests/basic.rs").exists());
        assert_eq!(res.files_created.len(), 6);
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn minimal_scaffold_omits_extras() {
        let p = tmp_dir("minimal");
        let res = generate_project(&cfg(&p, true, false)).unwrap();
        assert!(p.join("Cargo.toml").exists());
        assert!(p.join("src/lib.rs").exists());
        assert!(p.join(".sdkt.toml").exists());
        assert!(!p.join("README.md").exists());
        assert!(!p.join(".gitignore").exists());
        assert!(!p.join("tests").exists());
        assert_eq!(res.files_created.len(), 3);
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn rejects_existing_dir() {
        let p = tmp_dir("exists");
        fs::create_dir_all(&p).unwrap();
        let err = generate_project(&cfg(&p, false, false)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn force_overwrites_existing() {
        let p = tmp_dir("force");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("user_file.txt"), "keep me").unwrap();
        let res = generate_project(&cfg(&p, false, true)).unwrap();
        assert!(res.files_created.len() >= 6);
        // User file not deleted
        assert!(p.join("user_file.txt").exists());
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn cargo_toml_has_no_std_profile() {
        let p = tmp_dir("profile");
        generate_project(&cfg(&p, true, false)).unwrap();
        let content = fs::read_to_string(p.join("Cargo.toml")).unwrap();
        assert!(content.contains("[profile.release]"));
        assert!(content.contains("panic = \"abort\""));
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn lib_rs_is_no_std() {
        let p = tmp_dir("nostd");
        generate_project(&cfg(&p, true, false)).unwrap();
        let content = fs::read_to_string(p.join("src/lib.rs")).unwrap();
        assert!(content.contains("#![no_std]"));
        assert!(content.contains("soroban_sdk"));
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn hyphenated_name_produces_valid_crate() {
        let p = tmp_dir("hyphen-test");
        generate_project(&cfg(&p, false, false)).unwrap();
        let test_content = fs::read_to_string(p.join("tests/basic.rs")).unwrap();
        // Rust crate names use underscores
        assert!(test_content.contains("sdkt_test_hyphen_test"));
        let _ = fs::remove_dir_all(&p);
    }

    fn pcfg(path: &Path, force: bool) -> PluginScaffoldConfig {
        PluginScaffoldConfig {
            name: path.to_string_lossy().to_string(),
            force,
        }
    }

    /// Temp dir whose *basename* is exactly `name` (the plugin scaffold derives
    /// the package name from the directory name, so a `sdkt_test_` prefix would
    /// leak into the generated crate).
    fn tmp_plugin(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn plugin_scaffold_creates_documented_layout() {
        let p = tmp_dir("plugin-layout");
        let res = generate_plugin_project(&pcfg(&p, false)).unwrap();
        for f in PLUGIN_SCAFFOLD_FILES {
            assert!(p.join(f).exists(), "missing scaffolded file: {f}");
        }
        assert_eq!(
            res.files_created, PLUGIN_SCAFFOLD_FILES,
            "scaffolded file set must match the documented layout"
        );
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_scaffold_derives_rule_id_from_name() {
        let p = tmp_plugin("plugin-rule-id");
        generate_plugin_project(&pcfg(&p, false)).unwrap();
        let lib = fs::read_to_string(p.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("PLUGIN-RULE-ID-001"),
            "rule id derived from project name"
        );
        assert!(lib.contains("pub struct PluginRuleId;"));
        let abi = fs::read_to_string(p.join("src/plugin_abi.rs")).unwrap();
        assert!(abi.contains("PLUGIN-RULE-ID-001\\0"));
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_scaffold_wires_standalone_cargo_toml() {
        let p = tmp_plugin("plugin-cargo");
        generate_plugin_project(&pcfg(&p, false)).unwrap();
        let cargo = fs::read_to_string(p.join("Cargo.toml")).unwrap();
        // Standalone crate: must opt out of any enclosing workspace.
        assert!(cargo.contains("[workspace]"));
        assert!(cargo.contains("sdkt-audit"));
        assert!(cargo.contains("plugins = [\"sdkt-audit/plugins\"]"));
        assert!(cargo.contains("crate-type = [\"rlib\", \"cdylib\"]"));
        assert!(cargo.contains("name = \"plugin_cargo\""));
        let toml = fs::read_to_string(p.join("plugin/plugin.toml")).unwrap();
        assert!(toml.contains("id = \"plugin_cargo\""));
        let artifact = format!(
            "{}{}{}",
            std::env::consts::DLL_PREFIX,
            "plugin_cargo",
            std::env::consts::DLL_SUFFIX
        );
        assert!(toml.contains(&format!("artifact = \"{artifact}\"")));
        // WASM manifest mirrors the native one but with kind/extension matched.
        let wasm = fs::read_to_string(p.join("plugin-wasm/plugin.toml")).unwrap();
        assert!(wasm.contains("kind = \"wasm\""));
        assert!(wasm.contains("artifact = \"plugin_cargo.wasm\""));
        assert!(wasm.contains("abi_major = 1"));
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_scaffold_rejects_existing_dir() {
        let p = tmp_dir("plugin-exists");
        fs::create_dir_all(&p).unwrap();
        let err = generate_plugin_project(&pcfg(&p, false)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_scaffold_force_overwrites_existing() {
        let p = tmp_dir("plugin-force");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("user_file.txt"), "keep me").unwrap();
        let res = generate_plugin_project(&pcfg(&p, true)).unwrap();
        assert!(p.join("Cargo.toml").exists());
        // User file not deleted
        assert!(p.join("user_file.txt").exists());
        assert_eq!(res.files_created.len(), PLUGIN_SCAFFOLD_FILES.len());
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_scaffold_rejects_invalid_names() {
        let p = tmp_plugin("plugin bad name");
        let err = generate_plugin_project(&pcfg(&p, false)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // First char must be a letter.
        let p2 = tmp_plugin("9bad");
        assert!(generate_plugin_project(&pcfg(&p2, false)).is_err());
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn plugin_struct_name_avoids_reserved_identifier() {
        // `self` must not yield `pub struct Self;` (Self is a Rust keyword).
        assert_eq!(plugin_struct_name("self"), "PluginSelf");
        // All other names keep their existing derived identifier.
        assert_eq!(plugin_struct_name("self-rule"), "SelfRule");
        assert_eq!(plugin_struct_name("my-rule"), "MyRule");
        assert_eq!(plugin_struct_name("snake_case_rule"), "SnakeCaseRule");
    }

    #[test]
    fn plugin_scaffold_readme_documents_workflow() {
        let p = tmp_dir("plugin-readme");
        generate_plugin_project(&pcfg(&p, false)).unwrap();
        let readme = fs::read_to_string(p.join("README.md")).unwrap();
        assert!(readme.contains("cargo build --release --features plugins"));
        assert!(readme.contains("sdkt plugin pack"));
        assert!(readme.contains("sdkt plugin install"));
        assert!(readme.contains("sdkt audit path/to/contract.rs --rules"));
        let _ = fs::remove_dir_all(&p);
    }
}
