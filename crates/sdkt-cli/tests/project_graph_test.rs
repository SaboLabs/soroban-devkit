use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

fn sdkt(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.current_dir(dir);
    cmd.env("SDKT_IDENTITY_DIR", dir.join("identity"));
    cmd.env("SDKT_NETWORK_DIR", dir.join("network"));
    cmd
}

/// Set up a 3-contract workspace (token ← vault ← router) without WASM artifacts.
fn setup_three_contract_workspace(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]

[contracts.router]
path = "contracts/router"
depends_on = ["vault"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();
}

/// Set up a 2-contract workspace (token ← vault) without WASM artifacts.
fn setup_two_contract_workspace(dir: &Path) {
    let manifest = r#"
[contracts.token]
path = "contracts/token"

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    std::fs::write(dir.join(".sdkt.toml"), manifest).unwrap();
}

#[test]
fn project_graph_pretty_two_contract_workspace() {
    let tmp = TempDir::new().unwrap();
    setup_two_contract_workspace(tmp.path());

    let output = sdkt(tmp.path())
        .args(["project", "graph"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let token_pos = stdout
        .find("token (contracts/token) depends on: \u{2014}")
        .expect("token row present with em-dash");
    let vault_pos = stdout
        .find("vault (contracts/vault) depends on: token")
        .expect("vault row present with token dependency");
    assert!(
        token_pos < vault_pos,
        "expected token before vault in output:\n{}",
        stdout
    );
}

#[test]
fn project_graph_json_matches_resolve_deploy_order() {
    let tmp = TempDir::new().unwrap();
    setup_three_contract_workspace(tmp.path());

    let output = sdkt(tmp.path())
        .args(["project", "graph", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("valid JSON output from project graph");

    let config = sdkt_core::config::DevKitConfig::load_from_file(tmp.path().join(".sdkt.toml"))
        .expect("load fixture config");
    let expected_order = sdkt_core::project::resolve_deploy_order(&config).unwrap();

    let actual_order: Vec<String> = parsed["order"]
        .as_array()
        .expect("order array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(actual_order, expected_order);

    let nodes = parsed["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0]["alias"], "token");
    assert_eq!(nodes[0]["path"], "contracts/token");
    assert_eq!(nodes[0]["depends_on"], serde_json::json!([]));
    assert_eq!(nodes[1]["alias"], "vault");
    assert_eq!(nodes[1]["path"], "contracts/vault");
    assert_eq!(nodes[1]["depends_on"], serde_json::json!(["token"]));
    assert_eq!(nodes[2]["alias"], "router");
    assert_eq!(nodes[2]["path"], "contracts/router");
    assert_eq!(nodes[2]["depends_on"], serde_json::json!(["vault"]));
}

#[test]
fn project_graph_dot_emits_digraph_and_edges() {
    let tmp = TempDir::new().unwrap();
    setup_three_contract_workspace(tmp.path());

    let output = sdkt(tmp.path())
        .args(["project", "graph", "--format", "dot"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("digraph"), "missing digraph:\n{}", stdout);
    assert!(
        stdout.contains("\"token\";"),
        "missing token node:\n{}",
        stdout
    );
    assert!(
        stdout.contains("\"vault\";"),
        "missing vault node:\n{}",
        stdout
    );
    assert!(
        stdout.contains("\"router\";"),
        "missing router node:\n{}",
        stdout
    );
    assert!(
        stdout.contains("\"vault\" -> \"token\";"),
        "missing vault -> token edge:\n{}",
        stdout
    );
    assert!(
        stdout.contains("\"router\" -> \"vault\";"),
        "missing router -> vault edge:\n{}",
        stdout
    );
}

#[test]
fn project_graph_cycle_exits_nonzero_with_no_stdout() {
    let tmp = TempDir::new().unwrap();
    let manifest = r#"
[contracts.token]
path = "contracts/token"
depends_on = ["vault"]

[contracts.vault]
path = "contracts/vault"
depends_on = ["token"]
"#;
    std::fs::write(tmp.path().join(".sdkt.toml"), manifest).unwrap();

    let output = sdkt(tmp.path())
        .args(["project", "graph"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "expected non-zero exit for cyclic workspace"
    );
    assert!(
        output.stdout.is_empty(),
        "expected no stdout on invalid workspace, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Circular dependency"),
        "expected circular dependency error in stderr, got: {}",
        stderr
    );
}
