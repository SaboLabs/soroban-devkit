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
