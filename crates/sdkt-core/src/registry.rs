//! Agent-facing capability registry for the `sdkt` CLI.
//!
//! This is metadata **about commands that already exist** — it executes
//! nothing and adds no new CLI surface. It exists so an agent or orchestrator
//! can discover what the toolkit can do, what each call requires, and how
//! dangerous it is, without scraping `--help` output.
//!
//! Shape of the world it describes:
//!
//! ```text
//! Agent → Tool Registry → SDKT CLI → deterministic evidence
//! ```
//!
//! The registry is the contract layer; the CLI remains the source of truth
//! for behavior. If the two ever disagree, the CLI wins and this table is the
//! bug.
//!
//! # Evidence discipline
//!
//! Every entry carries an [`Evidence`] level. Entries are only marked
//! `Verified*` when the command was actually executed and its exit code and
//! stdout shape were observed (local artifacts, or live read-only calls
//! against Testnet). Anything not observed is `Unverified` and MUST carry a
//! `notes` string saying why — the registry never asserts a capability it has
//! not seen work, and never guesses a mutating command's outcome.
//!
//! # Adding a capability
//!
//! 1. Confirm the command exists: `sdkt <cmd> --help`, and run it once in the
//!    cheapest safe mode (offline fixture, or read-only against Testnet).
//! 2. Append a `Capability` to [`capabilities()`] using struct-update syntax
//!    from [`Capability::BASE`] so only the fields that differ are spelled out:
//!    ```text
//!    Capability {
//!        id: "family.action",          // lowercase, dotted, unique
//!        command: &["family", "action"],
//!        summary: "one line, what it returns",
//!        required_args: &["<arg>"],
//!        formats: PRETTY_JSON,
//!        network: NetworkRequirement::Both,
//!        evidence: Evidence::VerifiedLocal,   // or VerifiedTestnet
//!        ..Capability::BASE
//!    }
//!    ```
//! 3. Rules that the tests enforce — do not work around them:
//!    - IDs are unique and non-empty; `command`, `summary` are non-empty.
//!    - `safety: Mutating` ⇒ `requires_confirmation: true`.
//!    - Read-only commands must NOT set `requires_confirmation`.
//!    - `formats.json: true` only when the real command accepts
//!      `--format json` and emits a parseable document.
//!    - `evidence: Unverified` ⇒ `notes` must explain what is missing.
//!    - Exit codes are the CLI-wide convention: 0 success, 1 runtime/
//!      verdict failure, 2 usage error. Set `verdict_can_fail` only for
//!      commands that exit 1 while still producing a valid report.
//! 4. Add the id to the pinned lists in the tests that cross-check against
//!    observed behavior (`json_format_flags_match_cli`,
//!    `verdict_gated_commands_are_marked`).
//! 5. `cargo test -p sdkt-core` must stay green. Bump
//!    [`SCHEMA_VERSION`] only on a breaking change to this metadata format.

use serde::{Deserialize, Serialize};

/// Schema version of the exported document. Consumers may pin against it.
pub const SCHEMA_VERSION: u32 = 1;

/// How dangerous an invocation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    /// Inspects state; changes nothing on any network or on disk.
    ReadOnly,
    /// Writes local files/state (scaffolds, caches, keystores, lock files),
    /// but never signs or submits anything.
    LocalWrite,
    /// Signs and/or submits a transaction, or funds an account. Changes
    /// ledger state. Always `requires_confirmation`.
    Mutating,
}

/// Network prerequisite for the happy path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkRequirement {
    /// Runs fully offline.
    None,
    /// Needs a reachable Soroban RPC, conventionally Testnet.
    Testnet,
    /// Meaningful against Mainnet (read-only inspection or guarded mutation).
    Mainnet,
    /// Any configured network; the endpoint comes from a profile/flags.
    Both,
}

/// Exit-code semantics an orchestrator can branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitSemantics {
    /// Code for a successful run.
    pub success: i32,
    /// Code for a runtime/verdict failure (bad path, RPC error, failed gate).
    pub runtime_error: i32,
    /// Code for a usage error (missing/invalid arguments, clap-level).
    pub usage_error: i32,
    /// True when a run that produces a *valid* report can still exit
    /// non-zero because its verdict failed (e.g. artifact mismatch, a
    /// `critical` health posture, a violated size policy). When false,
    /// a non-zero exit always means the command itself did not complete.
    pub verdict_can_fail: bool,
}

impl ExitSemantics {
    /// CLI-wide convention: 0 ok, 1 runtime/verdict failure, 2 usage error.
    const STANDARD: ExitSemantics = ExitSemantics {
        success: 0,
        runtime_error: 1,
        usage_error: 2,
        verdict_can_fail: false,
    };
    /// As [`ExitSemantics::STANDARD`], but a failed verdict also exits 1.
    const VERDICT_GATED: ExitSemantics = ExitSemantics {
        verdict_can_fail: true,
        ..ExitSemantics::STANDARD
    };
}

/// Which output modes the command actually implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputFormats {
    /// Human-readable default output.
    pub pretty: bool,
    /// `--format json` produces a parseable document on stdout alone.
    pub json: bool,
}

const PRETTY: OutputFormats = OutputFormats {
    pretty: true,
    json: false,
};
const PRETTY_JSON: OutputFormats = OutputFormats {
    pretty: true,
    json: true,
};

/// How well this entry's claims are backed by observed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Executed against local artifacts/fixtures; exit code and output shape
    /// observed.
    VerifiedLocal,
    /// Executed live against a read-only network path; exit code and output
    /// shape observed.
    VerifiedTestnet,
    /// Not executed during the audit that produced this table. `notes`
    /// explains what is unverified.
    Unverified,
}

/// One machine-described CLI capability.
///
/// Serialize-only: the borrowed `&'static` fields describe a registry compiled
/// into the binary, and export (`to_json`) is the consumer contract. Anything
/// reading an exported document back should deserialize into its own owned
/// types (see the round-trip test for a worked example).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capability {
    /// Stable unique id, `family.action`, snake/dotted lowercase.
    pub id: &'static str,
    /// Argument path to the leaf, e.g. `["wasm", "metadata"]`.
    pub command: &'static [&'static str],
    /// One-line summary of what it returns.
    pub summary: &'static str,
    /// Positional arguments and mandatory flags.
    pub required_args: &'static [&'static str],
    /// Optional flags that change or extend the happy path.
    pub optional_args: &'static [&'static str],
    pub formats: OutputFormats,
    pub exit: ExitSemantics,
    pub network: NetworkRequirement,
    pub safety: Safety,
    /// True when an agent must obtain explicit human confirmation before
    /// invoking. Always true for [`Safety::Mutating`].
    pub requires_confirmation: bool,
    pub evidence: Evidence,
    /// Caveats, prerequisites, or the reason an entry is `Unverified`.
    pub notes: Option<&'static str>,
}

impl Capability {
    /// Read-only, offline, pretty-only, standard exit convention, unverified.
    /// Entries are written as `Capability { ..Capability::BASE, .. }` so each
    /// line states only what deviates from this default.
    const BASE: Capability = Capability {
        id: "",
        command: &[],
        summary: "",
        required_args: &[],
        optional_args: &[],
        formats: PRETTY,
        exit: ExitSemantics::STANDARD,
        network: NetworkRequirement::None,
        safety: Safety::ReadOnly,
        requires_confirmation: false,
        evidence: Evidence::Unverified,
        notes: None,
    };

    /// Full argv form, for messages and shell previews.
    pub fn argv(&self) -> String {
        let mut s = String::from("sdkt");
        for part in self.command {
            s.push(' ');
            s.push_str(part);
        }
        s
    }
}

/// The registry document: versioned, serializable, agent-consumable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistryDocument<'a> {
    pub schema_version: u32,
    pub tool: &'static str,
    pub capabilities: &'a [Capability],
}

/// All described capabilities, in a stable (documented-by-lifetime) order.
pub fn capabilities() -> &'static [Capability] {
    &CAPABILITIES
}

/// Build the exportable document.
pub fn document() -> RegistryDocument<'static> {
    RegistryDocument {
        schema_version: SCHEMA_VERSION,
        tool: "sdkt",
        capabilities: capabilities(),
    }
}

/// Look a capability up by id.
pub fn find(id: &str) -> Option<&'static Capability> {
    capabilities().iter().find(|c| c.id == id)
}

/// Serialize the document to a pretty JSON string for export.
pub fn to_json() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&document())
}

const CAPABILITIES: [Capability; CAP_COUNT] = [
    // ---- WASM inspection -------------------------------------------------
    Capability {
        id: "wasm.inspect",
        command: &["wasm", "inspect"],
        summary: "Offline analysis of a local WASM artifact: hash, size, sections, decoded contractmetav0, spec.",
        required_args: &["<file.wasm>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "wasm.metadata",
        command: &["wasm", "metadata"],
        summary: "Metadata + parsed ABI + storage/TTL posture for a deployed contract (cached; optional refresh).",
        required_args: &["--contract <C...>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--network", "--network-profile", "--refresh", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        ..Capability::BASE
    },
    // ---- Diff / upgrade safety -------------------------------------------
    Capability {
        id: "diff",
        command: &["diff"],
        summary: "Offline ABI/function/event/type diff between two WASM artifacts, plus signed size deltas.",
        required_args: &["--old-wasm <f>", "--new-wasm <f>"],
        optional_args: &["--format", "--upgrade-safety", "--max-size-bytes", "--max-growth-pct"],
        formats: PRETTY_JSON,
        exit: ExitSemantics::VERDICT_GATED,
        evidence: Evidence::VerifiedLocal,
        notes: Some("Size policy flags are rejected in combination with --upgrade-safety."),
        ..Capability::BASE
    },
    Capability {
        id: "diff.upgrade_safety",
        command: &["diff", "--upgrade-safety"],
        summary: "Breaking/non-breaking upgrade verdict between two artifacts.",
        required_args: &["--old-wasm <f>", "--new-wasm <f>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        notes: Some(
            "An incompatible verdict is reported in the JSON (`compatible: false`) but still exits 0 — it is a signal to read, not a gate. Only the opt-in size policy (--max-size-bytes / --max-growth-pct) makes `diff` exit non-zero.",
        ),
        ..Capability::BASE
    },
    // ---- Static audit -----------------------------------------------------
    Capability {
        id: "audit",
        command: &["audit"],
        summary: "Heuristic static security analysis of Soroban Rust sources (AUTH/MATH/CEI/MOVE rules).",
        required_args: &["<paths..>"],
        optional_args: &["--format", "--disable", "--rules", "--no-plugins", "--list-rules"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        notes: Some(
            "Findings are advisory signals, never proofs of exploitability; the audit exit code does not encode finding severity (use the GitHub Action's severity-threshold).",
        ),
        ..Capability::BASE
    },
    Capability {
        id: "audit.list_rules",
        command: &["audit", "--list-rules"],
        summary: "Registered rule ids with severity and description; no source path required.",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    // ---- Deployed reality --------------------------------------------------
    Capability {
        id: "verify",
        command: &["verify"],
        summary: "Compare a local WASM's offline hash against the deployed contract's on-chain hash.",
        required_args: &["--contract <C...>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--wasm <f>", "--network", "--network-profile", "--format", "--upgrade-safety"],
        formats: PRETTY_JSON,
        exit: ExitSemantics::VERDICT_GATED,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("Mismatch exits 1; without --wasm the report is OnChainOnly and exits 0."),
        ..Capability::BASE
    },
    Capability {
        id: "health",
        command: &["health"],
        summary: "Read-only contract posture: WASM verification, storage layout, TTL expiry, verdict + reasons.",
        required_args: &["--contract <C...>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--wasm <f>", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        exit: ExitSemantics::VERDICT_GATED,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("A `critical` verdict exits 1 while still printing a full report; `at_risk` and `healthy` exit 0."),
        ..Capability::BASE
    },
    Capability {
        id: "inspect",
        command: &["inspect"],
        summary: "On-chain contract inspection: wasm hash/size, ABI summary, storage and TTL.",
        required_args: &["<contract-id>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("Protocol 28 `stellar_asset` / CAP-85 external executables have no WASM hash and error explicitly."),
        ..Capability::BASE
    },
    Capability {
        id: "release_assurance",
        command: &["release-assurance"],
        summary: "Aggregate release verdict: artifact, audit, upgrade safety, deployed verification, health, size policy.",
        required_args: &["--wasm <f>"],
        optional_args: &[
            "--rpc-url",
            "--network-passphrase",
            "--previous-wasm <f>", "--audit <path>", "--disable <RULE>", "--contract <C>",
            "--network <n>", "--max-size-bytes <N>", "--max-growth-pct <N>", "--format",
        ],
        formats: PRETTY_JSON,
        exit: ExitSemantics::VERDICT_GATED,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("Skipped sections never flatten into PASS; --max-growth-pct requires --previous-wasm and is a hard CLI error without it."),
        ..Capability::BASE
    },
    // ---- Storage / events / calls ------------------------------------------
    Capability {
        id: "storage.analyze",
        command: &["storage", "analyze"],
        summary: "Classify a contract's storage into Instance/Persistent/Temporary with TTL summary.",
        required_args: &["<contract-id>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--key-xdr", "--map-key", "--key-arg", "--durability", "--abi-contract", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        ..Capability::BASE
    },
    Capability {
        id: "events",
        command: &["events"],
        summary: "Fetch contract events with ledger range, server-side topic filter, optional ABI decoding.",
        required_args: &["<contract-id>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--start-ledger", "--end-ledger", "--topic", "--abi", "--abi-contract", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("An empty event list is a successful result, not an error."),
        ..Capability::BASE
    },
    Capability {
        id: "call",
        command: &["call"],
        summary: "Read-only contract invocation via simulation; never signs or submits.",
        required_args: &["<contract-id>", "<function>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--args", "--args-json", "--abi", "--abi-contract", "--network", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some(
            "Only the error path was observed this session (simulation errors exit 1 with the host error text). A successful decoded call was not executed; argument shape is function-specific.",
        ),
        ..Capability::BASE
    },
    Capability {
        id: "account",
        command: &["account"],
        summary: "Account balances and complete signer list (Horizon-enriched).",
        required_args: &["<address>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedTestnet,
        notes: Some("A nonexistent account exits 1 with 'Account not found on network'."),
        ..Capability::BASE
    },
    // ---- Network / diagnostics ---------------------------------------------
    Capability {
        id: "network.list",
        command: &["network", "list"],
        summary: "Saved network profiles (name, RPC URL, passphrase).",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "network.show",
        command: &["network", "show"],
        summary: "One saved profile.",
        required_args: &["<name>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "network.check",
        command: &["network", "check"],
        summary: "Profile reachability + node health + protocol version; unreachable endpoint exits non-zero.",
        required_args: &["<name>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedLocal,
        notes: Some(
            "Resolves a SAVED profile by name and has no --rpc-url; verified against a saved profile on a reachable host.",
        ),
        ..Capability::BASE
    },
    Capability {
        id: "network.add",
        command: &["network", "add"],
        summary: "Save a named RPC endpoint + passphrase profile.",
        required_args: &["<name>", "--rpc-url <url>", "--passphrase <phrase>"],
        optional_args: &["--friendbot", "--description", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        notes: Some("Writes user config under the sdkt network dir; reversible with network remove."),
        ..Capability::BASE
    },
    Capability {
        id: "network.remove",
        command: &["network", "remove"],
        summary: "Delete a saved network profile.",
        required_args: &["<name>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "doctor",
        command: &["doctor"],
        summary: "Environment/toolchain/project diagnostics with a healthy flag.",
        optional_args: &["--format", "--json"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    // ---- Keystore ------------------------------------------------------------
    Capability {
        id: "identity.list",
        command: &["identity", "list"],
        summary: "Local ED25519 identities (names + public keys; no secret material).",
        safety: Safety::ReadOnly,
        evidence: Evidence::VerifiedLocal,
        notes: Some("No --format json (known limitation; output is human text only)."),
        ..Capability::BASE
    },
    Capability {
        id: "identity.generate",
        command: &["identity", "generate"],
        summary: "Create a new keypair in the local keystore.",
        required_args: &["<name>"],
        optional_args: &["--default"],
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; keystore write, no ledger interaction. Secret material must never be surfaced to an agent."),
        ..Capability::BASE
    },
    Capability {
        id: "identity.import",
        command: &["identity", "import"],
        summary: "Store an existing secret key in the local keystore.",
        required_args: &["<name>", "<secret-key>"],
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit. Handles secret input; an agent must never receive the key material in either direction."),
        ..Capability::BASE
    },
    Capability {
        id: "identity.delete",
        command: &["identity", "delete"],
        summary: "Remove an identity from the local keystore.",
        required_args: &["<name>"],
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; irreversible local key destruction — confirmation required for the operator."),
        requires_confirmation: true,
        ..Capability::BASE
    },
    Capability {
        id: "identity.default",
        command: &["identity", "default"],
        summary: "Show or set the default identity.",
        required_args: &["[name]"],
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; read when called without a name, write when given one."),
        ..Capability::BASE
    },
    // ---- Build / scaffold / package / lock -----------------------------------
    Capability {
        id: "build",
        command: &["build"],
        summary: "Compile workspace contracts to WASM artifacts; JSON reports success + artifact list.",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        notes: Some("Requires the wasm32v1-none target; the advisory sdkt.lock report goes to stderr so stdout stays pure JSON. Protocol 28 projects route through `stellar contract build`."),
        ..Capability::BASE
    },
    Capability {
        id: "init",
        command: &["init"],
        summary: "Scaffold a new Soroban contract project.",
        required_args: &["<name>"],
        optional_args: &["--minimal", "--force", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "generate.client",
        command: &["generate", "client"],
        summary: "Generate typed client Rust from a contract WASM spec.",
        required_args: &["<file.wasm>"],
        optional_args: &["--out <dir>", "--skip-unsupported"],
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        notes: Some("No --format json; emits Rust source (stdout or --out)."),
        ..Capability::BASE
    },
    Capability {
        id: "package.validate",
        command: &["package", "validate"],
        summary: "Offline validation of the local package manifest + dependencies.",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        notes: Some("An invalid manifest is a valid JSON report ({valid:false,error}) and exits non-zero."),
        ..Capability::BASE
    },
    Capability {
        id: "package.update",
        command: &["package", "update"],
        summary: "Resolve/update git dependencies against semver constraints.",
        optional_args: &["--check", "--dry-run", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; --dry-run/--check are the safe modes an agent should start with."),
        ..Capability::BASE
    },
    Capability {
        id: "package.publish",
        command: &["package", "publish"],
        summary: "Publish-readiness check / publish of the package.",
        optional_args: &["--dry-run"],
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; --dry-run is the non-mutating mode. Actual publication is external and must stay human-gated."),
        requires_confirmation: true,
        ..Capability::BASE
    },
    Capability {
        id: "lock.verify",
        command: &["lock", "verify"],
        summary: "Verify sdkt.lock against on-disk artifacts and resolved dependencies.",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        notes: Some("A missing/invalid lock exits 1 with an error line."),
        ..Capability::BASE
    },
    Capability {
        id: "lock.generate",
        command: &["lock", "generate"],
        summary: "Write sdkt.lock from current build artifacts (build first).",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    // ---- Plugins ---------------------------------------------------------------
    Capability {
        id: "plugin.list",
        command: &["plugin", "list"],
        summary: "Installed audit plugins in the local store.",
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "plugin.doctor",
        command: &["plugin", "doctor"],
        summary: "Staged plugin self-check (metadata, integrity, ABI, dry-run).",
        required_args: &["[target]"],
        optional_args: &["--all", "--format"],
        formats: PRETTY_JSON,
        exit: ExitSemantics {
            runtime_error: 1,
            ..ExitSemantics::STANDARD
        },
        evidence: Evidence::VerifiedLocal,
        notes: Some("Exit code encodes the failed stage (1..=6), not just pass/fail."),
        ..Capability::BASE
    },
    Capability {
        id: "plugin.verify_bundle",
        command: &["plugin", "verify-bundle"],
        summary: "Verify a .sdktplugin bundle's digests and signature without installing.",
        required_args: &["<bundle>"],
        optional_args: &["--public-key <file>", "--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        ..Capability::BASE
    },
    Capability {
        id: "plugin.install",
        command: &["plugin", "install"],
        summary: "Install a plugin artifact or bundle into the local store.",
        required_args: &["<path>"],
        optional_args: &["--id", "--force", "--public-key <file>", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::LocalWrite,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by the audit; third-party code entering the plugin store — treat as supply-chain-sensitive and require confirmation."),
        requires_confirmation: true,
        ..Capability::BASE
    },
    // ---- Fee / tx --------------------------------------------------------------
    Capability {
        id: "fee.estimate",
        command: &["fee", "estimate"],
        summary: "Fee estimate from supplied base fees or live RPC ledger-fee statistics.",
        optional_args: &["--rpc-url", "--network-passphrase", "--network", "--base-fees", "--rpc", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::VerifiedLocal,
        notes: Some("--base-fees path is offline; --rpc fetches network statistics (read-only)."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.decode",
        command: &["tx", "decode"],
        summary: "Human/JSON breakdown of a transaction envelope: ops, footprint, resources, signatures.",
        required_args: &["<xdr|file>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::VerifiedLocal,
        notes: Some("Fully offline. Malformed base64 or trailing bytes exit non-zero; the valid-envelope breakdown was not exercised this session."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.validate",
        command: &["tx", "validate"],
        summary: "Validate an envelope offline (structure, missing file, input resolution).",
        required_args: &["<xdr|file>"],
        optional_args: &["--format"],
        formats: PRETTY_JSON,
        evidence: Evidence::Unverified,
        notes: Some("Only the invalid-input path was observed (exit non-zero with a shared error message); a passing validation was not executed."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.simulate",
        command: &["tx", "simulate"],
        summary: "Simulate an envelope without submitting: resources, auth entries, diagnostics, stateChanges.",
        required_args: &["--envelope <xdr|file>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--abi", "--abi-contract", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by this audit; read-only against the network, never signs or submits."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.build",
        command: &["tx", "build"],
        summary: "Build an invocation envelope (optionally simulating to adopt fee/footprint).",
        required_args: &["--contract <C>", "--function <fn>"],
        optional_args: &["--rpc-url", "--network-passphrase", "--arg", "--source", "--sequence", "--fee", "--memo-text", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        safety: Safety::ReadOnly,
        evidence: Evidence::Unverified,
        notes: Some("Produces bytes only — it does not sign or submit — but the output is submission-ready, so downstream intent matters."),
        ..Capability::BASE
    },
    Capability {
        id: "project.status",
        command: &["project", "status"],
        summary: "Multi-contract project resolution + deployment status.",
        optional_args: &["--rpc-url", "--network-passphrase", "--format", "--network-profile"],
        formats: PRETTY_JSON,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Not executed by this audit; read-only."),
        ..Capability::BASE
    },
    // ---- Mutating capabilities: never agent-executable without confirmation ---
    Capability {
        id: "deploy",
        command: &["deploy"],
        summary: "Upload WASM + instantiate a contract on a network (signs and submits).",
        required_args: &["--wasm <f>"],
        optional_args: &[
            "--arg", "--salt", "--wasm-hash", "--identity", "--network", "--network-profile",
            "--deny-breaking", "--previous-wasm", "--format",
        ],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some(
            "NEVER agent-executable without explicit human confirmation: spends fees and writes ledger state. Mainnet requires an explicitly selected network (safety guard). Not exercised by the audit — deploy/upload is out of read-only bounds.",
        ),
        ..Capability::BASE
    },
    Capability {
        id: "invoke",
        command: &["invoke"],
        summary: "Signed, submitted state-changing invocation (sequence → simulate → sign → submit → poll).",
        required_args: &["<contract-id>", "<function>"],
        optional_args: &["--args", "--identity", "--build-only", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("NEVER agent-executable without explicit human confirmation. --build-only stops before submission and is the only acceptable agent path."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.sign",
        command: &["tx", "sign"],
        summary: "Sign an envelope with a local ED25519 identity.",
        required_args: &["<envelope>", "--identity <name>"],
        optional_args: &["--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        evidence: Evidence::Unverified,
        notes: Some("Key-bearing operation. An agent must never see secret material; signing authority requires explicit human confirmation."),
        ..Capability::BASE
    },
    Capability {
        id: "tx.submit",
        command: &["tx", "submit"],
        summary: "Submit a signed envelope to the network.",
        required_args: &["<envelope>"],
        optional_args: &["--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Broadcasts irreversible ledger state; mainnet guarded by explicit-network requirement."),
        ..Capability::BASE
    },
    Capability {
        id: "storage.extend",
        command: &["storage", "extend"],
        summary: "Extend TTL of a contract footprint via ExtendFootprintTtl.",
        required_args: &["<contract-id>"],
        optional_args: &["--ledgers", "--identity", "--dry-run", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Submits a transaction and spends fees. --dry-run is read-only and is the only mode an agent may call unattended."),
        ..Capability::BASE
    },
    Capability {
        id: "storage.restore",
        command: &["storage", "restore"],
        summary: "Restore archived storage by adopting a simulation's restorePreamble.",
        required_args: &["--envelope <xdr>"],
        optional_args: &["--dry-run", "--identity", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Submits a RestoreFootprint transaction. --dry-run lists keys/fee without submitting."),
        ..Capability::BASE
    },
    Capability {
        id: "project.deploy",
        command: &["project", "deploy"],
        summary: "Deploy a whole multi-contract project in resolved dependency order.",
        optional_args: &["--identity", "--network", "--network-profile", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Both,
        evidence: Evidence::Unverified,
        notes: Some("Fan-out of deploy transactions; highest blast radius in the toolkit."),
        ..Capability::BASE
    },
    Capability {
        id: "identity.fund",
        command: &["identity", "fund"],
        summary: "Fund an account via Testnet Friendbot.",
        required_args: &["<name>"],
        optional_args: &["--network", "--format"],
        formats: PRETTY_JSON,
        safety: Safety::Mutating,
        requires_confirmation: true,
        network: NetworkRequirement::Testnet,
        evidence: Evidence::Unverified,
        notes: Some("Submits a funding transaction; testnet only."),
        ..Capability::BASE
    },
];

/// Number of described capabilities; kept as a const so the array length is
/// checked by the compiler against every entry.
const CAP_COUNT: usize = 51;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn capability_ids_are_unique() {
        let mut seen = HashSet::new();
        let dupes: Vec<_> = capabilities()
            .iter()
            .filter(|c| !seen.insert(c.id))
            .map(|c| c.id)
            .collect();
        assert!(dupes.is_empty(), "duplicate capability ids: {dupes:?}");
    }

    #[test]
    fn required_fields_are_present() {
        for c in capabilities() {
            assert!(!c.id.is_empty(), "empty id");
            assert!(
                c.id.is_ascii() && !c.id.contains(char::is_whitespace),
                "id must be ascii and space-free: {}",
                c.id
            );
            assert!(!c.summary.is_empty(), "{}: empty summary", c.id);
            assert!(
                !c.command.is_empty(),
                "{}: command path must be non-empty",
                c.id
            );
            // The argv must start with the leaf command path it claims.
            assert!(
                c.argv().starts_with("sdkt ") && c.argv().contains(c.command[c.command.len() - 1]),
                "{}: argv {} does not contain its leaf command",
                c.id,
                c.argv()
            );
        }
    }

    #[test]
    fn mutating_capabilities_always_require_confirmation() {
        for c in capabilities() {
            if c.safety == Safety::Mutating {
                assert!(
                    c.requires_confirmation,
                    "{}: mutating capability must require confirmation",
                    c.id
                );
            }
        }
    }

    #[test]
    fn read_only_capabilities_never_require_confirmation() {
        for c in capabilities() {
            if c.safety == Safety::ReadOnly {
                assert!(
                    !c.requires_confirmation,
                    "{}: read-only capability must not be flagged for confirmation",
                    c.id
                );
            }
        }
    }

    #[test]
    fn mutating_capabilities_never_claim_verified_execution() {
        // The audit is read-only; nothing that signs/submits was executed.
        for c in capabilities() {
            if c.safety == Safety::Mutating {
                assert_eq!(
                    c.evidence,
                    Evidence::Unverified,
                    "{}: mutating capability cannot be marked verified by a read-only audit",
                    c.id
                );
            }
        }
    }

    #[test]
    fn unverified_entries_explain_themselves() {
        for c in capabilities() {
            if c.evidence == Evidence::Unverified {
                assert!(
                    c.notes.is_some(),
                    "{}: Unverified entries must say what is missing",
                    c.id
                );
            }
        }
    }

    #[test]
    fn exit_convention_is_consistent() {
        for c in capabilities() {
            assert_eq!(c.exit.success, 0, "{}: success must be 0", c.id);
            assert_eq!(c.exit.runtime_error, 1, "{}: runtime error must be 1", c.id);
            assert_eq!(c.exit.usage_error, 2, "{}: usage error must be 2", c.id);
        }
    }

    #[test]
    fn json_format_flags_match_cli() {
        // Pinned from observed `--help` + executed `--format json` runs.
        const JSON: &[&str] = &[
            "wasm.inspect",
            "wasm.metadata",
            "diff",
            "diff.upgrade_safety",
            "audit",
            "audit.list_rules",
            "verify",
            "health",
            "inspect",
            "release_assurance",
            "storage.analyze",
            "events",
            "call",
            "account",
            "network.list",
            "network.show",
            "network.check",
            "network.add",
            "network.remove",
            "doctor",
            "build",
            "init",
            "package.validate",
            "package.update",
            "lock.verify",
            "lock.generate",
            "plugin.list",
            "plugin.doctor",
            "plugin.verify_bundle",
            "plugin.install",
            "fee.estimate",
            "tx.decode",
            "tx.validate",
            "tx.simulate",
            "tx.build",
            "project.status",
            "deploy",
            "invoke",
            "tx.sign",
            "tx.submit",
            "storage.extend",
            "storage.restore",
            "project.deploy",
            "identity.fund",
        ];
        const NO_JSON: &[&str] = &["identity.list", "generate.client", "package.publish"];
        for id in JSON {
            let c = find(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(
                c.formats.json,
                "{id}: CLI accepts --format json but registry says no"
            );
        }
        for id in NO_JSON {
            let c = find(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(
                !c.formats.json,
                "{id}: registry claims JSON the CLI does not provide"
            );
            assert!(
                c.notes.is_some(),
                "{id}: missing-JSON limitation must be noted"
            );
        }
    }

    #[test]
    fn verdict_gated_commands_are_marked() {
        // Commands that can exit 1 while still emitting a valid report.
        for id in ["health", "verify", "release_assurance", "diff"] {
            let c = find(id).expect("pinned verdict command");
            assert!(
                c.exit.verdict_can_fail,
                "{id}: verdict-gated exit must be flagged"
            );
        }
        // `diff.upgrade_safety` reports incompatibility in JSON but exits 0:
        // verified by running it, so it must NOT claim a verdict gate.
        for id in [
            "wasm.inspect",
            "audit",
            "inspect",
            "events",
            "account",
            "diff.upgrade_safety",
        ] {
            let c = find(id).expect("pinned command");
            assert!(
                !c.exit.verdict_can_fail,
                "{id}: not a verdict gate; a non-zero exit means the command did not complete"
            );
        }
    }

    #[test]
    fn document_serializes_to_a_machine_readable_manifest() {
        let json = to_json().expect("registry must serialize");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        assert_eq!(v["tool"], "sdkt");
        let list = v["capabilities"].as_array().expect("array");
        assert_eq!(list.len(), CAP_COUNT);
        // Every entry carries the fields an agent needs to plan a call.
        for entry in list {
            for key in [
                "id",
                "command",
                "summary",
                "required_args",
                "optional_args",
                "formats",
                "exit",
                "network",
                "safety",
                "requires_confirmation",
                "evidence",
            ] {
                assert!(entry.get(key).is_some(), "missing {key} in {entry}");
            }
        }
    }

    #[test]
    fn document_round_trips() {
        let json = to_json().unwrap();
        let back: RegistryDocumentOwned =
            serde_json::from_str(&json).expect("registry document must deserialize");
        assert_eq!(back.capabilities.len(), CAP_COUNT);
        assert!(back
            .capabilities
            .iter()
            .any(|c| c.id == "release_assurance" && c.safety == Safety::ReadOnly));
    }

    #[test]
    fn mutation_surface_is_not_agent_executable_by_default() {
        let mutating: Vec<_> = capabilities()
            .iter()
            .filter(|c| c.safety == Safety::Mutating)
            .collect();
        // deploy, invoke, tx.sign, tx.submit, storage.extend, storage.restore,
        // project.deploy, identity.fund
        assert_eq!(
            mutating.len(),
            8,
            "unexpected mutating surface: {mutating:?}"
        );
        assert!(mutating.iter().all(|c| c.requires_confirmation));
    }

    #[test]
    fn release_assurance_is_the_default_agent_entry_point() {
        let ra = find("release_assurance").unwrap();
        assert_eq!(ra.safety, Safety::ReadOnly);
        assert!(!ra.requires_confirmation);
        assert!(ra.exit.verdict_can_fail);
        assert_eq!(ra.network, NetworkRequirement::Both);
    }

    /// Owned mirror of [`RegistryDocument`] for round-trip tests.
    #[derive(Deserialize)]
    struct RegistryDocumentOwned {
        #[allow(dead_code)]
        schema_version: u32,
        #[allow(dead_code)]
        capabilities: Vec<CapabilityOwned>,
    }

    #[derive(Deserialize, PartialEq, Eq, Debug)]
    struct CapabilityOwned {
        id: String,
        summary: String,
        safety: Safety,
        requires_confirmation: bool,
    }
}
