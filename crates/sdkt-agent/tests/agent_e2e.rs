//! End-to-end tests: request text → planner → executor → structured result.
//!
//! These drive the real `sdkt-agent` binary against a **fake** `sdkt` on
//! `PATH` (via `SDKT_AGENT_BIN`), so they are hermetic: no network, no live
//! testnet, no real CLI. The fake CLI lets each test dictate stdout, stderr
//! and exit code independently, which is exactly what the agent's contract
//! is about.

use assert_cmd::Command;
use std::process::Output;

// The same fake-CLI spec the executor unit tests use: a native script per
// platform (sh on unix, PowerShell on Windows) with identical bytes out.
#[path = "common/fixture.rs"]
mod fixture;

use fixture::FakeCli;

/// Run the agent against a prepared fake CLI.
fn agent(fx: &fixture::Fixture, request: &str, json: bool) -> Output {
    let mut cmd = Command::cargo_bin("sdkt-agent").expect("sdkt-agent built");
    cmd.env("SDKT_AGENT_BIN", &fx.program);
    // Windows runs the fake as `powershell.exe -File <script>`, so the
    // fixed leading arguments have to reach the executor too.
    cmd.env("SDKT_AGENT_BIN_ARGS", fx.prefix.join("|"));
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
    let fx = FakeCli {
        stdout: "{\"health\":\"healthy\"}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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
    let fx = FakeCli {
        stdout: "{\"match\":false,\"verification_status\":\"Mismatch\"}\n".into(),
        stderr: "Error: local WASM does NOT match\n".into(),
        exit_code: 1,
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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
    let fx = FakeCli {
        stdout: "{\"compatible\":false}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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
    let fx = FakeCli {
        stdout: "{}
"
        .into(),
        marker: Some(marker.clone()),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(&fx, "check health and verify the contract", true);
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
        let fx = FakeCli {
            stdout: "{}
"
            .into(),
            marker: Some(marker.clone()),
            ..FakeCli::ok()
        }
        .write(tmp.path());

        let out = agent(&fx, request, true);
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
    let fx = FakeCli {
        stdout: "{}
"
        .into(),
        marker: Some(marker.clone()),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(&fx, "check health of the contract", true);
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
    let fx = FakeCli {
        stdout: "{\"health\":\"critical\",\"verified\":false}\n".into(),
        stderr: "Verdict: On-chain WASM does NOT match\n".into(),
        exit_code: 1,
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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
    let fx = FakeCli {
        stdout: "{\"health\":\"healthy\"}\n".into(),
        stderr: "noise line one\nnoise line two\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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

// ------------------------------------------------- release-assurance argv ---
#[test]
fn release_assurance_baseline_reaches_sdkt_as_a_flagged_pair() {
    // Regression: the candidate used to be emitted as a positional argument
    // and the baseline took `--wasm`, so sdkt answered with an exit-2 usage
    // error. The fake echoes the argv it actually received, which is what the
    // real CLI parses.
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        echo_args: true,
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
        "release assurance for candidate.wasm previous baseline.wasm",
        true,
    );
    let v = parse_json(&out);

    assert_eq!(v["capability_id"], "release_assurance");
    assert_eq!(v["status"], "success", "{v}");
    assert_eq!(v["exit_code"], 0);

    // argv as received by the child, not as the planner remembers it.
    let echoed = v["stdout_text"].as_str().unwrap();
    assert_eq!(
        echoed,
        "release-assurance|--wasm|candidate.wasm|--previous-wasm|baseline.wasm|--rpc-url|\
         https://soroban-testnet.stellar.org|--network-passphrase|Test SDF Network ; September \
         2015|--format|json|",
        "argv reached sdkt malformed: {echoed}"
    );
    // The planner's own record must agree.
    let argv: Vec<&str> = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        &argv[..4],
        &[
            "release-assurance",
            "--wasm",
            "candidate.wasm",
            "--previous-wasm"
        ]
    );
    assert_eq!(argv[4], "baseline.wasm");
}

#[test]
fn release_assurance_single_artifact_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"release_status\":\"REVIEW\"}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());
    let out = agent(&fx, "release assurance for candidate.wasm", true);
    let v = parse_json(&out);
    assert_eq!(v["status"], "success");
    let argv: Vec<&str> = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        &argv[..3],
        &["release-assurance", "--wasm", "candidate.wasm"]
    );
    assert!(!argv.contains(&"--previous-wasm"));
}

#[test]
fn usage_error_is_not_reported_as_a_failed_verdict() {
    // A clap usage error exits 2 with an empty stdout. Calling that a failed
    // verdict points the reader at a report that was never produced.
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stderr: "error: unexpected argument 'candidate.wasm' found\n".into(),
        exit_code: 2,
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(&fx, "release assurance for candidate.wasm", true);
    let v = parse_json(&out);

    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["status"], "failed");
    let explanation = v["explanation"].as_str().unwrap();
    assert!(
        explanation.contains("usage error"),
        "usage error must be named: {explanation}"
    );
    assert!(
        !explanation.contains("verdict failed"),
        "a usage error must not be described as a failed verdict: {explanation}"
    );
    assert!(
        !explanation.contains("the report above"),
        "no report exists for a usage error: {explanation}"
    );
}

// ------------------------------------------------------------ extra ---
#[test]
fn mainnet_request_is_refused_without_executing() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ran");
    let fx = FakeCli {
        stdout: "{}
"
        .into(),
        marker: Some(marker.clone()),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
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
    let fx = FakeCli {
        stdout: "{\"health\":\"healthy\"}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());
    let out = agent(
        &fx,
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
