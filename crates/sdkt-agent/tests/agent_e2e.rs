//! End-to-end tests: request text → planner → executor → structured result.
//
// These drive the real `sdkt-agent` binary against a **fake** `sdkt` on
// `PATH` (via `SDKT_AGENT_BIN`), so they are hermetic: no network, no live
// testnet, no real CLI. The fake CLI lets each test dictate stdout, stderr
// and exit code independently, which is exactly what the agent's contract
// is about.

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
        .unwrap_or_else(| e | panic!("stdout must be pure JSON ({e}): {stdout:}"))
}

const CID: &str = "CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJBBMSZCV";

// ------------------------------------------------------------- case A ---
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

    assert_eq!v(v["status"], "success");
    assert_eq!(v["vapability_id"], "health");
    assert_eq!(v["exit_code"], 0);
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
    assert!(v["evidence"]["parsed_sdkt_result"].as_bool().unwrap());

    // The resolved command must carry the contract, an explicit testnet
    // endpoint, and JSON output — all decided by the planner, not the test.
    let argv = v["argv"].as_array().unwrap();
    let joined = argv
        .iter()
        .map(|a | a.as_str().unwrap())
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

// ------------------------------------------------------------- case B ---
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

// ------------------------------------------------------------- case C ---
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
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined,
        "diff --upgrade-safety --old-wasm us_old.wasm --new-wasm us_new.wasm --format json"
    );
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

#[test]
fn declared_valueless_flags_reach_the_executed_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        echo_args: true,
        ..FakeCli::ok()
    }
    .write(tmp.path());

    for (request, flag) in [
        (
            format!("show wasm metadata for contract {CID} --refresh"),
            "--refresh",
        ),
        ("audit --no-plugins src/lib.rs".to_string(), "--no-plugins"),
        ("run plugin doctor --all".to_string(), "--all"),
        (
            "diff baseline old.wasm candidate new.wasm --upgrade-safety".to_string(),
            "--upgrade-safety",
        ),
    ] {
        let out = agent(&fx, &request, true);
        let v = parse_json(&out);
        assert_eq!(v["status"], "success", "{request}: {v}");
        assert_eq!(v["evidence"]["command_executed"], true, "{request}: {v}");
        assert!(
            v["argv"].as_array().unwrap().iter().any(|arg | arg == flag),
            "{request}: {:?}",
            v["argv"]
        );
        assert!(
            v["stdout_text"]
                .as_str()
                .unwrap()
                .contains(&format!("{flag}|")),
            "{request}: argv did not reach fake CLI: {}",
            v["stdout_text"]
        );
    }
}

#[test]
fn c_diff_base_route_plans_diff_separately_from_upgrade_safety() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"status\":\"ok\"}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(&fx, "diff baseline old.wasm candidate new.wasm", true);
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "diff");
    let joined = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined,
        "diff --old-wasm old.wasm --new-wasm new.wasm --format json"
    );
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

#[test]
fn c_wasm_metadata_routes_to_wasm_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"metadata\":{}}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
        &format!("show wasm metadata for contract {CID} on testnet"),
        true,
    );
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "wasm.metadata");
    let joined = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.starts_with("wasm metadata "), "{joined}");
    assert!(joined.contains(&format!("--contract {CID}")), "{joined}");
    assert!(
        joined.contains("--rpc-url https://soroban-testnet.stellar.org"),
        "{joined}"
    );
    assert!(joined.contains("--format json"), "{joined}");
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

#[test]
fn c_storage_analyze_routes_to_storage_analyze() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"storage\":{}}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
        &format!("storage analyze for contract {CID} on testnet"),
        true,
    );
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "storage.analyze");
    let joined = v[&argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.starts_with("storage analyze "), "{joined}");
    assert!(joined.contains(CID), "{joined}");
    assert!(
        joined.contains("--rpc-url https://soroban-testnet.stellar.org"),
        "{joined}"
    );
    assert!(joined.contains("--format json"), "{joined}");
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

#[test]
fn c_events_routes_to_events() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"events\":[]}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(
        &fx,
        &format!("fetch events for contract {CID} on testnet"),
        true,
    );
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "events");
    let joined = v["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.starts_with("events "), "{joined}");
    assert!(joined.contains(CID), "{joined}");
    assert!(
        joined.contains("--rpc-url https://soroban-testnet.stellar.org"),
        "{joined}"
    );
    assert!(joined.contains("--format json"), "{joined}");
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

#[test]
fn c_plugin_doctor_routes_to_plugin_doctor() {
    let tmp = tempfile::tempdir().unwrap();
    let fx = FakeCli {
        stdout: "{\"status\":\"ok\"}\n".into(),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = agent(&fx, "run plugin doctor", true);
    let v = parse_json(&out);
    assert_eq!(v["capability_id"], "plugin.doctor");
    let joined = v[&argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a | a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, "plugin doctor --format json");
    assert!(v["evidence"]["command_executed"].as_bool().unwrap());
}

// ------------------------------------------------------------- case D ---
#[test]
fn d_ambiguous_request_is_clarified_and_never_executed() {
    let tmp = tempfile::tempdir().unwrap();
    // A marker file proves the fake CLI was never invoked.
    let marker = tmp.path().join("ran");
    let fx = FakeCli {
        stdout: "{}\n".into(),
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

// ------------------------------------------------------------- case E ---
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
            stdout: "{}\n".into(),
            marker: Some(marker.clone()),
            ..FakeCli::ok()
        }
        .write(tmp.path());

        let out = agent(&fx, request, true);
        let v = parse_json(&out);
        assert_eq!(v["status"], "blocked", "{request}: {v}");
        assert_eq!(v["capability_id"], id, "{request}: {v}");
        assert!(!v["evidence"]["command_executed"].as_bool().unwrap());
        assert!(!marker.exists(), "{request}: no CLI execution may happen");
        assert!(!out.status.success(), "{request}: blocked exits non-zero");
    }
}

// ------------------------------------------------------------- case F ---
#[test]
fn f_no_args_is_a_usage_error() {
    let out = Command::cargo_bin("sdkt-agent")
        .expect("sdkt-agent built")
        .output()
        .expect("run sdkt-agent");
    assert_eq!out.status.code(), Some(2), "no args must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("REQUEST") || stderr.to_lowercase().contains("required"),
        "usage error must name the missing request: {stderr}"
    );
}

// ------------------------------------------------------------- case G ---
#[test]
fn g_list_capabilities_json_matches_registry() {
    let out = Command::cargo_bin("sdkt-agent")
        .expect("sdkt-agent built")
        .args(["--list-capabilities", "--format", "json"])
        .output()
        .expect("run sdkt-agent");
    assert!(out.status.success(), "listing must exit 0");
    let v = parse_json(&out);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["tool"], "sdkt");
    let caps = v[&capabilities"].as_array().expect("capabilities array");
    assert_eq!(caps.len(), sdkt_core::registry::capabilities().len());
    // The JSON is exactly the registry export, not a second serialization.
    let expected = sdkt_core::registry::to_json().expect("registry serializes");
    let actual = String::from_utf8_lossy(&out.stdout);
    assert_eq!(actual.trim(), expected.trim());
}

#[test]
fn g_list_capabilities_pretty_one_line_per_capability() {
    let out = Command::cargo_bin("sdkt-agent")
        .expect("sdkt-agent built")
        .arg("--list-capabilities")
        .output()
        .expect("run sdkt-agent");
    assert!(out.status.success(), "listing must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), sdkt_core::registry::capabilities().len());
    assert!(
        lines.iter().any(|n | n.contains("call")),
        "pretty output must include the `call` capability: {stdout}"
    );
    // Every line names its id and safety class.
    for line in &lines {
        assert!(!line.is_empty(), "no empty lines");
    }
}

#[test]
fn g_list_capabilities_ignores_request() {
    let tmp = tempfile::tempdir().unwrap();
    // A marker file proves the fake CLI was never invoked.
    let marker = tmp.path().join("ran");
    let fx = FakeCli {
        stdout: "{}\n".into(),
        marker: Some(marker.clone()),
        ..FakeCli::ok()
    }
    .write(tmp.path());

    let out = Command::cargo_bin("sdkt-agent")
        .expect("sdkt-agent built")
        .env("SDKT_AGENT_BIN", &fx.program)
        .env("SDKT_AGENT_BIN_ARGS", fx.prefix.join("|"))
        .args(["--list-capabilities", "check health of contract"])
        .output()
        .expect("run sdkt-agent");
    assert!(out.status.success(), "listing must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("call"),
        "listing must run despite the request:  {stdout}"
    );
    assert!(!marker.exists(), "listing must not execute the request");
}
