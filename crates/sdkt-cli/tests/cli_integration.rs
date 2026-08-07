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
