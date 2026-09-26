//! — CLI plugin subcommand integration tests (hermetic, temp store).
//!
//! Uses a temp directory as the plugin store via `SDKT_PLUGIN_DIR` so it never
//! touches the developer's real profile. Verifies the local lifecycle
//! (install/list/show/remove) and that `sdkt audit --rules <id>` resolves a
//! plugin id to its artifact (proven by the loader branch it hits, not by a
//! real load which requires the `wasm-plugins`/`plugins` feature).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn sdkt() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    let dir = std::env::temp_dir().join(format!(
        "sdkt-cli-plugin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dir);
    cmd.env("SDKT_PLUGIN_DIR", &dir);
    // Avoid any real network profile lookups during these offline tests.
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

/// A dummy contract source file so `sdkt audit <path>` reaches the rule-loading
/// stage (the source content is irrelevant for plugin-resolution assertions).
fn dummy_src(store: &std::path::Path) -> std::path::PathBuf {
    let p = store.join("contract.rs");
    fs::write(&p, "pub fn hello() {}\n").unwrap();
    p
}

fn fixture_plugin(store: &std::path::Path) -> std::path::PathBuf {
    // Dummy artifact (not a real loadable plugin; default build skips dry-run load).
    let src = store.join("ex_rule.wasm");
    fs::write(&src, b"dummy-wasm-bytes").unwrap();
    fs::write(
        store.join("plugin.toml"),
        r#"
id = "example-rule"
name = "Example Rule"
version = "1.0.0"
author = "SaboLabs"
description = "Reference audit rule."
kind = "wasm"
artifact = "ex_rule.wasm"
abi_major = 1
abi_minor = 0
"#,
    )
    .unwrap();
    src
}

#[test]
fn plugin_install_list_show_remove_lifecycle() {
    let store = std::env::temp_dir().join(format!(
        "sdkt-plugin-life-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&store);
    let src = fixture_plugin(&store);

    // list empty
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No plugins installed"));

    // install
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["plugin", "install", src.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed plugin 'example-rule'"));

    // list shows it
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("example-rule"));

    // show
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["plugin", "show", "example-rule"])
        .assert()
        .success()
        .stdout(predicate::str::contains("kind: wasm"));

    // audit --rules <id> resolves the id (default build: hits wasm-plugins feature branch)
    let src = dummy_src(&store);
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["audit", src.to_str().unwrap(), "--rules", "example-rule"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("wasm-plugins"));

    // remove
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["plugin", "remove", "example-rule"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed plugin"));

    // audit --rules <id> now fails as unresolved path
    let src = dummy_src(&store);
    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args(["audit", src.to_str().unwrap(), "--rules", "example-rule"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));

    let _ = fs::remove_dir_all(&store);
}

#[test]
fn plugin_install_with_id_override_uses_effective_identity() {
    let store = tempfile::TempDir::new().unwrap();
    let source = fixture_plugin(store.path());
    let source = source.to_str().unwrap();

    sdkt()
        .env("SDKT_PLUGIN_DIR", store.path())
        .args(["plugin", "install", source, "--id", "override-name"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed plugin 'override-name'"));

    sdkt()
        .env("SDKT_PLUGIN_DIR", store.path())
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("override-name"));

    sdkt()
        .env("SDKT_PLUGIN_DIR", store.path())
        .args(["plugin", "show", "override-name"])
        .assert()
        .success()
        .stdout(predicate::str::contains("id: override-name"));

    let contract = dummy_src(store.path());
    sdkt()
        .env("SDKT_PLUGIN_DIR", store.path())
        .args([
            "audit",
            contract.to_str().unwrap(),
            "--rules",
            "override-name",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("wasm-plugins"));

    sdkt()
        .env("SDKT_PLUGIN_DIR", store.path())
        .args(["plugin", "remove", "override-name"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed plugin"));

    assert!(!store.path().join("override-name").exists());
}

/// Parse a command's stdout as JSON, failing with the raw output if it is not.
fn stdout_json(out: &assert_cmd::assert::Assert) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"))
}

#[test]
fn plugin_json_output_lifecycle() {
    let store = tempfile::TempDir::new().unwrap();
    let src = fixture_plugin(store.path());
    let src = src.to_str().unwrap();
    let run = |args: &[&str]| {
        sdkt()
            .env("SDKT_PLUGIN_DIR", store.path())
            .args(args)
            .assert()
    };

    // An empty store is an empty array, not the "No plugins installed." sentence.
    let empty = stdout_json(&run(&["plugin", "list", "--format", "json"]).success());
    assert_eq!(empty, serde_json::json!([]));

    let installed = stdout_json(&run(&["plugin", "install", src, "--format", "json"]).success());
    assert_eq!(installed["status"], "installed");

    // `show` carries every field the pretty output prints, and `install`
    // reports the same metadata.
    let shown =
        stdout_json(&run(&["plugin", "show", "example-rule", "--format", "json"]).success());
    assert_eq!(
        shown,
        serde_json::json!({
            "id": "example-rule",
            "name": "Example Rule",
            "version": "1.0.0",
            "author": "SaboLabs",
            "description": "Reference audit rule.",
            "kind": "wasm",
            "artifact": "ex_rule.wasm",
            "abi_major": 1,
            "abi_minor": 0,
        })
    );
    assert_eq!(installed["plugin"], shown);

    let listed = stdout_json(&run(&["plugin", "list", "--format", "json"]).success());
    assert_eq!(listed, serde_json::json!([shown]));

    let updated =
        stdout_json(&run(&["plugin", "update", "example-rule", src, "--format", "json"]).success());
    assert_eq!(
        updated,
        serde_json::json!({ "status": "updated", "plugin": shown })
    );

    // Removal stays idempotent: the same result whether or not it was installed.
    let expected_removed = serde_json::json!({ "status": "removed", "id": "example-rule" });
    for _ in 0..2 {
        let removed =
            stdout_json(&run(&["plugin", "remove", "example-rule", "--format", "json"]).success());
        assert_eq!(removed, expected_removed);
    }

    // Errors keep the existing contract: non-zero exit, message on stderr, and
    // nothing on stdout for a JSON parser to misread.
    run(&["plugin", "show", "example-rule", "--format", "json"])
        .failure()
        .stderr(predicate::str::contains("is not installed"))
        .stdout(predicate::str::is_empty());
}

#[test]
fn audit_raw_path_still_works_backward_compat() {
    // A raw existing path must NOT be treated as a plugin id.
    let store = std::env::temp_dir().join(format!(
        "sdkt-plugin-raw-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&store);
    let fake_src = store.join("fake.wasm");
    fs::write(&fake_src, b"not a real plugin").unwrap();
    let src = dummy_src(&store);

    sdkt()
        .env("SDKT_PLUGIN_DIR", &store)
        .args([
            "audit",
            src.to_str().unwrap(),
            "--rules",
            fake_src.to_str().unwrap(),
        ])
        .assert()
        .failure()
        // hits the loader branch for a raw path (not "does not exist")
        .stderr(predicate::str::contains("wasm-plugins"));

    let _ = fs::remove_dir_all(&store);
}
