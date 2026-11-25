use assert_cmd::Command;

#[test]
fn lists_registry_as_json_without_a_request() {
    let output = Command::cargo_bin("sdkt-agent")
        .unwrap()
        .args(["--list-capabilities", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["tool"], "sdkt");
    assert!(document["capabilities"].as_array().unwrap().len() > 1);
}

#[test]
fn lists_known_capability_in_pretty_mode() {
    let output = Command::cargo_bin("sdkt-agent")
        .unwrap()
        .arg("--list-capabilities")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&output).contains("call | sdkt call"));
}

#[test]
fn no_request_without_listing_is_a_usage_error() {
    let output = Command::cargo_bin("sdkt-agent")
        .unwrap()
        .assert()
        .failure()
        .code(2)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&output).contains("REQUEST"));
}

#[test]
fn listing_does_not_execute_a_supplied_request() {
    let output = Command::cargo_bin("sdkt-agent")
        .unwrap()
        .args(["--list-capabilities", "this is not a request"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&output).contains("call | sdkt call"));
}
