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
soroban-sdk = "20.0.0"

[dev-dependencies]
soroban-sdk = {{ version = "20.0.0", features = ["testutils"] }}

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
            "# {name}\n\nA Soroban smart contract project.\n\n## Build\n\n```\ncargo build --target wasm32-unknown-unknown --release\n```\n\n## Test\n\n```\ncargo test\n```\n",
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
}
