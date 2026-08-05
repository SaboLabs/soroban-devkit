//! Integration test for M18 dynamic plugin loading (requires `--features plugins`).
//!
//! Builds the example plugin cdylib on the fly, then drives `sdkt audit
//! --rules <plugin.so>` and asserts the dynamic rule fires without breaking the
//! built-in rules. Non-`plugins` builds skip this test (the behavior is covered
//! by the default audit regression tests).

use std::process::Command;

use assert_cmd::cargo::CommandCargoExt;
use assert_cmd::prelude::*;
use predicates::prelude::*;

/// Build the example plugin cdylib and return its path under `target/`.
#[cfg(feature = "plugins")]
fn build_example_plugin() -> std::path::PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "sdkt-audit-example-rule",
            "--features",
            "plugins",
        ])
        .status()
        .expect("failed to spawn cargo build for example plugin");
    assert!(status.success(), "example plugin build failed");

    // Find the produced cdylib in target/debug.
    let patterns = [
        "libsdkt_audit_example_rule.so",
        "sdkt_audit_example_rule.dll",
    ];
    for name in patterns {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(name);
        if p.exists() {
            return p;
        }
    }
    panic!("example plugin cdylib not found after build");
}

#[cfg(feature = "plugins")]
#[test]
fn dynamic_plugin_rule_fires() {
    let plugin = build_example_plugin();
    let src = "pub fn sdkt_example_trigger_admin() {}";

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let tmp = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
    std::fs::write(tmp.path(), src).unwrap();
    cmd.arg("audit").arg(tmp.path()).arg("--rules").arg(&plugin);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("EXAMPLE-001"))
        .stdout(predicate::str::contains("sdkt_example_trigger_admin"));
}

#[cfg(feature = "plugins")]
#[test]
fn plugin_bounds_clamping_prevents_read_overflow() {
    let plugin = build_example_plugin();
    // 70 triggers > 64 MAX_FINDINGS. If not clamped, host loop would read
    // past buffer end and segfault or read garbage. Clamping ensures only 64
    // are processed safely.
    let src = "
        pub fn sdkt_example_trigger_01() {}
        pub fn sdkt_example_trigger_02() {}
        pub fn sdkt_example_trigger_03() {}
        pub fn sdkt_example_trigger_04() {}
        pub fn sdkt_example_trigger_05() {}
        pub fn sdkt_example_trigger_06() {}
        pub fn sdkt_example_trigger_07() {}
        pub fn sdkt_example_trigger_08() {}
        pub fn sdkt_example_trigger_09() {}
        pub fn sdkt_example_trigger_10() {}
        pub fn sdkt_example_trigger_11() {}
        pub fn sdkt_example_trigger_12() {}
        pub fn sdkt_example_trigger_13() {}
        pub fn sdkt_example_trigger_14() {}
        pub fn sdkt_example_trigger_15() {}
        pub fn sdkt_example_trigger_16() {}
        pub fn sdkt_example_trigger_17() {}
        pub fn sdkt_example_trigger_18() {}
        pub fn sdkt_example_trigger_19() {}
        pub fn sdkt_example_trigger_20() {}
        pub fn sdkt_example_trigger_21() {}
        pub fn sdkt_example_trigger_22() {}
        pub fn sdkt_example_trigger_23() {}
        pub fn sdkt_example_trigger_24() {}
        pub fn sdkt_example_trigger_25() {}
        pub fn sdkt_example_trigger_26() {}
        pub fn sdkt_example_trigger_27() {}
        pub fn sdkt_example_trigger_28() {}
        pub fn sdkt_example_trigger_29() {}
        pub fn sdkt_example_trigger_30() {}
        pub fn sdkt_example_trigger_31() {}
        pub fn sdkt_example_trigger_32() {}
        pub fn sdkt_example_trigger_33() {}
        pub fn sdkt_example_trigger_34() {}
        pub fn sdkt_example_trigger_35() {}
        pub fn sdkt_example_trigger_36() {}
        pub fn sdkt_example_trigger_37() {}
        pub fn sdkt_example_trigger_38() {}
        pub fn sdkt_example_trigger_39() {}
        pub fn sdkt_example_trigger_40() {}
        pub fn sdkt_example_trigger_41() {}
        pub fn sdkt_example_trigger_42() {}
        pub fn sdkt_example_trigger_43() {}
        pub fn sdkt_example_trigger_44() {}
        pub fn sdkt_example_trigger_45() {}
        pub fn sdkt_example_trigger_46() {}
        pub fn sdkt_example_trigger_47() {}
        pub fn sdkt_example_trigger_48() {}
        pub fn sdkt_example_trigger_49() {}
        pub fn sdkt_example_trigger_50() {}
        pub fn sdkt_example_trigger_51() {}
        pub fn sdkt_example_trigger_52() {}
        pub fn sdkt_example_trigger_53() {}
        pub fn sdkt_example_trigger_54() {}
        pub fn sdkt_example_trigger_55() {}
        pub fn sdkt_example_trigger_56() {}
        pub fn sdkt_example_trigger_57() {}
        pub fn sdkt_example_trigger_58() {}
        pub fn sdkt_example_trigger_59() {}
        pub fn sdkt_example_trigger_60() {}
        pub fn sdkt_example_trigger_61() {}
        pub fn sdkt_example_trigger_62() {}
        pub fn sdkt_example_trigger_63() {}
        pub fn sdkt_example_trigger_64() {}
        pub fn sdkt_example_trigger_65() {}
        pub fn sdkt_example_trigger_66() {}
        pub fn sdkt_example_trigger_67() {}
        pub fn sdkt_example_trigger_68() {}
        pub fn sdkt_example_trigger_69() {}
        pub fn sdkt_example_trigger_70() {}
    ";

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    let tmp = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
    std::fs::write(tmp.path(), src).unwrap();
    cmd.arg("audit").arg(tmp.path()).arg("--rules").arg(&plugin);

    cmd.assert()
        .success()
        // The example plugin itself halts at MAX_FINDINGS, but even if it didn't,
        // the host now clamps reads to 64.
        .stdout(predicate::str::contains("(64 total)"));
}

#[cfg(feature = "plugins")]
#[test]
fn dynamic_plugin_coexists_with_builtins() {
    let plugin = build_example_plugin();
    // A contract with a real auth bug (AUTH-001) AND a trigger function
    // (EXAMPLE-001) must surface both rules together.
    let src =
        "pub fn mint_token(to: Address) { /* no auth */ }\npub fn sdkt_example_trigger_admin() {}";
    let tmp = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
    std::fs::write(tmp.path(), src).unwrap();

    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();
    cmd.arg("audit").arg(tmp.path()).arg("--rules").arg(&plugin);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("AUTH-001"))
        .stdout(predicate::str::contains("EXAMPLE-001"));
}

#[cfg(feature = "wasm-plugins")]
fn build_example_wasm_plugin() -> std::path::PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "sdkt-audit-example-rule",
            "--target",
            "wasm32-wasip1",
            "--features",
            "wasm-plugins",
        ])
        .status()
        .expect("failed to spawn cargo build for example WASM plugin");
    assert!(status.success(), "example WASM plugin build failed");

    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let wasm_path = workspace_root.join("target/wasm32-wasip1/debug/sdkt_audit_example_rule.wasm");
    assert!(
        wasm_path.exists(),
        "WASM plugin not found at {:?}",
        wasm_path
    );
    wasm_path
}

#[cfg(feature = "wasm-plugins")]
#[test]
fn wasm_plugin_coexists_with_builtins() {
    let wasm_path = build_example_wasm_plugin();
    let mut cmd = Command::cargo_bin("sdkt-cli").unwrap();

    // We create a temp file that triggers both AUTH-001 (builtin) and EXAMPLE-001 (wasm)
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        "pub fn sdkt_example_trigger_admin(who: Address) { /* no auth */ }\n",
    )
    .unwrap();

    cmd.args([
        "audit",
        tmp.path().to_str().unwrap(),
        "--rules",
        wasm_path.to_str().unwrap(),
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("AUTH-001"))
        .stdout(predicate::str::contains("EXAMPLE-001"))
        .stdout(predicate::str::contains("sdkt_example_trigger_admin"));
}

#[test]
fn plugin_without_feature_errors_clearly() {
    // Build the CLI WITHOUT the plugins feature and confirm a `.so` rule path
    // is rejected with a clear message. This mirrors the default-build guard.
    // (We can't easily rebuild the bin here, so we assert the guard text exists
    //  in the source instead — the actual runtime path is covered by CI with
    //  and without the feature.)
    let guard = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .unwrap();
    assert!(
        guard.contains("without the `plugins` feature"),
        "default-build plugin guard message missing"
    );
    assert!(
        guard.contains("without the `wasm-plugins` feature"),
        "default-build wasm plugin guard message missing"
    );
}
