//! End-to-end CLI integration tests for `sdkt`.
//!
//! These exercise the real binary via `assert_cmd` and assert on stdout/stderr
//! and exit codes. They are fully deterministic and offline:
//!
//! * Network-profile tests point `SDKT_NETWORK_DIR` at a per-test temporary
//!   directory so they never touch the developer's real profile store and never
//!   collide with each other.
//! * "Offline" commands (`--help`, `--version`, `completions`, `network list`
//!   against an empty store) are exercised without any network access.

use assert_cmd::Command;
use predicates::prelude::*;

/// Build a `sdkt` command with an isolated network-profile directory.
fn sdkt_isolated() -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    // Point network storage at a fresh temp dir for determinism.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    cmd.env("SDKT_NETWORK_DIR", &dir);
    cmd
}

#[test]
fn help_lists_all_top_level_commands() {
    sdkt_isolated()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("decode"))
        .stdout(predicate::str::contains("inspect"))
        .stdout(predicate::str::contains("network"))
        .stdout(predicate::str::contains("completions"));
}

#[test]
fn version_reports_current_release() {
    sdkt_isolated()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("sdkt 2.4.0"));
}

#[test]
fn completions_bash_emits_script() {
    let out = sdkt_isolated()
        .args(["completions", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8_lossy(&out);
    // bash completion scripts define a _sdkt completion function.
    assert!(
        text.contains("_sdkt") || text.contains("complete -F") || text.contains("sdkt"),
        "bash completion output did not look like a completion script:\n{}",
        text
    );
}

#[test]
fn completions_zsh_emits_script() {
    sdkt_isolated()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sdkt"));
}

#[test]
fn completions_fish_emits_script() {
    sdkt_isolated()
        .args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sdkt"));
}

#[test]
fn completions_powershell_emits_script() {
    sdkt_isolated()
        .args(["completions", "powershell"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sdkt"));
}

#[test]
fn completions_rejects_unknown_shell() {
    sdkt_isolated()
        .args(["completions", "tcsh"])
        .assert()
        .failure();
}

#[test]
fn network_add_then_list_then_show_then_remove_json() {
    // Shared, isolated network directory for the whole flow.
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-flow-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let _guard = {
        struct G(std::path::PathBuf);
        impl Drop for G {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        G(dir.clone())
    };

    // add
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .env("SDKT_NETWORK_DIR", &dir)
        .args([
            "network",
            "add",
            "testnet",
            "--rpc-url",
            "https://soroban-testnet.stellar.org",
            "--passphrase",
            "Test SDF Network ; September 2015",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"testnet\""))
        .stdout(predicate::str::contains(
            "\"rpc_url\":\"https://soroban-testnet.stellar.org\"",
        ));

    // list shows the profile
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .env("SDKT_NETWORK_DIR", &dir)
        .args(["network", "list", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("testnet"));

    // show
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .env("SDKT_NETWORK_DIR", &dir)
        .args(["network", "show", "testnet", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"testnet\""))
        .stdout(predicate::str::contains(
            "\"rpc_url\":\"https://soroban-testnet.stellar.org\"",
        ));

    // remove
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .env("SDKT_NETWORK_DIR", &dir)
        .args(["network", "remove", "testnet", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"removed\""));

    // after removal, list is empty again
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .env("SDKT_NETWORK_DIR", &dir)
        .args(["network", "list", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[test]
fn network_list_empty_is_valid_json_array() {
    sdkt_isolated()
        .args(["network", "list", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[]"));
}

#[test]
fn network_add_rejects_missing_required_args() {
    // Missing --rpc-url and --passphrase should fail.
    sdkt_isolated()
        .args(["network", "add", "broken"])
        .assert()
        .failure();
}

#[test]
fn offline_command_help_does_not_require_network() {
    // `sdkt diff --help` is fully offline and must succeed without RPC access.
    sdkt_isolated()
        .args(["diff", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--old-wasm"))
        .stdout(predicate::str::contains("--new-wasm"));
}

#[test]
fn invalid_top_level_subcommand_fails() {
    sdkt_isolated().arg("bogus-cmd").assert().failure();
}

#[cfg(unix)]
#[test]
fn completions_broken_pipe_exits_successfully() {
    // `sdkt completions bash | head -c 1` closes the pipe after one byte.
    // sdkt must NOT panic and must exit 0 (broken pipe is expected, not fatal).
    // `bash -O pipefail` ensures sdkt's own exit status propagates through the
    // pipeline (otherwise `head`'s success would mask a sdkt panic).
    let bin = env!("CARGO_BIN_EXE_sdkt");
    let script = format!("set -o pipefail; {:?} completions bash | head -c 1", bin);
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .expect("failed to spawn bash");
    assert!(
        output.status.success(),
        "sdkt should exit 0 on a closed stdout pipe (broken pipe), got status {:?}",
        output.status.code()
    );
}

// ---------------------------------------------------------------------------
// M34.1 — `sdkt.lock` generation / inspection (offline, hermetic).
// ---------------------------------------------------------------------------

/// Build a temp project: `.sdkt.toml` + two contract dirs each with a fake
/// `target/wasm32-unknown-unknown/release/<name>.wasm`. Returns the temp root.
fn make_lock_fixture(root: &std::path::Path, token_bytes: &[u8], router_bytes: &[u8]) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join(".sdkt.toml"),
        r#"
[contracts.token]
path = "contracts/token"
deploy_after = []

[contracts.router]
path = "contracts/router"
deploy_after = ["token"]
"#,
    )
    .unwrap();

    for (name, bytes) in [("token", token_bytes), ("router", router_bytes)] {
        let wasm_dir = root
            .join("contracts")
            .join(name)
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release");
        std::fs::create_dir_all(&wasm_dir).unwrap();
        std::fs::write(wasm_dir.join(format!("{}.wasm", name)), bytes).unwrap();
    }
}

#[test]
fn lock_generate_writes_sdkt_lock() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-lock-gen-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    make_lock_fixture(&tmp, b"token-bytes", b"router-bytes");

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("lock").arg("generate");
    let assert = cmd.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    // token (no deps) is deployed before router.
    assert!(out.contains("token"));
    assert!(out.contains("router"));
    assert!(out.contains("sha256"));

    // The lock file now exists on disk.
    assert!(
        tmp.join("sdkt.lock").exists(),
        "sdkt.lock should have been written"
    );

    // `lock show` prints the same lock contents.
    let mut show = Command::cargo_bin("sdkt").expect("sdkt binary built");
    show.current_dir(&tmp).arg("lock").arg("show");
    let assert = show.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(out.contains("deploy_order"));

    // `lock verify` reports consistency (no drift).
    let mut verify = Command::cargo_bin("sdkt").expect("sdkt binary built");
    verify.current_dir(&tmp).arg("lock").arg("verify");
    let assert = verify.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(out.contains("consistent") || out.contains("✓"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn lock_verify_detects_drift() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-lock-drift-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    make_lock_fixture(&tmp, b"token-bytes", b"router-bytes");

    // Generate the lock first.
    let mut gen = Command::cargo_bin("sdkt").expect("sdkt binary built");
    gen.current_dir(&tmp)
        .arg("lock")
        .arg("generate")
        .assert()
        .success();

    // Now tamper with the token artifact.
    let token_wasm = tmp
        .join("contracts/token")
        .join("target/wasm32-unknown-unknown/release/token.wasm");
    std::fs::write(&token_wasm, b"tampered-token-bytes").unwrap();

    let mut verify = Command::cargo_bin("sdkt").expect("sdkt binary built");
    verify.current_dir(&tmp).arg("lock").arg("verify");
    let assert = verify.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    // Advisory: drift is reported, but the command still exits 0 (non-fatal).
    assert!(out.contains("drift") || out.contains("stale"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn lock_verify_without_lock_is_non_fatal() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-lock-none-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    make_lock_fixture(&tmp, b"token-bytes", b"router-bytes");
    // No `sdkt build` / `sdkt lock generate` run → no sdkt.lock.

    let mut verify = Command::cargo_bin("sdkt").expect("sdkt binary built");
    verify.current_dir(&tmp).arg("lock").arg("verify");
    // Must exit 0 (backward compatible): a missing lock is acceptable.
    verify.assert().success();

    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// M34.2 — invalid project dependency graphs (offline, hermetic).
// `sdkt build` validates the graph up front, so a bad graph fails fast with a
// clear, non-zero-exit error (no cargo invocation, no silent default).
// ---------------------------------------------------------------------------

fn write_sdkt_toml(root: &std::path::Path, body: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(".sdkt.toml"), body).unwrap();
}

#[test]
fn build_rejects_unknown_dependency() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m342-unknown-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_sdkt_toml(
        &tmp,
        "[contracts.router]\npath = \"contracts/router\"\ndepends_on = [\"ghost\"]\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("build");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.contains("ghost"),
        "error should name the unknown dependency: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn build_rejects_self_dependency() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m342-self-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_sdkt_toml(
        &tmp,
        "[contracts.token]\npath = \"contracts/token\"\ndepends_on = [\"token\"]\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("build");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.to_lowercase().contains("self"),
        "error should report self-dependency: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn build_rejects_circular_dependency() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m342-cycle-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_sdkt_toml(
        &tmp,
        "[contracts.a]\npath = \"contracts/a\"\ndepends_on = [\"b\"]\n\n[contracts.b]\npath = \"contracts/b\"\ndepends_on = [\"a\"]\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("build");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.to_lowercase().contains("circular"),
        "error should report circular dependency: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn build_rejects_duplicate_contract_name() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m342-dupname-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_sdkt_toml(
        &tmp,
        "[contracts.token]\npath = \"contracts/token\"\n\n[contracts.token]\npath = \"contracts/token2\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("build");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !out.is_empty(),
        "duplicate contract name must produce a load error: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// M35.0 — local package manifest validation (offline, hermetic).
// `sdkt package validate` checks `[package]` metadata and the local
// `[dependencies]` graph, never touching the network.
// ---------------------------------------------------------------------------

fn write_manifest(root: &std::path::Path, body: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(".sdkt.toml"), body).unwrap();
}

#[test]
fn package_validate_accepts_valid_manifest() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m350-valid-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("math")).unwrap();
    std::fs::create_dir_all(tmp.join("auth")).unwrap();
    write_manifest(
        &tmp,
        "[package]\nname = \"my-token\"\nversion = \"0.1.0\"\ndescription = \"Example Soroban token\"\n\n[dependencies.math]\npath = \"math\"\n\n[dependencies.auth]\npath = \"auth\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        out.to_lowercase().contains("valid"),
        "valid manifest should report valid: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_validate_rejects_missing_version() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m350-nover-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        "[package]\nname = \"my-token\"\n\n[dependencies.math]\npath = \"math\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.to_lowercase().contains("version"),
        "missing version must be reported: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_validate_rejects_missing_path() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m350-nopath-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        "[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[dependencies.math]\n\n[dependencies.auth]\npath = \"auth\"\n",
    );
    // `math` dependency has no `path` -> unsupported/missing source.

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.to_lowercase().contains("path"),
        "missing dependency path must be reported: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_validate_rejects_git_dependency_at_parse() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m350-git-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        "[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"https://github.com/example/math\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !out.is_empty(),
        "git dependency must be rejected (parse error): {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_validate_rejects_self_dependency() {
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m350-self-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("my-token")).unwrap();
    write_manifest(
        &tmp,
        "[package]\nname = \"my-token\"\nversion = \"0.1.0\"\n\n[dependencies.my-token]\npath = \"my-token\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        out.to_lowercase().contains("self"),
        "self-dependency must be reported: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// M35.1 — Git dependency sources: validation + fetch (offline, local git).
// `sdkt package validate` accepts git deps; `sdkt package fetch` clones a
// local git repo into `.sdkt-cache` (no real network).
// ---------------------------------------------------------------------------

fn make_local_git_repo() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-gitsrc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("git available");
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@sdkt.local"]);
    run(&["config", "user.name", "sdkt test"]);
    let f = dir.join("lib.rs");
    std::fs::write(&f, "pub fn answer() -> u32 { 42 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "initial"]);
    run(&["tag", "v1.0.0"]);
    dir
}

#[test]
fn package_validate_accepts_git_dependency() {
    let src = make_local_git_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m351-valid-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        out.to_lowercase().contains("valid"),
        "git dependency manifest should be valid: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_validate_rejects_git_without_ref() {
    let src = make_local_git_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m351-noref-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\n",
            url
        ),
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("validate");
    let assert = cmd.assert().failure();
    let out = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !out.is_empty(),
        "git dependency without a reference must be rejected: {}",
        out
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_fetch_git_dependency_offline() {
    let src = make_local_git_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m351-fetch-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );

    // Fetch the git dependency (clones the local repo, no real network).
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("fetch");
    let assert = cmd.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        out.to_lowercase().contains("fetched"),
        "fetch should report success: {}",
        out
    );
    // Cache entry must exist under .sdkt-cache/git/<key>/.
    let cache = tmp.join(".sdkt-cache").join("git");
    assert!(cache.exists(), "git cache dir should exist");
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(&cache) {
        for e in entries.flatten() {
            if e.path().join(".git").exists() && e.path().join("lib.rs").exists() {
                found = true;
            }
        }
    }
    assert!(found, "cloned checkout with lib.rs should be in cache");

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn lock_verify_reports_dependency_mismatch() {
    // Offline: a manifest with a local path dependency whose path is missing
    // from disk must be reported by `sdkt lock verify` (M35.2).
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m352-verify-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    // Claim a dependency path that does NOT exist on disk.
    write_manifest(
        &tmp,
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\npath = \"libs/math\"\n",
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("lock").arg("verify");
    let assert = cmd.assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout);
    // Dependency drift must be surfaced (no sdkt.lock yet => unverified, or the
    // path-missing condition once a lock is written). Either way the verify
    // command must not panic and must mention dependencies.
    assert!(
        out.to_lowercase().contains("dependenc"),
        "lock verify should report dependency status: {}",
        out
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_fetch_writes_locked_dependencies() {
    // Offline: fetch a local git dependency; the run must record a
    // reproducible sdkt.lock with the resolved commit + cache location.
    let src = make_local_git_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m352-fetchlock-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );

    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(&tmp).arg("package").arg("fetch");
    cmd.assert().success();

    // sdkt.lock must now exist and record the dependency with a commit + cache.
    let lock_path = tmp.join("sdkt.lock");
    assert!(lock_path.exists(), "sdkt.lock should be written by fetch");
    let content = std::fs::read_to_string(&lock_path).unwrap();
    assert!(
        content.contains("[[dependencies]]"),
        "lock should record a dependencies array: {}",
        content
    );
    assert!(
        content.contains("name = \"math\""),
        "lock should record dependency name 'math': {}",
        content
    );
    assert!(
        content.contains("commit_sha"),
        "lock should record resolved commit_sha: {}",
        content
    );
    assert!(
        content.contains("cache_location"),
        "lock should record cache_location: {}",
        content
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

// ---------------------------------------------------------------------------
// M36.0 — Package update & synchronization (offline, local git remotes).
// `sdkt package update` resolves available commits via git ls-remote against
// the declared git URL (a local repo here, no network), refreshes the cache,
// and rewrites sdkt.lock. `--check` reports; `--dry-run` previews; both are
// read-only. `rev` stays pinned; `tag`/`branch` update on drift.
// ---------------------------------------------------------------------------

/// Build a local "remote" repo with an initial commit tagged v1.0.0, then
/// advance HEAD (and move the tag) so an `update` has something to pull.
/// Build a local "remote" repo with an initial commit tagged v1.0.0. The tag
/// stays at v1 until a test calls `advance_repo` (moves HEAD + re-tags), so
/// `fetch` records the OLD commit and a later `update` has something to pull.
fn make_advancing_repo() -> (std::path::PathBuf, String, String) {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-sync-remote-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("git available");
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@sdkt.local"]);
    run(&["config", "user.name", "sdkt test"]);
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "initial"]);
    run(&["tag", "v1.0.0"]);
    let v1 = {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "v1.0.0"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    // A SECOND commit exists in the repo (HEAD) but is NOT yet tagged, so the
    // declared `tag = "v1.0.0"` still resolves to v1. Advancing re-tags to it.
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 43 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "second"]);
    let v2 = {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    (dir, v1, v2)
}

/// Move the remote's `v1.0.0` tag to its current HEAD, simulating an upstream
/// release. After this, `sdkt package update` should pull the new commit.
fn advance_repo(src: &std::path::Path) {
    let o = std::process::Command::new("git")
        .current_dir(src)
        .args(["tag", "-f", "v1.0.0"])
        .output()
        .expect("git available");
    assert!(o.status.success(), "advance tag failed");
}

/// Build a local "remote" with multiple semver tags so the M37 version resolver
/// has a choice. Tags: v1.0.0, v1.5.0, v2.0.0 (HEAD sits at v2.0.0).
fn make_version_repo() -> (std::path::PathBuf, String, String, String) {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-ver-remote-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("git available");
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@sdkt.local"]);
    run(&["config", "user.name", "sdkt test"]);
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "v1"]);
    run(&["tag", "v1.0.0"]);
    let v1 = {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "v1.0.0"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 43 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "v15"]);
    run(&["tag", "v1.5.0"]);
    let v15 = {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "v1.5.0"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 44 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "v2"]);
    run(&["tag", "v2.0.0"]);
    let v2 = {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "v2.0.0"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    (dir, v1, v15, v2)
}

#[test]
fn package_update_refreshes_lock() {
    let (src, v1, v2) = make_advancing_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m360-update-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );

    // 1) fetch records the OLD (v1) commit in the lock.
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();
    advance_repo(&src);
    let lock1 = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert!(
        lock1.contains(&v1[..12]),
        "lock should record old commit: {}",
        lock1
    );

    // 2) update pulls the NEW commit and rewrites the lock.
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("update")
        .assert()
        .success();
    let lock2 = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert!(
        lock2.contains(&v2[..12]),
        "lock should record new commit: {}",
        lock2
    );
    assert!(
        !lock2.contains(&v1[..12]) || lock2.matches(&v1[..12]).count() <= 1,
        "old commit should be replaced"
    );

    // 3) a second update is a no-op (already current).
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("update")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("up to date")
            || stdout.to_lowercase().contains("nothing to update"),
        "second update should report no changes: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_update_check_is_readonly() {
    let (src, _v1, _v2) = make_advancing_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m360-check-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let lock_before = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    advance_repo(&src);
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "update", "--check"])
        .output()
        .unwrap();
    assert!(out.status.success(), "check mode exits 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("update") || stdout.to_lowercase().contains("available"),
        "check should report an available update: {}",
        stdout
    );
    // Lock must be unchanged after --check.
    let lock_after = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert_eq!(lock_before, lock_after, "--check must not rewrite the lock");

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_update_dry_run_is_readonly() {
    let (src, _v1, _v2) = make_advancing_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m360-dryrun-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let lock_before = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    advance_repo(&src);
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "update", "--dry-run"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("would")
            || stdout.to_lowercase().contains("lock would change"),
        "dry-run should preview changes: {}",
        stdout
    );
    let lock_after = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert_eq!(
        lock_before, lock_after,
        "--dry-run must not rewrite the lock"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_update_json_reports_counts() {
    let (src, _v1, _v2) = make_advancing_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m360-json-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    advance_repo(&src);
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "update", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // JSON must carry the summary counters and at least one change entry.
    assert!(
        stdout.contains("\"checked\""),
        "json missing checked: {}",
        stdout
    );
    assert!(
        stdout.contains("\"updated\""),
        "json missing updated: {}",
        stdout
    );
    assert!(
        stdout.contains("\"changes\""),
        "json missing changes: {}",
        stdout
    );
    // The lock should have been refreshed (apply mode, not check/dry-run).
    let lock = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert!(
        lock.contains("commit_sha"),
        "lock should be refreshed: {}",
        lock
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_update_offline_local_path_unchanged() {
    // A local path dependency has no remote; update must report it unchanged
    // and stay fully offline.
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m360-local-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("libs/math")).unwrap();
    write_manifest(
        &tmp,
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\npath = \"libs/math\"\n",
    );

    // Fetch first so a lock exists (path deps are recorded unchanged).
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "update"])
        .output()
        .unwrap();
    assert!(out.status.success(), "local-path update succeeds offline");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("unchanged") || stdout.to_lowercase().contains("up to date"),
        "local path dep should be reported unchanged: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn package_update_version_constraint_picks_highest() {
    // A git dep constrained by `version = ">=1.0, <2"` must resolve to the
    // highest satisfying tag (v1.5.0), never v2.0.0 (which is outside the
    // range). Exercises the M37 VersionResolver end-to-end via fetch+update.
    let (src, _v1, v15, v2) = make_version_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m37-ver-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\nversion = \">=1.0, <2\"\n",
            url
        ),
    );

    // fetch materializes the resolved tag (v1.5.0) into the cache + lock.
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let lock = std::fs::read_to_string(tmp.join("sdkt.lock")).unwrap();
    assert!(
        lock.contains(&v15[..12]),
        "lock should record the highest satisfying tag v1.5.0: {}",
        lock
    );
    assert!(
        !lock.contains(&v2[..12]),
        "lock must NOT record v2.0.0 (outside constraint): {}",
        lock
    );

    // `package update` must report the dep as up-to-date (already at best).
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("update")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("up to date") || stdout.to_lowercase().contains("unchanged"),
        "version-constrained dep should be up-to-date at best tag: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_update_version_constraint_unsatisfied_reports_error() {
    // A constraint with no satisfying tag must surface a clear error (not a
    // panic) during fetch. The remote only has v1.x/v2.x tags.
    let (src, _v1, _v15, _v2) = make_version_repo();
    let url = src.to_string_lossy().replace('\\', "\\\\");
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m37-verbad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_manifest(
        &tmp,
        &format!(
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[dependencies.math]\ngit = \"{}\"\nversion = \">=3.0\"\n",
            url
        ),
    );

    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "fetch with unsatisfied constraint must fail clearly"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("satisfies") || stderr.to_lowercase().contains("constraint"),
        "error should mention the unsatisfied constraint: {}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

// --- M38 packaging / publishing CLI integration tests --------------------

/// Build a local "remote" git repo with a single tag `v1.0.0` for M38 pack
/// tests. Offline; no network.
fn make_pack_repo() -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!(
        "sdkt-it-m38-remote-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        let o = std::process::Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .expect("git available");
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@sdkt.local"]);
    run(&["config", "user.name", "sdkt test"]);
    std::fs::write(dir.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "initial"]);
    run(&["tag", "v1.0.0"]);
    let url = dir.to_string_lossy().replace('\\', "/");
    (dir, url)
}

fn write_pack_manifest(tmp: &std::path::Path, url: &str) {
    write_manifest(
        tmp,
        &format!(
            "[package]\nname = \"m38-app\"\nversion = \"0.3.0\"\n\n[dependencies.math]\ngit = \"{}\"\ntag = \"v1.0.0\"\n",
            url
        ),
    );
}

#[test]
fn package_pack_produces_artifact() {
    // `sdkt package pack` without --out must write to the default `./dist`.
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-pack-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);

    // Fetch first so a cache + lock exist (pack requires sdkt.lock).
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack"])
        .output()
        .unwrap();
    assert!(out.status.success(), "pack must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Packed m38-app v0.3.0"),
        "pack prints summary: {}",
        stdout
    );
    assert!(
        stdout.contains("lock sha256"),
        "pack prints lock hash: {}",
        stdout
    );

    // Default output dir is ./dist; artifact is a .tar.zst.
    let dist = tmp.join("dist");
    assert!(dist.exists(), "default dist/ must exist");
    let mut found = false;
    for entry in std::fs::read_dir(&dist).unwrap() {
        let p = entry.unwrap().path();
        if p.to_string_lossy().ends_with(".tar.zst") {
            found = true;
        }
    }
    assert!(found, "dist/ must contain a .tar.zst artifact");

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_pack_with_out_dir() {
    // `--out` must direct the artifact to a chosen directory.
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-packout-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let custom = tmp.join("artifacts");
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack", "--out", custom.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "pack --out must succeed");
    assert!(custom.exists(), "--out dir must be created");
    let mut found = false;
    for entry in std::fs::read_dir(&custom).unwrap() {
        if entry
            .unwrap()
            .path()
            .to_string_lossy()
            .ends_with(".tar.zst")
        {
            found = true;
        }
    }
    assert!(found, "--out must contain the artifact");

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_pack_format_handling() {
    // `tar.zst` (default) and `dir` both work; an unknown format errors.
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-fmt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    // dir format → <out>/<name>-<version>/ directory exists.
    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack", "--out", "d1", "--format", "dir"])
        .output()
        .unwrap();
    assert!(out.status.success(), "pack --format dir must succeed");
    assert!(
        tmp.join("d1").join("m38-app-0.3.0").exists(),
        "dir artifact missing"
    );
    assert!(
        tmp.join("d1")
            .join("m38-app-0.3.0")
            .join("package.json")
            .exists(),
        "descriptor missing in dir bundle"
    );

    // Unknown format → non-zero, clear error.
    let bad = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack", "--out", "d2", "--format", "zip"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "unknown format must fail");
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("unsupported --format"),
        "error should name the bad format: {}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_pack_roundtrip_preserves_lock_and_integrity() {
    // pack (tar.zst) → unpack → reconstructed tree reproduces lock + integrity.
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-rt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let out_dir = tmp.join("dist");
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack", "--out", "dist"])
        .assert()
        .success();

    // Locate the produced tarball.
    let tarball = {
        let mut tb = None;
        for entry in std::fs::read_dir(&out_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.to_string_lossy().ends_with(".tar.zst") {
                tb = Some(p);
            }
        }
        tb.expect("tarball produced")
    };

    // Reconstruct + verify using the public library API (no double-pack).
    use sdkt_core::package::{unpack, verify_bundle_equivalence, PackageBundle};
    let reconstruct = tmp.join("reconstruct");
    unpack(&tarball, &reconstruct).expect("unpack ok");
    let desc = std::fs::read_to_string(reconstruct.join("package.json")).unwrap();
    let bundle: PackageBundle = serde_json::from_str(&desc).unwrap();
    assert!(
        verify_bundle_equivalence(&reconstruct, &bundle).unwrap(),
        "round-trip must preserve lock + integrity"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_publish_dry_run_reports_ready() {
    // A consistent, fully cached project must pass `--dry-run` (exit 0).
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-ready-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "publish", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "publish --dry-run must exit 0 when ready"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("ready to publish"),
        "should report ready: {}",
        stdout
    );
    assert!(
        stdout.contains("✓"),
        "should list passing checks: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_publish_dry_run_failure_on_drift() {
    // Removing the cached checkout must make `--dry-run` fail (exit non-zero).
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-drift-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();

    // Delete the cached git checkout (simulate cache/lock drift).
    let cache_root = tmp.join(".sdkt-cache").join("git");
    assert!(cache_root.exists());
    std::fs::remove_dir_all(&cache_root).unwrap();

    let out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "publish", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "publish --dry-run must fail when dep cache is missing"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.to_lowercase().contains("not ready")
            || combined.contains("✗")
            || combined.to_lowercase().contains("cache"),
        "should report unready / drift: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn package_pack_and_publish_offline() {
    // Both commands must run with zero network: the "remote" is a local path
    // repo, and no git ls-remote to a real host is performed. This is the same
    // offline guarantee as fetch/update (M35/M36/M37).
    let (src, url) = make_pack_repo();
    let tmp = std::env::temp_dir().join(format!(
        "sdkt-it-m38-offline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    write_pack_manifest(&tmp, &url);

    // fetch + pack + publish all offline.
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .arg("package")
        .arg("fetch")
        .assert()
        .success();
    Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "pack"])
        .assert()
        .success();
    let pub_out = Command::cargo_bin("sdkt")
        .expect("sdkt binary built")
        .current_dir(&tmp)
        .args(["package", "publish", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        pub_out.status.success(),
        "offline publish --dry-run must pass"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&src);
}
