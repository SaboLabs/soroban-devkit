//! Registry ↔ CLI parity.
//!
//! The capability registry (`sdkt_core::registry`) is metadata *about* the
//! CLI. Metadata drifts; this test is the tripwire. It shells out to the real
//! binary and asserts, for every described capability, that the registry's
//! `formats.json` claim matches whether that command actually accepts
//! `--format`.
//!
//! Offline and hermetic: `--help` only, no network, no arguments executed.

use assert_cmd::Command;
use sdkt_core::registry::{capabilities, Safety};

/// `sdkt <cmd..> --help` stdout, or `None` when the command does not exist
/// (unknown subcommand → clap usage error).
fn help_for(command: &[&str]) -> Option<String> {
    let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
    cmd.args(command).arg("--help");
    let out = cmd.output().expect("run sdkt --help");
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn every_registered_command_path_exists() {
    for c in capabilities() {
        let help = help_for(c.command)
            .unwrap_or_else(|| panic!("{}: `sdkt {}` has no --help", c.id, c.argv()));
        assert!(
            !help.trim().is_empty(),
            "{}: empty help for `sdkt {}`",
            c.id,
            c.argv()
        );
    }
}

#[test]
fn json_format_claims_match_the_real_cli() {
    let mut mismatches = Vec::new();
    for c in capabilities() {
        // Skip commands whose help needs a value for the leaf (none here: all
        // registry leaves are real subcommands, so --help resolves).
        let Some(help) = help_for(c.command) else {
            mismatches.push(format!("{}: command path not resolvable", c.id));
            continue;
        };
        let accepts_format = help.contains("--format");
        if c.formats.json && !accepts_format {
            mismatches.push(format!(
                "{}: registry claims JSON, but `sdkt {} --help` has no --format",
                c.id,
                c.argv()
            ));
        }
        if !c.formats.json && accepts_format {
            mismatches.push(format!(
                "{}: registry omits JSON, but `sdkt {} --help` accepts --format",
                c.id,
                c.argv()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "registry/CLI drift ({} entries):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn pretty_claim_holds_for_every_capability() {
    // Every described command is invocable from a shell with no output flag,
    // so `pretty` is true across the board; guard against a stray entry
    // claiming otherwise.
    for c in capabilities() {
        assert!(
            c.formats.pretty,
            "{}: every CLI command has a default human-readable output",
            c.id
        );
    }
}

#[test]
fn mutating_capabilities_are_confirmation_gated() {
    // Safety invariant, re-checked against the registry as consumed by an
    // agent: nothing that signs, submits, or funds is reachable without an
    // explicit confirmation flag in the metadata.
    let unsafe_entries: Vec<&str> = capabilities()
        .iter()
        .filter(|c| c.safety == Safety::Mutating && !c.requires_confirmation)
        .map(|c| c.id)
        .collect();
    assert!(
        unsafe_entries.is_empty(),
        "mutating capabilities missing requires_confirmation: {unsafe_entries:?}"
    );
}

#[test]
fn registry_exports_a_versioned_manifest() {
    let json = sdkt_core::registry::to_json().expect("serialize registry");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON manifest");
    assert_eq!(v["schema_version"], sdkt_core::registry::SCHEMA_VERSION);
    assert_eq!(v["tool"], "sdkt");
    assert_eq!(
        v["capabilities"].as_array().map(|a| a.len()),
        Some(capabilities().len())
    );
    // The manifest must be self-sufficient: no field an agent needs may be
    // absent, and safety must always travel with the confirmation flag.
    for entry in v["capabilities"].as_array().expect("array") {
        assert!(entry["id"].is_string(), "missing id: {entry}");
        assert!(entry["command"].is_array(), "missing command: {entry}");
        assert!(entry["formats"]["json"].is_boolean(), "{entry}");
        assert!(entry["exit"]["verdict_can_fail"].is_boolean(), "{entry}");
        assert!(entry["safety"].is_string(), "{entry}");
        assert!(entry["requires_confirmation"].is_boolean(), "{entry}");
        assert!(entry["network"].is_string(), "{entry}");
        assert!(entry["evidence"].is_string(), "{entry}");
    }
}
