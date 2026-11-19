//! Integration tests for `sdkt generate docs` (issue #559).
//!
//! The workspace tests build a two-contract project whose release target
//! directories hold a real contractspecv0 WASM fixture, so `resolve_project`
//! resolves both aliases without running `sdkt build`. Everything is offline.

use assert_cmd::Command;
use predicates::prelude::*;
use sdkt_wasm::spec::parse_contract_spec;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// A real contractspecv0 WASM fixture (also used by the ABI contract tests).
static CONTRACT_WASM: &[u8] = include_bytes!("fixtures/us_new.wasm");

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Set up a 2-contract project (token ← vault) with real WASM artifacts under
/// each contract's release target directory.
fn setup_two_contract_project(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    fs::write(dir.join(".sdkt.toml"), manifest).unwrap();

    for alias in ["token", "vault"] {
        let target = dir
            .join("contracts")
            .join(alias)
            .join("target")
            .join(sdkt_core::build::WASM_BUILD_TARGET)
            .join("release");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(format!("{alias}.wasm")), CONTRACT_WASM).unwrap();
    }
}

fn function_count() -> usize {
    parse_contract_spec(CONTRACT_WASM)
        .expect("fixture carries a contract spec")
        .functions
        .len()
}

#[test]
fn project_writes_two_pages_and_an_index() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    setup_two_contract_project(dir);
    let out = dir.join("docs").join("api");

    sdkt(dir)
        .current_dir(dir)
        .args(["generate", "docs", "--project", "--out"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("Generated 2 contract page(s)"));

    let token = fs::read_to_string(out.join("token.md")).expect("token.md written");
    let vault = fs::read_to_string(out.join("vault.md")).expect("vault.md written");
    let index = fs::read_to_string(out.join("index.md")).expect("index.md written");

    // Each page carries a one-line title header, then the inspect rendering.
    assert!(token.starts_with("# token\n\n# Contract Interface\n"));
    assert!(vault.starts_with("# vault\n\n# Contract Interface\n"));

    // The index links both aliases with their function counts.
    let count = function_count();
    assert!(
        index.contains(&format!("- [token](token.md) — {count} functions")),
        "{index}"
    );
    assert!(
        index.contains(&format!("- [vault](vault.md) — {count} functions")),
        "{index}"
    );
}

#[test]
fn second_run_without_force_refuses_and_writes_nothing() {
    let project = TempDir::new().unwrap();
    let dir = project.path();
    setup_two_contract_project(dir);
    let out = dir.join("docs").join("api");

    sdkt(dir)
        .current_dir(dir)
        .args(["generate", "docs", "--project", "--out"])
        .arg(&out)
        .assert()
        .success();

    let before_token = fs::read_to_string(out.join("token.md")).unwrap();
    let before_index = fs::read_to_string(out.join("index.md")).unwrap();

    // The first conflict in deploy order is token.md; nothing is written.
    sdkt(dir)
        .current_dir(dir)
        .args(["generate", "docs", "--project", "--out"])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"))
        .stderr(predicate::str::contains("token.md"));

    assert_eq!(
        fs::read_to_string(out.join("token.md")).unwrap(),
        before_token,
        "a refused run must not touch existing pages"
    );
    assert_eq!(
        fs::read_to_string(out.join("index.md")).unwrap(),
        before_index,
        "a refused run must not touch the index"
    );

    // With --force the files are replaced.
    sdkt(dir)
        .current_dir(dir)
        .args(["generate", "docs", "--project", "--force", "--out"])
        .arg(&out)
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(out.join("token.md")).unwrap(),
        before_token
    );
}

#[test]
fn wasm_source_writes_a_page_named_for_the_file_stem() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path();
    let wasm = dir.join("token.wasm");
    fs::write(&wasm, CONTRACT_WASM).unwrap();
    let out = dir.join("docs").join("api");

    sdkt(dir)
        .current_dir(dir)
        .args(["generate", "docs", "--wasm"])
        .arg(&wasm)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let page = fs::read_to_string(out.join("token.md")).expect("token.md written");
    assert!(page.starts_with("# token\n\n# Contract Interface\n\n## Functions\n\n"));
    let index = fs::read_to_string(out.join("index.md")).expect("index.md written");
    assert!(index.contains(&format!(
        "- [token](token.md) — {} functions",
        function_count()
    )));
}

#[test]
fn wasm_and_contract_sources_conflict() {
    let tmp = TempDir::new().unwrap();
    let wasm = tmp.path().join("token.wasm");
    fs::write(&wasm, CONTRACT_WASM).unwrap();

    sdkt(tmp.path())
        .args(["generate", "docs", "--wasm"])
        .arg(&wasm)
        .args([
            "--contract",
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--wasm"))
        .stderr(predicate::str::contains("--contract"));
}

#[test]
fn generate_help_lists_docs() {
    sdkt(Path::new("."))
        .args(["generate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("docs"));
}
