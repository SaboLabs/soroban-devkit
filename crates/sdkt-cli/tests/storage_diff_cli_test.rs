use assert_cmd::Command;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use tempfile::tempdir;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

fn create_snapshot_file(path: &Path, contract_id: &str, entries: &[(&str, u32)]) {
    let entry_objs: Vec<serde_json::Value> = entries
        .iter()
        .map(|(k, ttl)| {
            serde_json::json!({
                "key": k,
                "class": "persistent",
                "current_ttl": ttl,
                "days_remaining": ttl / 17280,
                "extension_cost_stroops": 0,
            })
        })
        .collect();

    let report = serde_json::json!({
        "contract_id": contract_id,
        "total_entries": entries.len(),
        "instance_entries": 0,
        "persistent_entries": entries.len(),
        "temporary_entries": 0,
        "other_entries": 0,
        "entries": entry_objs,
    });

    fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
}

#[test]
fn extend_plan_cli_never_contacts_rpc_and_carries_network_flag() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let mock_url = format!("http://{}", listener.local_addr().unwrap());
    listener
        .set_nonblocking(true)
        .expect("set nonblocking listener");

    let dir = tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let new_path = dir.path().join("new.json");

    create_snapshot_file(&old_path, CONTRACT, &[("keyA==", 50_000)]);
    create_snapshot_file(&new_path, CONTRACT, &[("keyA==", 3_000)]);

    let mut cmd = sdkt();
    cmd.arg("storage")
        .arg("storage-diff")
        .arg("--old")
        .arg(&old_path)
        .arg("--new")
        .arg(&new_path)
        .arg("--extend-plan")
        .arg("--rpc-url")
        .arg(&mock_url);

    let assert = cmd.assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("Extension Plan"));
    assert!(stdout.contains("Ready-to-run:"));
    assert!(stdout.contains(&format!("--rpc-url '{}'", mock_url)));
    assert!(stdout.contains(&format!("--contract '{}'", CONTRACT)));
    assert!(stdout.contains("--key 'keyA=='"));

    // Verify mock server received zero connections.
    let accept_res = listener.accept();
    assert!(
        accept_res.is_err(),
        "Expected no RPC connections, but server received a connection!"
    );
}

#[test]
fn storage_diff_cli_shell_quoting_regression_test() {
    let dir = tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let new_path = dir.path().join("new.json");

    let pwn_file = "/tmp/sdkt_pwn_test_file";
    let _ = fs::remove_file(pwn_file);

    let injection_key = format!("BKEYB==; touch {pwn_file} space $(id)");

    create_snapshot_file(&old_path, CONTRACT, &[(&injection_key, 50_000)]);
    create_snapshot_file(&new_path, CONTRACT, &[]);

    let mut cmd = sdkt();
    cmd.arg("storage")
        .arg("storage-diff")
        .arg("--old")
        .arg(&old_path)
        .arg("--new")
        .arg(&new_path)
        .arg("--extend-plan");

    let assert = cmd.assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("Ready-to-run:"));
    assert!(stdout.contains(&format!("--key '{}'", injection_key)));

    // Verify that the command injection was NOT executed.
    assert!(
        !Path::new(pwn_file).exists(),
        "Injection file was created! Quoting failed."
    );
}

#[test]
fn storage_diff_cli_rejects_empty_entries_when_total_entries_positive() {
    let dir = tempdir().unwrap();
    let legacy_old_path = dir.path().join("legacy_old.json");
    let new_path = dir.path().join("new.json");

    // Legacy report with total_entries > 0 but missing `entries` array.
    let legacy_json = serde_json::json!({
        "contract_id": CONTRACT,
        "total_entries": 3,
        "instance_entries": 1,
        "persistent_entries": 2,
        "temporary_entries": 0,
        "other_entries": 0,
    });
    fs::write(
        &legacy_old_path,
        serde_json::to_string(&legacy_json).unwrap(),
    )
    .unwrap();

    create_snapshot_file(&new_path, CONTRACT, &[("keyA==", 10_000)]);

    let mut cmd = sdkt();
    cmd.arg("storage")
        .arg("storage-diff")
        .arg("--old")
        .arg(&legacy_old_path)
        .arg("--new")
        .arg(&new_path)
        .arg("--extend-plan");

    let assert = cmd.assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();

    assert!(stderr.contains(&format!(
        "Failed to load --old snapshot '{}'",
        legacy_old_path.display()
    )));
    assert!(stderr.contains("total_entries (3) > 0"));
    assert!(stderr.contains("entries array is empty"));
}

#[test]
fn storage_diff_cli_alias_diff_works() {
    let dir = tempdir().unwrap();
    let old_path = dir.path().join("old.json");
    let new_path = dir.path().join("new.json");

    create_snapshot_file(&old_path, CONTRACT, &[("keyA==", 50_000)]);
    create_snapshot_file(&new_path, CONTRACT, &[("keyA==", 3_000)]);

    let mut cmd = sdkt();
    cmd.arg("storage")
        .arg("diff")
        .arg("--old")
        .arg(&old_path)
        .arg("--new")
        .arg(&new_path)
        .arg("--extend-plan");

    let assert = cmd.assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("Extension Plan"));
}
