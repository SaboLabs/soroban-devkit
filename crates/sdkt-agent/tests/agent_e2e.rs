//! End-to-end tests: request text → planner → executor → structured result.
//!
//! These drive the real `sdkt-agent` binary against a **fake** `sdkt` on
//! `PATH` (via `SDKT_AGENT_BIN`), so they are hermetic: no network, no live
//! testnet, no real CLI. The fake CLI lets each test dictate stdout, stderr
//! and exit code independently, which is exactly what the agent's contract
//! is about.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use std::process::Output;

/// Write a fake `sdkt` script and return its path.
fn fake_sdkt(dir: &Path, body: &str) -> String {
    let path = dir.join("fake-sdkt.sh");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_string_lossy().into_owned()
}

fn agent(bin: &str, request: &str, json: bool) -> Output {
    let mut cmd = Command::cargo_bin("sdkt-agent").expect("sdkt-agent built");
    cmd.env("SDKT_AGENT_BIN", bin);
    if json {
        cmd.arg("--format").arg("json");
    }
    cmd.arg(request);
    cmd.output().expect("run sdkt-agent")
}

fn parse_json(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}): {stdout:?}"))
}

const CID: &str = "CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV";

// ---------------------------------------------------------------- case A ---
#[test]
fn a_health_request_resolves_and_executes() {
    let tmp = tempfile::tempdir().unwrap();
    // Echo the argv so the test can prove what was actually run.
    let bin = fake_sdkt(
        tmp.path(),
        r#"printf '{"health":"healthy","argv":"%s"}\n' "$*""#,
    );

    let out = agent(
        &bin,
        &format!("check health of contract {CID} on testnet"),
        true,
    );
    let v = parse_json(&out);

    assert_eq!(v["status"], "success");
    assert_eq!(v["capability_id"], "health");
    assert_eq!(v["exit_code"], 0);
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
    assert!(v["evidence"]["parsed_sdkt_result"].as_bool().unwrap());

    // The resolved command must carry the contract, an explicit testnet
    // endpoint, and JSON output — all decided by the planner, not the test.
    let argv = v["argv"].as_array().unwrap();
    let joined = argv
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.starts_with("health --contract "), "{joined}");
    assert!(joined.contains(CID), "{joined}");
    assert!(
        joined.contains("--rpc-url https://soroban-testnet.stellar.org"),
        "{joined}"
    );
    assert!(joined.contains("--format json"), "{joined}");
    assert!(out.status.success());
}

// ---------------------------------------------------------------- case B ---
#[test]
fn b_verify_preserves_exit_code_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    // A mismatch: valid report on stdout, exit 1.
    let bin = fake_sdkt(
        tmp.path(),
        r#"printf '{"match":false,"verification_status":"Mismatch"}\n'
echo "Error: local WASM does NOT match" 1>&2
exit 1"#,
    );

    let out = agent(
        &bin,
        &format!("verify contract {CID} against us_new.wasm on testnet"),
        true,
    );
    let v = parse_json(&out);

    assert_eq!(v["capability_id"], "verify");
    assert_eq!(v["exit_code"], 1, "the CLI's exit code must survive");
    assert_eq!(v["status"], "failed");
    assert_eq!(
        v["stdout_json"]["match"], false,
        "the report is still evidence"
    );
    assert!(
        v["evidence"]["stderr"]
            .as_str()
            .unwrap()
            .contains("does NOT match"),
        "stderr must be preserved as diagnostic evidence"
    );
    assert!(!out.status.success(), "a failed verdict exits non-zero");
}

// ---------------------------------------------------------------- case C ---
#[test]
fn c_upgrade_safety_plans_old_and_new() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sdkt(tmp.path(), r#"printf '{"compatible":false}\n'"#);

    let out = agent(
        &bin,
        "check whether this wasm is safe to upgrade from us_old.wasm to us_new.wasm",
        true,
    );
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "diff.upgrade_safety");
    let joined = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined,
        "diff --upgrade-safety --old-wasm us_old.wasm --new-wasm us_new.wasm --format json"
    );
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

// ---------------------------------------------------------------- case D ---
#[test]
fn d_ambiguous_request_is_clarified_and_never_executed() {
    let tmp = tempfile::tempdir().unwrap();
    // A marker file proves the fake CLI was never invoked.
    let marker = tmp.path().join("ran");
    let bin = fake_sdkt(
        tmp.path(),
        &format!("touch {}\nprintf '{{}}\\n'", marker.display()),
    );

    let out = agent(&bin, "check health and verify the contract", true);
    let v = parse_json(&out);
    assert_eq!(v["status"], "needs_clarification");
    assert_eq!(v["error_code"], "ambiguous_request");
    assert!(!v["evidence"]["command_executed"].as_bool().unwrap());
    assert!(v["argv"].as_array().unwrap().is_empty());
    assert!(!marker.exists(), "no CLI execution may happen");
}

// ---------------------------------------------------------------- case E ---
#[test]
fn e_mutation_request_is_blocked_and_never_executed() {
    for (request, id) in [
        ("deploy this contract to testnet", "deploy"),
        ("invoke the mint function", "invoke"),
        ("submit transaction AAAA", "tx.submit"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran");
        let bin = fake_sdkt(
            tmp.path(),
            &format!("touch {}\nprintf '{{}}\\n'", marker.display()),
        );

        let out = agent(&bin, request, true);
        let v = parse_json(&out);
        assert_eq!(v["status"], "blocked", "{request}");
        assert_eq!(v["capability_id"], id, "{request}");
        assert!(
            !v["evidence"]["command_executed"].as_bool().unwrap(),
            "{request}"
        );
        assert!(!marker.exists(), "{request}: nothing may be executed");
    }
}

// ---------------------------------------------------------------- case F ---
#[test]
fn f_missing_argument_is_clarified_and_never_executed() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ran");
    let bin = fake_sdkt(
        tmp.path(),
        &format!("touch {}\nprintf '{{}}\\n'", marker.display()),
    );

    let out = agent(&bin, "check health of the contract", true);
    let v = parse_json(&out);
    assert_eq!(v["status"], "needs_clarification");
    assert_eq!(v["error_code"], "missing_argument");
    assert!(!marker.exists());
}

// ---------------------------------------------------------------- case G ---
#[test]
fn g_critical_health_exit_1_stays_failed() {
    let tmp = tempfile::tempdir().unwrap();
    // This is the exact shape of a `critical` health run: a perfectly valid
    // JSON report AND a non-zero exit.
    let bin = fake_sdkt(
        tmp.path(),
        r#"printf '{"health":"critical","verified":false}\n'
echo "Verdict: On-chain WASM does NOT match" 1>&2
exit 1"#,
    );

    let out = agent(
        &bin,
        &format!("check health of contract {CID} on testnet"),
        true,
    );
    let v = parse_json(&out);

    assert_eq!(v["exit_code"], 1);
    assert_eq!(v["status"], "failed", "a valid report must not mask exit 1");
    assert_eq!(v["stdout_json"]["health"], "critical");
    assert!(!out.status.success());
}

// ---------------------------------------------------------------- case H ---
#[test]
fn h_stderr_never_contaminates_json_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sdkt(
        tmp.path(),
        r#"printf '{"health":"healthy"}\n'
echo "noise line one" 1>&2
echo "noise line two" 1>&2
exit 0"#,
    );

    let out = agent(
        &bin,
        &format!("check health of contract {CID} on testnet"),
        true,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The agent's stdout is a single JSON document even though the child
    // wrote to both streams.
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}): {stdout:?}"));

    // The captured *child* stdout holds only the report — never the child's
    // stderr — because the executor drains the pipes separately.
    assert_eq!(
        v["stdout_text"].as_str().unwrap(),
        "{\"health\":\"healthy\"}\n"
    );
    assert!(
        !v["stdout_text"]
            .as_str()
            .unwrap()
            .contains("noise line one"),
        "child stderr leaked into the child stdout field"
    );
    // stderr survives only as diagnostic evidence.
    assert!(v["evidence"]["stderr"]
        .as_str()
        .unwrap()
        .contains("noise line one"));
    assert_eq!(v["status"], "success");
}

// ------------------------------------------------------------ extra ---
#[test]
fn mainnet_request_is_refused_without_executing() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ran");
    let bin = fake_sdkt(
        tmp.path(),
        &format!("touch {}\nprintf '{{}}\\n'", marker.display()),
    );

    let out = agent(
        &bin,
        &format!("check health of contract {CID} on mainnet"),
        true,
    );
    let v = parse_json(&out);
    assert_eq!(v["status"], "blocked");
    assert_eq!(v["error_code"], "refused_network");
    assert!(
        !marker.exists(),
        "mainnet must never be executed implicitly"
    );
}

#[test]
fn help_and_version_work() {
    Command::cargo_bin("sdkt-agent")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    Command::cargo_bin("sdkt-agent")
        .unwrap()
        .arg("--version")
        .assert()
        .success();
}

#[test]
fn pretty_output_is_concise_and_labelled() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sdkt(tmp.path(), r#"printf '{"health":"healthy"}\n'"#);
    let out = agent(
        &bin,
        &format!("check health of contract {CID} on testnet"),
        false,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for label in [
        "REQUEST",
        "CAPABILITY",
        "COMMAND",
        "RESULT",
        "EVIDENCE",
        "EXPLANATION",
    ] {
        assert!(stdout.contains(label), "missing {label} in:\n{stdout}");
    }
}

#[test]
fn agent_never_plans_a_mutating_command() {
    // Cross-check against the registry: no read-only request may produce an
    // argv whose first word is a mutating command.
    let mutating: Vec<Vec<String>> = sdkt_core::registry::capabilities()
        .iter()
        .filter(|c| c.safety == sdkt_core::registry::Safety::Mutating)
        .map(|c| c.command.iter().map(|s| s.to_string()).collect())
        .collect();

    for request in [
        format!("inspect contract {CID}"),
        "inspect wasm a.wasm".to_string(),
        "run doctor".to_string(),
        "audit src/lib.rs".to_string(),
        "check network status profile wl-testnet".to_string(),
    ] {
        if let sdkt_agent::Planned::Run(p) = sdkt_agent::plan(&request) {
            for m in &mutating {
                assert!(
                    !p.argv.starts_with(m.as_slice()),
                    "{request:?} planned a mutating command {m:?}"
                );
            }
        }
    }
}
