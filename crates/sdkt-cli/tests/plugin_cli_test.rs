//! M40 — CLI plugin subcommand integration tests (hermetic, temp store).
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
author = "naninu123"
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
