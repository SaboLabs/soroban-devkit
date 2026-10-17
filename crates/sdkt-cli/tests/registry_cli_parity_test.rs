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

/// Every flag the registry lists for a capability must actually be accepted
/// by that command. Registry metadata is consumed as an argument contract, so
/// a stale flag (e.g. `--out` where the CLI only has `--output`, or
/// `--dry-run` on a command without it) misleads anything that trusts it.
///
/// Matching compares the leading flag token only, so `--network` never
/// satisfies a claim for `--network-passphrase`.
#[test]
fn declared_flags_exist_in_the_real_cli() {
    let mut stale = Vec::new();
    for c in capabilities() {
        let Some(help) = help_for(c.command) else {
            stale.push(format!("{}: command path not resolvable", c.id));
            continue;
        };
        // Long flags the CLI documents. A line may list a short form first
        // ("-c, --contract <X>"), so collect every `--` token on the line
        // rather than only the first.
        let declared: std::collections::HashSet<&str> = help
            .lines()
            .flat_map(|l| l.split_whitespace())
            .map(|tok| tok.trim_end_matches(','))
            .filter(|tok| tok.starts_with("--"))
            .collect();

        for arg in c.required_args.iter().chain(c.optional_args) {
            let Some(tok) = arg.split_whitespace().next() else {
                continue; // positional placeholder, e.g. "<name>"
            };
            if tok.starts_with("--") && !declared.contains(tok) {
                stale.push(format!(
                    "{}: registry lists {tok}, but `{} --help` does not accept it",
                    c.id,
                    c.argv()
                ));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "registry advertises flags the CLI rejects ({} entries):\n{}",
        stale.len(),
        stale.join("\n")
    );
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
fn cli_rejects_invented_positionals_and_registry_omits_no_required_flags() {
    let mut errors = Vec::new();

    // Verify bidirectional parity for the four capabilities targeted by the argument contract:
    // tx.validate, tx.sign, tx.submit, and tx.build.
    let target_ids = ["tx.validate", "tx.sign", "tx.submit", "tx.build"];

    for id in target_ids {
        let c = sdkt_core::registry::find(id)
            .unwrap_or_else(|| panic!("capability {id} not found in registry"));

        let help =
            help_for(c.command).unwrap_or_else(|| panic!("{}: command path not resolvable", c.id));

        // 1. Required flags direction: every required flag enforced by the CLI
        // in its Usage line must be present in the registry's required_args.
        let usage_line = help
            .lines()
            .find(|l| l.trim_start().starts_with("Usage:"))
            .unwrap_or_default();

        let registry_required_flags: std::collections::HashSet<&str> = c
            .required_args
            .iter()
            .filter_map(|arg| {
                let tok = arg.split_whitespace().next().unwrap_or_default();
                if tok.starts_with("--") {
                    Some(tok)
                } else {
                    None
                }
            })
            .collect();

        for tok in usage_line.split_whitespace() {
            if tok.starts_with("--") && !tok.ends_with(']') {
                let flag =
                    tok.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_');
                if flag.starts_with("--") && !registry_required_flags.contains(flag) {
                    errors.push(format!(
                        "{}: CLI requires flag `{flag}` in usage `{usage_line}`, but registry omits it from required_args",
                        c.id
                    ));
                }
            }
        }

        // 2. Rejected positional direction: every positional that the CLI rejects
        // must not be declared as required positional in the registry.
        let registry_has_positionals = c.required_args.iter().any(|arg| {
            !arg.split_whitespace()
                .next()
                .unwrap_or_default()
                .starts_with("--")
        });

        if registry_has_positionals {
            errors.push(format!(
                "{}: registry declares positional required_args ({:?}), but CLI requires flags instead",
                c.id, c.required_args
            ));
        }

        // Verify that passing an unexpected positional argument is rejected with exit code 2
        // and that stderr explicitly identifies the unexpected positional argument.
        // We supply dummy values for every required flag first so the command does not fail
        // early due to missing required arguments.
        let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
        cmd.args(c.command);
        for arg in c.required_args {
            let tok = arg.split_whitespace().next().unwrap_or_default();
            if tok.starts_with("--") {
                cmd.arg(tok).arg("dummy_val");
            }
        }
        cmd.arg("__unexpected_positional_test__");

        let out = cmd.output().expect("run sdkt with unexpected positional");
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.code() != Some(2) {
            errors.push(format!(
                "{}: CLI unexpectedly accepted positional argument on command that takes no positionals (exit code: {:?})",
                c.id,
                out.status.code()
            ));
        } else if !stderr.contains("unexpected argument '__unexpected_positional_test__'") {
            errors.push(format!(
                "{}: CLI exited with code 2 but stderr did not identify unexpected positional argument: {stderr}",
                c.id
            ));
        }
    }

    assert!(
        errors.is_empty(),
        "registry/CLI parity errors found ({} entries):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

#[test]
fn updated_tx_entries_resolve_against_real_cli() {
    let test_cases = [
        ("tx.validate", vec!["tx", "validate", "--envelope", "AAAA"]),
        (
            "tx.sign",
            vec!["tx", "sign", "--input", "AAA", "--identity", "x"],
        ),
        ("tx.submit", vec!["tx", "submit", "--envelope", "AAA"]),
        (
            "tx.build",
            vec![
                "tx",
                "build",
                "--source",
                "S",
                "--contract",
                "C",
                "--function",
                "f",
            ],
        ),
    ];

    for (id, argv) in test_cases {
        let mut cmd = Command::cargo_bin("sdkt").expect("sdkt binary built");
        cmd.args(&argv);
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("{id}: failed to run sdkt: {e}"));
        let code = out.status.code().unwrap_or(-1);
        assert_ne!(
            code,
            2,
            "{id} invocation `{argv:?}` was rejected by CLI with clap usage error (exit code 2). Stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
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
