use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use sdkt_core::fee::{FeeConfig, FeeEstimator, LedgerFeeSample, NetworkKind};
use sdkt_core::fetch::DependencyFetcher;
use sdkt_core::{DevKitConfig, OutputFormat};
use sdkt_rpc::inspect::StorageSummary;
use sdkt_rpc::wasm::get_wasm_bytecode;
use sdkt_rpc::{
    estimate_dynamic_fee, extend_footprint, first_topic_filter, get_contract_events_filtered,
    get_next_sequence, get_ttl_info, get_wasm_metadata, inspect_account, inspect_contract,
    inspect_transaction, read_contract_state, restore_footprint, simulate_transaction,
    SorobanRpcClient, StorageKeyInfo, TtlInfoSummary,
};
use sdkt_storage::WasmCache;
use sdkt_storage::{
    NetworkStore, StorageAnalyzer, DEFAULT_SUGGESTED_LEDGERS, EXPIRING_SOON_LEDGERS,
};
use sdkt_wasm::spec::parse_contract_spec;
use sdkt_xdr::abi_decode::decode_event_topics;
use sdkt_xdr::decode;
use sdkt_xdr::{
    build_invoke_transaction, sign_transaction, Ed25519Signer, InvokeTransactionParams, Network,
    SigningError, SigningOptions,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process;

use commands::network::{
    network_is_explicit, resolve_network_config, resolve_rpc_client, resolve_rpc_client_mutating,
    resolve_target_network, NetworkAction,
};

mod commands;

/// Version string used by `sdkt --version` / `sdkt --long-version` ().
///
/// Default (`provenance` feature off): returns just the semantic version, so
/// the build stays reproducible and identical to pre- releases.
///
/// When compiled with `--features provenance`, appends an optional
/// `commit@date` provenance suffix supplied at build time via the
/// `SDKT_GIT_COMMIT` / `SDKT_BUILD_DATE` environment variables. If those are
/// absent the provenance line is simply omitted — there is never any implicit
/// `git` invocation that would make the binary non-reproducible.
///
/// Returns `&'static str` because clap's `long_version` requires a static
/// string; the provenance branch leases a boxed string (one-time, at command
/// construction) which is intentional and harmless for a CLI binary.
fn sdkt_version_string() -> &'static str {
    let base = env!("CARGO_PKG_VERSION");
    #[cfg(feature = "provenance")]
    {
        let commit = option_env!("SDKT_GIT_COMMIT");
        let date = option_env!("SDKT_BUILD_DATE");
        match (commit, date) {
            (Some(c), Some(d)) => {
                Box::leak(format!("{} (commit {} built {})", base, c, d).into_boxed_str())
            }
            (Some(c), None) => Box::leak(format!("{} (commit {})", base, c).into_boxed_str()),
            (None, Some(d)) => Box::leak(format!("{} (built {})", base, d).into_boxed_str()),
            (None, None) => base,
        }
    }
    #[cfg(not(feature = "provenance"))]
    {
        base
    }
}

#[cfg(test)]
mod storage_label_tests {
    use super::*;

    #[test]
    fn projects_matching_union_key_and_falls_back_for_unknown_key() {
        let spec: sdkt_wasm::ContractSpec = serde_json::from_value(serde_json::json!({
            "env_meta": null, "functions": [], "events": [],
            "custom_types": [{
                "name": "DataKey", "kind": "union", "doc": "", "type_args": [],
                "bytes_n": null,
                "members": [{"name": "Balance", "doc": "", "types": [{
                    "name": "u32", "kind": "primitive", "doc": "", "members": [],
                    "type_args": [], "bytes_n": null
                }], "value": null}]
            }]
        }))
        .unwrap();
        let contract = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
        let key = resolve_storage_read_key(
            contract,
            None,
            Some("Balance"),
            &["u32:7".to_string()],
            false,
            "persistent",
        )
        .unwrap();
        assert_eq!(storage_key_label(&key, &spec), "DataKey::Balance(u32)");
        let unknown =
            resolve_storage_read_key(contract, None, Some("Unknown"), &[], false, "persistent")
                .unwrap();
        assert_eq!(storage_key_label(&unknown, &spec), unknown);
    }
}

/// Reusable network-resolution flags shared by every command that talks to a
/// Soroban/Stellar RPC endpoint.
///
/// Flattened into those commands via `#[command(flatten)]` so the resolution
/// semantics stay identical everywhere (no copy/paste).
#[derive(Args, Clone, Debug, Default)]
pub(crate) struct NetworkArgs {
    /// Use a saved network profile (see `sdkt network add`) for the RPC URL and
    /// network passphrase. Overrides .sdkt.toml defaults.
    #[arg(long, value_name = "NAME", global = true)]
    network_profile: Option<String>,
    /// Explicit RPC endpoint URL. Overrides any profile and .sdkt.toml value.
    #[arg(long, value_name = "URL", global = true)]
    rpc_url: Option<String>,
    /// Explicit network passphrase. Overrides any profile and .sdkt.toml value.
    #[arg(long, value_name = "PASSPHRASE", global = true)]
    network_passphrase: Option<String>,
}
/// Adapter that makes a closed consumer (EPIPE / `BrokenPipe`) look like a
/// successful write.
///
/// `clap_complete::generate` writes the script to the provided `Write` and
/// (in this version) unwraps write errors internally. When the consumer closes
/// the pipe early — e.g. `sdkt completions bash | head` — the underlying write
/// fails with `BrokenPipe`, which would otherwise panic. By mapping that one
/// error to `Ok`, downstream writers never see it and `sdkt` exits cleanly.
/// Every other I/O error is passed through unchanged, preserving the existing
/// failure behavior for real write problems.
struct BrokenPipeOk<W: Write>(W);

impl<W: Write> Write for BrokenPipeOk<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.write(buf) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(buf.len()),
            other => other,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.flush() {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            other => other,
        }
    }
}

/// Soroban DevKit — unified toolkit for Stellar/Soroban development.
#[derive(Parser)]
#[command(name = "sdkt")]
#[command(about = "Soroban DevKit — unified toolkit for Stellar/Soroban development")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(long_version = sdkt_version_string())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decode base64-encoded XDR to JSON
    Decode {
        /// Base64 XDR string to decode. Optional when --file is provided.
        #[arg(value_name = "XDR")]
        payload: Option<String>,
        #[arg(short, long, value_name = "TYPE")]
        r#type: Option<String>,
        #[arg(short, long, value_name = "FORMAT", default_value = "pretty")]
        format: String,
        /// Read the XDR payload from a file instead of the positional argument.
        #[arg(short = 'i', long, value_name = "FILE")]
        file: Option<String>,
    },
    /// Encode typed values (TYPE:VALUE) to base64 XDR (reverse of decode)
    Encode {
        /// One typed value: u32, i32, u64, i64, timepoint, duration, u128, i128, bool, string, symbol, bytes, or address.
        /// Examples: u128:1000000 timepoint:1758000000 duration:86400 bytes:deadbeef
        ///
        /// Or json:<JSON> to encode ONE composite value (json:[1,2,3] is a single Vec):
        /// array -> Vec, object -> Map with String keys, null -> Void, bool -> Bool,
        /// string -> String, integer -> smallest of u32/u64 (or i32/i64 if negative).
        /// Floats are rejected. Example: json:'[{"alice":"100"},{"bob":"250"}]'

        #[arg(value_name = "TYPE:VALUE", num_args = 1..)]
        values: Vec<String>,
    },
    /// Inspect storage TTL for a contract
    Storage {
        #[command(subcommand)]
        action: StorageAction,
        /// Path to contract WASM for ABI-aware storage decoding
        #[arg(long, value_name = "WASM", global = true)]
        abi: Option<String>,
        /// Use the ABI of a deployed contract (fetched on-chain via path) for
        /// storage decoding. Mutually exclusive with `--abi`.
        #[arg(long, value_name = "CONTRACT_ID", global = true)]
        abi_contract: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Inspect a contract's ABI and storage
    Inspect {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Render the complete contract interface instead of the name-only ABI view.
        #[arg(long, default_value_t = false)]
        interface: bool,
        /// Path to contract WASM for ABI-aware storage inspection
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        /// Use the ABI of a deployed contract fetched from RPC.
        #[arg(long, value_name = "CONTRACT_ID")]
        abi_contract: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Verify a deployed contract matches a local WASM binary ()
    Verify {
        /// Stellar contract ID (C...)
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: String,
        /// Path to a local WASM file to compare against the on-chain code
        #[arg(long, value_name = "WASM")]
        wasm: Option<String>,
        /// Network to fetch the on-chain contract from (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Emit an upgrade-safety verdict comparing the live deployed contract
        /// (fetched on-chain) against the local `--wasm` candidate. Requires `--wasm`.
        #[arg(long, default_value_t = false)]
        upgrade_safety: bool,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Unified read-only contract posture report ()
    Health {
        /// Stellar contract ID (C...)
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: String,
        /// Optional local WASM to verify against the on-chain hash
        #[arg(long, value_name = "WASM")]
        wasm: Option<String>,
        /// Network label for the report (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Inspect a Soroban transaction
    Tx {
        #[command(subcommand)]
        action: TxAction,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Event explorer
    Events {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Start ledger sequence number for event search range
        #[arg(long)]
        start_ledger: Option<u32>,
        /// End ledger sequence number for event search range
        #[arg(long)]
        end_ledger: Option<u32>,
        /// Path to contract WASM for ABI-aware decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        /// Use the ABI of a deployed contract (fetched on-chain via path) for
        /// decoding. Mutually exclusive with `--abi`.
        #[arg(long, value_name = "CONTRACT_ID")]
        abi_contract: Option<String>,
        /// Only return events whose first topic (the event name) is this
        /// symbol. Case-sensitive; filtered server-side by the RPC.
        #[arg(long, value_name = "SYMBOL")]
        topic: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Inspect an account's balances and signers
    Account {
        address: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Estimate transaction fee from recent ledger base fees
    Fee {
        #[command(subcommand)]
        action: FeeAction,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Manage WASM metadata and caching
    Wasm {
        #[command(subcommand)]
        action: WasmAction,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Offline diff of two contract WASM files (ABI/function/event/type changes)
    Diff {
        /// Path to the OLD (baseline) WASM file
        #[arg(long, value_name = "WASM")]
        old_wasm: String,
        /// Path to the NEW (candidate) WASM file
        #[arg(long, value_name = "WASM")]
        new_wasm: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Emit an upgrade-safety verdict (breaking vs non-breaking changes)
        #[arg(long, default_value_t = false)]
        upgrade_safety: bool,
        /// Fail when the candidate grows more than N percent over the old
        /// artifact (e.g. `--max-growth-pct 10`). Operator-supplied policy:
        /// SDKT never assumes a network limit. Requires `--old-wasm` (always
        /// present for `diff`). Violation exits non-zero.
        #[arg(long, value_name = "N")]
        max_growth_pct: Option<f64>,
        /// Fail when the candidate artifact exceeds N bytes (e.g.
        /// `--max-size-bytes 131072`). Operator-supplied policy; not a
        /// hardcoded network limit. Violation exits non-zero.
        #[arg(long, value_name = "N")]
        max_size_bytes: Option<u64>,
    },
    /// Static security analysis of a Soroban contract source file (Gap C)
    Audit {
        /// Rust source file(s) or directory path(s) to analyze
        #[arg(required_unless_present = "list_rules", num_args = 1..)]
        paths: Vec<String>,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// List available audit rules and exit
        #[arg(long, default_value_t = false)]
        list_rules: bool,
        /// Disable a rule by id (repeatable), e.g. --disable MOVE-001
        #[arg(long, value_name = "RULE_ID", action = clap::ArgAction::Append)]
        disable: Vec<String>,
        /// Path to an external rule crate or local rule source directory to load.
        /// Repeatable. (Phase A: the rule must be compiled into the binary; this
        /// flag validates the path and runs the registered rules.)
        #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
        rules: Vec<String>,
        /// Skip loading installed plugins automatically from the plugin store
        #[arg(long, default_value_t = false)]
        no_plugins: bool,
        /// Path to a contract WASM for spec-correlated analysis.
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        /// Contract ID whose deployed WASM should provide the ABI.
        #[arg(long, value_name = "CONTRACT_ID")]
        abi_contract: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Manage Soroban identities (keys)
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Manage named network profiles (RPC endpoint + passphrase)
    Network {
        #[command(subcommand)]
        action: NetworkAction,
    },
    /// Read-only deployment verification: is the deployed contract the
    /// artifact I built?
    ///
    /// Compares a local WASM's offline SHA-256 against the deployed
    /// contract's executable, obtained by a raw ledger probe (no inspection
    /// shortcuts, no bytecode download). Verdicts: MATCH, DRIFT, UNKNOWN,
    /// NOT_FOUND. Never signs, submits, or deploys.
    DeploymentVerify {
        /// Stellar contract ID (C...)
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: String,
        /// Path to a local WASM file to compare against the on-chain code
        #[arg(long, value_name = "WASM")]
        wasm: Option<String>,
        /// Network to verify against (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Initialize a new Soroban contract project
    Init {
        /// Project name (directory)
        name: String,
        /// Generate only essential files
        #[arg(long, default_value_t = false)]
        minimal: bool,
        /// Overwrite existing directory
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Deploy a contract (Upload WASM + Instantiate)
    Deploy {
        /// Path to the WASM binary to upload and deploy. Mutually exclusive with
        /// `--wasm-hash`.
        #[arg(short, long)]
        wasm: Option<String>,
        /// Create-only: deploy from already-uploaded code identified by its
        /// 64-char hex WASM hash, skipping the upload step (resume a deploy whose
        /// upload succeeded but create failed). Mutually exclusive with `--wasm`.
        #[arg(long, value_name = "HASH")]
        wasm_hash: Option<String>,
        /// Deployment salt (40 hex chars = 20 bytes). Auto-generated if omitted.
        #[arg(short, long)]
        salt: Option<String>,
        /// Display the deterministic contract address before submitting.
        #[arg(long, default_value_t = false)]
        show_address: bool,
        /// Predict the address and WASM hash without submitting transactions.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Identity name to sign deployment transactions
        #[arg(short, long, default_value = "default")]
        identity: String,
        /// Constructor arguments: `type:value` (e.g. `u32:100`, `string:hello`,
        /// `bool:true`, `bytes:0a0b`, `address:G...`). Base64-encoded ScVal strings
        /// are also accepted as-is. Can be repeated.
        #[arg(long)]
        arg: Vec<String>,
        /// Abort deployment if the upgrade is not backwards-compatible.
        /// Requires --old-wasm (the currently deployed WASM) to be supplied.
        #[arg(long, default_value_t = false)]
        deny_breaking: bool,
        /// Path to the currently deployed (baseline) WASM, used with --deny-breaking.
        #[arg(long, value_name = "WASM")]
        old_wasm: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Compile Rust contracts into WASM artifacts
    Build {
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Invoke a contract function (read-only, no signing/submission)
    Call {
        /// Stellar contract ID (C...)
        #[arg(value_name = "CONTRACT_ID")]
        contract_id: String,
        /// Function name
        #[arg(value_name = "FUNCTION")]
        function: String,
        /// Typed arguments (e.g. u32:100, address:G..., string:hello, bool:true)
        #[arg(short, long, value_name = "TYPE:VALUE")]
        args: Vec<String>,
        /// Composite arguments as a JSON array; appended after --args values.
        #[arg(long, value_name = "JSON")]
        args_json: Vec<String>,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Path to contract WASM for ABI-aware result decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        /// Use the ABI of a deployed contract (fetched on-chain via RPC) for
        /// decoding the result. Mutually exclusive with `--abi`.
        #[arg(long, value_name = "CONTRACT_ID")]
        abi_contract: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Invoke a contract function with a signed, submitted transaction
    /// (state-changing end-to-end: sequence → simulate → sign → submit → poll)
    Invoke {
        /// Stellar contract ID (C...)
        #[arg(value_name = "CONTRACT_ID")]
        contract_id: String,
        /// Function name
        #[arg(value_name = "FUNCTION")]
        function: String,
        /// Typed arguments (e.g. u32:100, address:G..., string:hello, bool:true)
        #[arg(short, long, value_name = "TYPE:VALUE")]
        args: Vec<String>,
        /// Composite arguments as a JSON array; appended after --args values.
        #[arg(long, value_name = "JSON")]
        args_json: Vec<String>,
        /// Identity name whose account signs and pays for the invocation
        #[arg(short = 'I', long, default_value = "default")]
        identity: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Return after submission with the transaction hash instead of polling for settlement
        #[arg(long)]
        no_wait: bool,
        /// Build and sign the invocation envelope, print it, and stop without submitting (#73)
        #[arg(long)]
        build_only: bool,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Generate or inspect the project lock file (`sdkt.lock`)
    Lock {
        #[command(subcommand)]
        action: LockCommand,
    },
    /// Validate and inspect local package manifests ()
    Package {
        #[command(subcommand)]
        action: PackageCommand,
    },
    /// Manage multi-contract projects
    Project {
        #[command(subcommand)]
        action: ProjectCommand,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Manage local audit plugins (install/remove/list/show/update) —
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Read-only release-assurance: artifact → security → upgrade safety →
    /// deployed verification → contract health, aggregated into one release
    /// status. Never signs, submits, extends TTL, or mutates any state.
    ReleaseAssurance {
        /// Candidate WASM artifact to assess (hash, spec, upgrade diff, audit)
        #[arg(long, value_name = "WASM")]
        wasm: String,
        /// Previous (baseline) WASM for the offline upgrade-safety diff
        #[arg(long, value_name = "WASM")]
        previous_wasm: Option<String>,
        /// Rust source file(s) or directory for the static security audit
        /// (same engine and semantics as `sdkt audit`)
        #[arg(long, value_name = "PATH", num_args = 1..)]
        audit: Vec<String>,
        /// Disable an audit rule by id (repeatable), e.g. --disable AUTH-001
        #[arg(long, value_name = "RULE_ID", action = clap::ArgAction::Append)]
        disable: Vec<String>,
        /// Deployed contract ID (C...) for on-chain verification + health.
        /// On-chain checks are skipped entirely when omitted.
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: Option<String>,
        /// Deployed network (testnet | mainnet | futurenet) for the on-chain
        /// checks. Only required together with --contract.
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Attach the JSON evidence file written by a completed `sdkt fuzz
        /// campaign --artifact-dir` run (release-assurance records the
        /// evidence; it never runs a fuzz campaign itself). Only
        /// Classification::Finding entries count as security evidence.
        #[arg(long, value_name = "EVIDENCE_JSON")]
        fuzz_evidence: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Fail (artifact section -> release FAIL, exit 1) when the candidate
        /// artifact exceeds N bytes (e.g. `--max-size-bytes 131072`).
        /// Operator-supplied policy; SDKT never hardcodes a network limit.
        /// Violates when size > N; a size of exactly N passes.
        #[arg(long, value_name = "N")]
        max_size_bytes: Option<u64>,
        /// Fail (artifact section -> release FAIL, exit 1) when the candidate
        /// grew more than N percent over `--previous-wasm`. Requires
        /// `--previous-wasm`; a request without it is an error, never a silent
        /// skip. Violates when growth > N; exactly N passes. Growth is undefined
        /// against a zero-byte baseline and is reported as such.
        #[arg(long, value_name = "N")]
        max_growth_pct: Option<f64>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Generate shell completion scripts for your shell
    Completions {
        /// Shell to generate completions for (bash, zsh, fish, powershell, elvish)
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Run diagnostic checks on the sdkt environment and project
    Doctor {
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Shorthand for --format json (machine-readable output)
        #[arg(long, conflicts_with = "format")]
        json: bool,
    },
    /// Soroban-aware deterministic fuzzing for release assurance (offline,
    /// read-only: executes contracts in an isolated in-memory host; never
    /// signs, submits, or touches the network). Findings are rule-driven:
    /// only explicitly declared expectations can produce them, and a finding
    /// is evidence, not a vulnerability claim.
    #[command(subcommand)]
    Fuzz(FuzzAction),
    /// Generate typed clients and other artifacts from contract interfaces
    #[command(subcommand)]
    Generate(GenerateAction),
}

#[derive(Subcommand)]
enum FuzzAction {
    /// Run a deterministic fuzz campaign against a compiled contract
    Campaign {
        /// Path to the compiled contract WASM file
        #[arg(value_name = "WASM")]
        wasm: String,
        /// Deterministic seed (u64)
        #[arg(long, value_name = "N", default_value_t = 0)]
        seed: u64,
        /// Number of cases to execute
        #[arg(long, value_name = "N", default_value_t = 100)]
        cases: usize,
        /// Restrict the campaign to one exported function
        #[arg(long, value_name = "NAME")]
        function: Option<String>,
        /// Directory for finding artifacts (default: ./sdkt-fuzz-artifacts)
        #[arg(long, value_name = "DIR")]
        artifact_dir: Option<String>,
        /// Authorization mode for every case: no-auth | correct-auth |
        /// wrong-auth (see the sdkt-fuzz docs for exact semantics)
        #[arg(long, value_name = "MODE", default_value = "no-auth")]
        auth: String,
        /// Declare that a function must succeed: --expect-success NAME
        /// (repeatable). Violations become RETURN_MISMATCH or
        /// UNEXPECTED_ERROR findings.
        #[arg(long, value_name = "FN", action = clap::ArgAction::Append)]
        expect_success: Vec<String>,
        /// Declare that a function must fail: --expect-error FN:TYPE:CODE
        /// (repeatable), e.g. --expect-error bump:Auth:6
        #[arg(long, value_name = "FN:TYPE:CODE", action = clap::ArgAction::Append)]
        expect_error: Vec<String>,
        /// Replay every written artifact to verify determinism
        #[arg(long)]
        replay: bool,
        /// Output format (pretty | json | jsonl)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Replay a finding artifact against its WASM and verify it reproduces
    Replay {
        /// Path to the finding artifact (JSON)
        #[arg(value_name = "ARTIFACT")]
        artifact: String,
        /// Path to the campaign WASM the artifact was produced from
        #[arg(long, value_name = "WASM")]
        wasm: String,
    },
}

#[derive(Subcommand)]
enum GenerateAction {
    /// Generate a typed Rust client from a compiled contract's ContractSpec
    Client {
        /// Path to the compiled contract WASM file
        #[arg(value_name = "WASM")]
        wasm: String,
        /// Output file path (prints to stdout if omitted)
        #[arg(short, long, value_name = "PATH")]
        output: Option<String>,
        /// Skip functions with unsupported types instead of aborting
        #[arg(long)]
        skip_unsupported: bool,
    },
}

#[derive(Subcommand)]
enum IdentityAction {
    /// Generate a new ED25519 identity
    Generate {
        name: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    Import {
        name: String,
        /// Secret key, or `-` to read it from stdin (keeps the secret out of
        /// the process argv / `ps` output — the path CI smoke jobs need).
        secret: String,
    },
    /// List all saved identities
    List {
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Show a single identity by name
    Show {
        name: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    Delete {
        name: String,
    },
    Default {
        name: String,
    },
    /// Fund an identity via the Stellar Testnet Friendbot.
    Fund {
        name: String,
        /// Network profile name (must have a Friendbot URL configured).
        #[arg(long)]
        network_profile: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// Scaffold a new sdkt-audit plugin rule crate
    Init {
        /// Plugin rule project name (directory). The rule id is derived from
        /// this name (e.g. my-rule → MY-RULE-001).
        name: String,
        /// Overwrite existing directory
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// List installed plugins
    List {
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Show metadata for an installed plugin
    Show {
        id: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Install a plugin from a local artifact + sibling plugin.toml, or from a
    /// `.sdktplugin` bundle (verified before anything is installed)
    Install {
        /// Path to the local plugin artifact (.so/.dylib/.dll/.wasm) or `.sdktplugin` bundle
        source: String,
        /// Override the plugin id from metadata (rarely needed)
        #[arg(long)]
        id: Option<String>,
        /// Overwrite an existing install of the same id
        #[arg(long)]
        force: bool,
        /// Require the bundle to be signed by this Ed25519 public key file (32 bytes, raw);
        /// unsigned bundles are refused
        #[arg(long)]
        public_key: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Remove an installed plugin by id (idempotent)
    Remove {
        id: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Update an installed plugin from a new local artifact (local-only)
    Update {
        id: String,
        /// Path to the new local artifact
        source: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Pack a plugin directory into a `.sdktplugin` bundle
    Pack {
        /// Path to the plugin directory (must contain plugin.toml + artifact)
        source: String,
        /// Output bundle path (defaults to <id>-<version>.sdktplugin in current dir)
        #[arg(long)]
        output: Option<String>,
        /// Optional Ed25519 secret key file for signing (32 bytes, raw)
        #[arg(long)]
        secret_key: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Verify a `.sdktplugin` bundle's integrity and signature
    VerifyBundle {
        /// Path to the `.sdktplugin` bundle
        bundle: String,
        /// Optional Ed25519 public key file for signature verification (32 bytes, raw)
        #[arg(long)]
        public_key: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Run end-to-end diagnostics and self-check on an installed plugin, directory, or bundle
    Doctor {
        /// Target plugin id, directory, or .sdktplugin bundle
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        target: Option<String>,
        /// Run doctor across all installed plugins in the store
        #[arg(long)]
        all: bool,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum WasmAction {
    /// Inspect a local WASM contract file offline
    Inspect {
        /// Path to the WASM file to inspect
        file: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Inspect WASM metadata for a deployed contract
    Metadata {
        #[arg(short, long)]
        contract: String,
        /// Network to fetch the on-chain contract from (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Force bypass the cache and fetch fresh from RPC
        #[arg(long, default_value_t = false)]
        refresh: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Manage the local WASM cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Show stats about the cache (size, item count)
    Info {
        #[arg(short, long, default_value = "testnet")]
        network: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Remove a specific hash from the cache
    Remove {
        hash: String,
        #[arg(short, long, default_value = "testnet")]
        network: String,
    },
    /// Clear all items in the cache for the network
    Clear {
        #[arg(short, long, default_value = "testnet")]
        network: String,
    },
}

#[derive(Subcommand)]
enum FeeAction {
    Estimate {
        /// Network: testnet, mainnet, standalone
        #[arg(short, long, default_value = "testnet")]
        network: String,
        /// Comma-separated recent base fees in stroops (e.g. "100,120,110"). Optional if --rpc is used.
        #[arg(short, long, value_name = "FEES")]
        base_fees: Option<String>,
        /// Fetch fee statistics directly from Soroban RPC instead of manual base fees
        #[arg(long, default_value_t = false)]
        rpc: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum TxAction {
    Inspect {
        hash: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Validate a transaction envelope offline (pre-flight checks)
    Validate {
        /// Base64 XDR transaction envelope or path to a file containing it
        #[arg(short, long)]
        envelope: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Show a transaction envelope in human-readable form (offline)
    ///
    /// Renders source, sequence, fee, memo, each operation (contract calls
    /// with decoded arguments), the Soroban footprint and attached signatures,
    /// so an envelope can be reviewed before `tx sign` or `tx submit`.
    Decode {
        /// Base64 XDR transaction envelope or path to a file containing it
        envelope: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Simulate a transaction envelope without submitting it
    Simulate {
        /// Base64 XDR transaction envelope or path to a file containing it
        #[arg(short, long)]
        envelope: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Path to contract WASM for ABI-aware result decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        /// Use the ABI of a deployed contract (fetched on-chain via path) for
        /// result decoding. Mutually exclusive with `--abi`.
        #[arg(long, value_name = "CONTRACT_ID")]
        abi_contract: Option<String>,
    },
    /// Submit a transaction envelope to the network, optionally waiting
    Submit {
        /// Base64 XDR transaction envelope or path to a file containing it
        #[arg(short, long)]
        envelope: String,
        /// Wait and poll until the transaction settles
        #[arg(short, long)]
        wait: bool,
        /// Timeout in seconds while waiting
        #[arg(short = 't', long, default_value = "60")]
        timeout: u64,
        /// Polling interval in seconds while waiting
        #[arg(short, long, default_value = "2")]
        interval: u64,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Build a Soroban transaction envelope XDR
    ///
    /// Offline by default: the fee is the base inclusion fee only. Pass the
    /// usual network flags (`--network-profile` / `--rpc-url`) to simulate the
    /// invocation and adopt the resource fee and footprint the network reports.
    /// An explicit `--fee` always wins over both.
    Build {
        #[arg(long)]
        source: String,
        /// Account sequence number to build against. When omitted, it is
        /// resolved automatically from the network for `--source` (requires the
        /// network/RPC flags to be resolvable, same as other RPC commands).
        #[arg(long)]
        sequence: Option<i64>,
        /// Total fee in stroops. Overrides the network-derived fee when the
        /// network flags are also given. Defaults to 100 (inclusion only).
        #[arg(long)]
        fee: Option<u32>,
        #[arg(long)]
        contract: String,
        #[arg(long)]
        function: String,
        /// Optional arguments: `type:value` (e.g. `u32:100`, `string:hello`,
        /// `bool:true`, `bytes:0a0b`). Base64-encoded ScVal strings are also
        /// accepted as-is (passthrough).
        #[arg(long)]
        arg: Vec<String>,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Optional file path to write the output envelope XDR
        #[arg(short, long)]
        output: Option<String>,
        /// Attach a text memo (up to 28 bytes) to the built transaction.
        /// Mutually exclusive with `--memo-id`.
        #[arg(long, value_name = "TEXT")]
        memo_text: Option<String>,
        /// Attach a numeric ID memo to the built transaction.
        /// Mutually exclusive with `--memo-text`.
        #[arg(long, value_name = "ID")]
        memo_id: Option<u64>,
    },
    /// Sign a transaction envelope using a local identity ( / PR2)
    Sign {
        /// Input: base64 XDR envelope, or a path to a file containing it
        #[arg(short, long, value_name = "INPUT")]
        input: String,
        /// Output file to write the signed base64 envelope. Prints to stdout if omitted.
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<String>,
        /// Identity name to sign with. Defaults to "default".
        #[arg(short = 'I', long, default_value = "default")]
        identity: String,
        /// Network: testnet | mainnet | futurenet | custom:<passphrase>
        #[arg(short, long, default_value = "testnet")]
        network: String,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Show deployments recorded for the selected network profile.
    Status {
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Deploy all contracts defined in the workspace. Every deployed contract
    /// is persisted to `.sdkt-deployments.json` (per network profile) so a
    /// failure mid-graph never loses the contracts that already landed.
    Deploy {
        /// Deployment salt (40 hex chars = 20 bytes). Auto-generated if omitted.
        #[arg(short, long)]
        salt: Option<String>,
        /// Skip aliases whose recorded contract ID still exists on-chain
        /// (verified via getLedgerEntries against the `.sdkt-deployments.json`
        /// record for this network profile). Resume an interrupted deploy
        /// without re-deploying (and re-paying for) what already succeeded.
        #[arg(long)]
        skip_deployed: bool,
        /// Identity name to sign and pay for each contract deployment. Defaults to "default".
        #[arg(short = 'I', long, default_value = "default")]
        identity: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum LockCommand {
    /// Generate `sdkt.lock` from the current build artifacts (next to
    /// `.sdkt.toml`). Requires `sdkt build` to have run first.
    Generate {
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Verify the lock file against the current on-disk artifacts.
    /// Advisory: reports drift but never fails the build.
    Verify {
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Print the contents of `sdkt.lock` if present.
    Show {
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

#[derive(Subcommand)]
enum PackageCommand {
    /// Validate the local package manifest (metadata + dependency graph).
    /// Offline: never performs network or registry operations.
    Validate {
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Fetch declared dependencies into the local cache.
    /// Git deps are cloned/checked out; local `path` deps are passed through.
    /// Never builds automatically. Use `--force` to update existing checkouts.
    Fetch {
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Re-fetch / update existing checkouts instead of reusing them.
        #[arg(long)]
        force: bool,
    },
    /// Synchronize dependencies with what is available upstream and refresh the
    /// lock. `rev` deps stay pinned; `tag`/`branch` deps update when the remote
    /// commit changed. Use `--check` to report only, `--dry-run` to preview
    /// changes without touching the cache or lock.
    Update {
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Only report available updates; do not fetch or rewrite the lock.
        #[arg(long)]
        check: bool,
        /// Compute and preview changes; do not modify the cache or lock.
        #[arg(long)]
        dry_run: bool,
    },
    /// Bundle the resolved project into a portable offline artifact: the
    /// manifest, the lockfile, and the cached git dependency checkouts. The
    /// artifact can be unpacked on another machine and rebuilt without network.
    Pack {
        /// Output directory for the artifact (default: `./dist`).
        #[arg(short, long, default_value = "dist")]
        out: String,
        /// Artifact format: `tar.zst` (compressed tarball) or `dir` (directory tree).
        #[arg(long, default_value = "tar.zst")]
        format: String,
    },
    /// Validate publish readiness (read-only): manifest valid, lock consistent,
    /// all cached commits present, integrity hashes match. Default is
    /// `--dry-run`; nothing is published. `--broadcast` is opt-in and only acts
    /// when a registry source is configured (none in , so it stays offline).
    Publish {
        /// Perform a read-only readiness check (default: true). No publish.
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        /// Actually publish. Opt-in only; requires a configured registry source.
        #[arg(long)]
        broadcast: bool,
    },
}

#[derive(Subcommand)]
enum StorageAction {
    Check {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Estimate storage rent cost for a WASM contract offline based on its ContractSpec.
    ///
    /// Computes an offline baseline storage cost breakdown per storage class
    /// (Instance, Persistent, Temporary) and total cost in stroops and XLM, using the
    /// rent approximation (100 stroops/ledger/entry).
    ///
    /// Note: This is an offline baseline derived from the contract's declared ABI.
    /// Dynamic runtime entries (e.g. user balances created during contract execution)
    /// cannot be predicted offline. For live on-chain storage inspection, use
    /// `sdkt storage analyze`.
    Estimate {
        /// Path to the compiled contract WASM file.
        wasm: String,
        /// Number of ledgers to estimate extension cost for (default: 17280, ~1 day at 5s/ledger).
        #[arg(short, long, default_value = "17280")]
        ledgers: u32,
        /// Output format (pretty or json).
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Analyze a contract's storage layout (Instance/Persistent/Temporary
    /// categorization, TTL summary, and per-entry detail).
    #[command(alias = "snapshot")]
    Analyze {
        contract_id: String,
        /// Repeatable: extra ledger keys (base64 XDR or hex XDR) to include in
        /// the analysis. The contract instance key is always included.
        #[arg(long, value_name = "BASE64_XDR", alias = "key")]
        key_xdr: Vec<String>,
        /// Leading symbol of a typed data key — the map/enum-variant name.
        /// Combined with `--key-arg` this builds `ScVec[symbol, args...]`,
        /// e.g. `--map-key balances --key-arg address:G...`.
        #[arg(long, value_name = "SYMBOL")]
        map_key: Option<String>,
        /// Repeatable typed key component (`TYPE:VALUE`, e.g. `address:G...`,
        /// `u32:100`) appended after `--map-key`. Requires `--map-key`.
        #[arg(long, value_name = "TYPE:VALUE")]
        key_arg: Vec<String>,
        /// Include the contract's instance-storage entry (always included by default).
        #[arg(long)]
        instance: bool,
        /// Durability of a typed data key: `persistent` (default) or `temporary`.
        #[arg(
            long,
            value_name = "persistent|temporary",
            default_value = "persistent"
        )]
        durability: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Extend the TTL of a contract's footprint via `ExtendFootprintTtl`.
    Extend {
        /// Contract whose storage footprint should have TTL extended.
        #[arg(long)]
        contract: String,
        /// Minimum TTL in ledgers: entries will live at least this many ledgers past the current ledger.
        #[arg(short, long)]
        ledgers: u32,
        /// Repeatable: extra ledger keys (base64 XDR or hex XDR) to include in
        /// the footprint. The contract instance key is always included.
        #[arg(long, value_name = "KEY")]
        key: Vec<String>,
        /// Identity name to sign the extend transaction. Defaults to "default".
        #[arg(short = 'I', long, default_value = "default")]
        identity: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Restore archived contract storage via `RestoreFootprint`.
    ///
    /// Simulates the invocation that hit archived state, adopts the
    /// `restorePreamble` footprint and minimum resource fee, then signs and
    /// submits a `RestoreFootprint` transaction.
    Restore {
        /// Contract whose archived storage should be restored.
        #[arg(long)]
        contract: String,
        /// Transaction envelope (base64 XDR) of the invocation that failed due to archived state.
        #[arg(long)]
        envelope: String,
        /// Identity name to sign the restore transaction. Defaults to "default".
        #[arg(short = 'I', long, default_value = "default")]
        identity: String,
        /// Dry-run: show what would be restored without submitting.
        #[arg(long)]
        dry_run: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Read a contract's storage entry.
    ///
    /// The entry can be identified either by a complete raw `LedgerKey`
    /// (`--key-xdr`, base64 or hex) or by a typed key specification built from
    /// the contract's own types (`--map-key`/`--key-arg`, or `--instance`).
    Read {
        /// Contract ID whose storage entry should be read (C... StrKey).
        #[arg(long)]
        contract: String,
        /// Complete base64/hex-encoded `LedgerKey` (escape hatch for advanced
        /// use). Mutually exclusive with the typed key options.
        #[arg(long, value_name = "BASE64_XDR")]
        key_xdr: Option<String>,
        /// Leading symbol of a typed data key — the map/enum-variant name.
        /// Combined with `--key-arg` this builds `ScVec[symbol, args...]`,
        /// e.g. `--map-key balances --key-arg address:G...`.
        #[arg(long, value_name = "SYMBOL")]
        map_key: Option<String>,
        /// Repeatable typed key component (`TYPE:VALUE`, e.g. `address:G...`,
        /// `u32:100`) appended after `--map-key`. Requires `--map-key`.
        #[arg(long, value_name = "TYPE:VALUE")]
        key_arg: Vec<String>,
        /// Read the contract's instance-storage entry (always persistent).
        #[arg(long)]
        instance: bool,
        /// Durability of a typed data key: `persistent` (default) or `temporary`.
        #[arg(
            long,
            value_name = "persistent|temporary",
            default_value = "persistent"
        )]
        durability: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Diff two storage snapshots and, optionally, derive a TTL extension plan.
    ///
    /// `--old` and `--new` each point to a JSON file that is the output of
    /// `sdkt storage analyze --format json` (a serialised `StorageReport`).
    ///
    /// Without `--extend-plan` the command prints the diff entries only.
    ///
    /// With `--extend-plan` the command additionally prints the remediation
    /// plan: the contract ID, the ledger keys covering the removed/expiring
    /// entries, and a suggested `--ledgers` value.  Nothing is signed or
    /// submitted.
    ///
    /// Exit codes: 0 in all non-error cases (including an empty plan).
    #[command(name = "storage-diff", alias = "diff")]
    StorageDiff {
        /// Path to the OLD (baseline) storage snapshot JSON file.
        #[arg(long, value_name = "FILE")]
        old: String,
        /// Path to the NEW (current) storage snapshot JSON file.
        #[arg(long, value_name = "FILE")]
        new: String,
        /// Derive and print a TTL extension plan from the diff.
        /// Prints the contract, ledger keys, and suggested --ledgers value.
        /// No transaction is built, signed, or submitted.
        #[arg(long, default_value_t = false)]
        extend_plan: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

/// — Contract verification report.
///
/// Serializes directly to the JSON schema defined in _PLAN.md §10.
/// `local_wasm_hash` / `local_wasm_size_bytes` / `match` are `Option` so they
/// serialize as `null` when no local WASM is supplied (OnChainOnly mode).
#[derive(Debug, serde::Serialize)]
struct VerificationReport {
    contract_id: String,
    network: String,
    on_chain_wasm_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_wasm_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_wasm_size_bytes: Option<usize>,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    matches: Option<bool>,
    verification_status: String,
    explanation: String,
}

/// — Contract health / posture report.
///
/// Aggregates the existing read-only surfaces (`inspect_contract` +
/// `StorageAnalyzer`) plus optional verification into one report.
/// Serializes to the JSON schema in _PLAN.md §12.
#[derive(Debug, serde::Serialize)]
struct ContractHealthReport {
    contract_id: String,
    network: String,
    /// "healthy" | "at_risk" | "critical" (snake_case for stable parsing)
    health: String,
    /// bool when --wasm supplied, null otherwise
    #[serde(rename = "verified", skip_serializing_if = "Option::is_none")]
    verified: Option<bool>,
    on_chain_wasm_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_wasm_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_wasm_size_bytes: Option<usize>,
    storage: HealthStorage,
    /// Human-readable verdict reasons (empty when healthy)
    #[serde(default)]
    reasons: Vec<String>,
}

/// Storage subsection of [`ContractHealthReport`] (mirrors `StorageReport`
/// field names; omits `total_size_bytes`/`entries` per _PLAN.md §12).
#[derive(Debug, serde::Serialize)]
struct HealthStorage {
    total_entries: usize,
    instance_entries: usize,
    persistent_entries: usize,
    temporary_entries: usize,
    other_entries: usize,
    ttl: Option<HealthTtl>,
}

#[derive(Debug, serde::Serialize)]
struct HealthTtl {
    minimum_ttl: u32,
    maximum_ttl: u32,
    average_ttl: u32,
    expiring_entries_count: usize,
    estimated_rent_cost: Option<u64>,
}

/// — Pure health-verdict derivation (no I/O, fully testable).
///
/// Rules (transparent, per _PLAN.md §10):
/// - `verified == Some(false)` → Critical (deployed != built).
/// - else `expiring_soon > 0` → AtRisk (entries near TTL expiry).
/// - else `total_entries == 0` → AtRisk (empty contract).
/// - else → Healthy.
fn derive_verdict(
    verified: Option<bool>,
    expiring_soon: usize,
    total_entries: usize,
) -> (String, Vec<String>) {
    let mut reasons: Vec<String> = Vec::new();

    if verified == Some(false) {
        reasons.push(
            "On-chain WASM does NOT match the supplied local file. Rebuild and redeploy, \
or confirm you are comparing the correct artifact."
                .to_string(),
        );
        return ("critical".to_string(), reasons);
    }

    if expiring_soon > 0 {
        reasons.push(format!(
            "{} storage entr{} expiring soon (within ~{} day{}).",
            expiring_soon,
            if expiring_soon == 1 {
                "y is"
            } else {
                "ies are"
            },
            (u64::from(EXPIRING_SOON_LEDGERS) * 5).div_ceil(86_400),
            if (u64::from(EXPIRING_SOON_LEDGERS) * 5).div_ceil(86_400) == 1 {
                ""
            } else {
                "s"
            }
        ));
    }

    if total_entries == 0 {
        reasons.push("Contract has no storage entries (unusual for a live contract).".to_string());
    }

    if !reasons.is_empty() {
        return ("at_risk".to_string(), reasons);
    }

    ("healthy".to_string(), reasons)
}

/// Orchestrates the contract health report (read-only).
///
/// - Offline-hashes `--wasm` first (fail-fast) when supplied.
/// - Fetches on-chain WASM hash via `sdkt-rpc::inspect_contract` (no bytecode download).
/// - Fetches storage posture via `sdkt-storage::StorageAnalyzer` (no new RPC).
/// - Reuses `verification_outcome` for the local-vs-onchain comparison.
async fn contract_health(
    client: &SorobanRpcClient,
    contract_id: &str,
    local_wasm: Option<&[u8]>,
    network: &str,
) -> Result<ContractHealthReport, String> {
    // Optional local WASM — hashed fully offline FIRST (fail fast).
    let local_hash = match local_wasm {
        Some(bytes) => {
            let meta = sdkt_wasm::parse_metadata(bytes).map_err(|e| format!("{}", e))?;
            Some((meta.hash, meta.size_bytes))
        }
        None => None,
    };

    // On-chain WASM hash only (read-only, existing RPC).
    let inspection = inspect_contract(client, contract_id)
        .await
        .map_err(|e| match e {
            sdkt_rpc::RpcError::ContractNotFound => {
                format!("contract {} not found on {}", contract_id, network)
            }
            other => format!("{}", other),
        })?;
    let on_chain_hash = inspection.wasm_hash;

    // Storage posture (read-only, existing RPC via StorageAnalyzer).
    let storage_report = sdkt_storage::StorageAnalyzer::new(client.clone())
        .inspect_contract_storage(contract_id)
        .await
        .map_err(|e| format!("{}", e))?;

    // Optional -style verification (reuse existing helper, no duplicate logic).
    let verified = local_hash
        .as_ref()
        .and_then(|(h, s)| verification_outcome(&on_chain_hash, Some((h.clone(), *s))).0);

    let expiring_soon = storage_report
        .ttl_summary
        .as_ref()
        .map(|t| t.expiring_entries_count)
        .unwrap_or(0);

    let (health, reasons) = derive_verdict(verified, expiring_soon, storage_report.total_entries);

    let ttl = storage_report.ttl_summary.as_ref().map(|t| HealthTtl {
        minimum_ttl: t.minimum_ttl,
        maximum_ttl: t.maximum_ttl,
        average_ttl: t.average_ttl,
        expiring_entries_count: t.expiring_entries_count,
        estimated_rent_cost: t.estimated_rent_cost,
    });

    Ok(ContractHealthReport {
        contract_id: contract_id.to_string(),
        network: network.to_string(),
        health,
        verified,
        on_chain_wasm_hash: on_chain_hash,
        local_wasm_hash: local_hash.as_ref().map(|(h, _)| h.clone()),
        local_wasm_size_bytes: local_hash.as_ref().map(|(_, s)| *s),
        storage: HealthStorage {
            total_entries: storage_report.total_entries,
            instance_entries: storage_report.instance_entries,
            persistent_entries: storage_report.persistent_entries,
            temporary_entries: storage_report.temporary_entries,
            other_entries: storage_report.other_entries,
            ttl,
        },
        reasons,
    })
}

/// Pure comparison logic for (no I/O, fully testable).
///
/// Given the on-chain hash and an optional local `(hash, size)` pair, returns
/// the `(match, status, explanation)` triple per _PLAN.md §10/§11.
fn verification_outcome(
    on_chain_hash: &str,
    local: Option<(String, usize)>,
) -> (Option<bool>, String, String) {
    match local {
        Some((ref lh, _size)) => {
            if *lh == on_chain_hash {
                (Some(true), "Verified".to_string(), String::new())
            } else {
                (
                    Some(false),
                    "Mismatch".to_string(),
                    format!(
                        "The deployed bytecode does NOT match the local file.\nOn-chain : {}\nLocal    : {}\nRebuild and redeploy, or confirm you are comparing the correct artifact.",
                        on_chain_hash, lh
                    ),
                )
            }
        }
        None => (
            None,
            "OnChainOnly".to_string(),
            "No local WASM provided; reporting on-chain hash only.".to_string(),
        ),
    }
}

/// Orchestrates contract verification ().
///
/// Fetches the on-chain WASM hash via `sdkt-rpc::inspect_contract` (no bytecode
/// download) and compares it against the offline local WASM hash from
/// `sdkt-wasm::parse_metadata`. `local_wasm` is optional: when `None`, the
/// report is `OnChainOnly` (no comparison verdict).
async fn verify_contract(
    client: &SorobanRpcClient,
    contract_id: &str,
    local_wasm: Option<&[u8]>,
    network: &str,
) -> Result<VerificationReport, String> {
    // Hash the local WASM fully offline FIRST (fail fast on bad/missing files
    // before touching the network), per _PLAN.md "Offline hashing".
    let local_hash = match local_wasm {
        Some(bytes) => {
            let meta = sdkt_wasm::parse_metadata(bytes).map_err(|e| format!("{}", e))?;
            Some((meta.hash, meta.size_bytes))
        }
        None => None,
    };

    // On-chain hash only — never download the bytecode.
    let inspection = inspect_contract(client, contract_id)
        .await
        .map_err(|e| format!("{}", e))?;

    let on_chain_hash = inspection.wasm_hash;

    // Capture report fields from the (still-owned) local hash before the
    // comparison consumes it.
    let local_wasm_hash = local_hash.as_ref().map(|(h, _)| h.clone());
    let local_wasm_size_bytes = local_hash.as_ref().map(|(_, s)| *s);

    let (matches, status, explanation) = verification_outcome(&on_chain_hash, local_hash);

    Ok(VerificationReport {
        contract_id: contract_id.to_string(),
        network: network.to_string(),
        on_chain_wasm_hash: on_chain_hash,
        local_wasm_hash,
        local_wasm_size_bytes,
        matches,
        verification_status: status,
        explanation,
    })
}

// === release-assurance foundation: shared types + pure classifiers ===
// (No new engine — reuses sdkt-wasm, sdkt-audit, verify_contract, contract_health.)

/// Status of a single release-assurance check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum RaStatus {
    Pass,
    Review,
    Fail,
    Skipped,
    Error,
}

impl std::fmt::Display for RaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RaStatus::Pass => "PASS",
            RaStatus::Review => "REVIEW",
            RaStatus::Fail => "FAIL",
            RaStatus::Skipped => "SKIPPED",
            RaStatus::Error => "ERROR",
        };
        f.write_str(s)
    }
}

/// A single check section: status + machine-readable detail.
#[derive(Debug, serde::Serialize)]
struct RaCheck {
    status: RaStatus,
    detail: String,
}

/// Result of an on-chain check: the existing engine's report, or a classified
/// status paired with the engine's original error message.
type OnChainResult<T> = Result<T, (RaStatus, String)>;

/// Artifact section.
#[derive(Debug, serde::Serialize)]
struct ArtifactResult {
    #[serde(flatten)]
    check: RaCheck,
    /// The candidate WASM metadata (hash/size/spec), when parsed successfully.
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<sdkt_wasm::WasmMetadata>,
}

/// Security section, carrying the full audit report when run.
#[derive(Debug, serde::Serialize)]
struct SecurityResult {
    #[serde(flatten)]
    check: RaCheck,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit: Option<sdkt_audit::AuditReport>,
}

/// Upgrade-safety section.
#[derive(Debug, serde::Serialize)]
struct UpgradeResult {
    #[serde(flatten)]
    check: RaCheck,
    /// Where the verdict came from: `previous WASM`, `live contract <id>`, etc.
    baseline: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<sdkt_wasm::UpgradeVerdict>,
}

/// Deployed verification section (reuses `verify_contract`'s report).
#[derive(Debug, serde::Serialize)]
struct VerifyResult {
    #[serde(flatten)]
    check: RaCheck,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<VerificationReport>,
}

/// Contract health section (reuses `contract_health`'s report).
#[derive(Debug, serde::Serialize)]
struct HealthResult {
    #[serde(flatten)]
    check: RaCheck,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<ContractHealthReport>,
}

/// Aggregate, machine-readable release-assurance result.
#[derive(Debug, serde::Serialize)]
struct ReleaseAssuranceReport {
    contract: Option<String>,
    network: String,
    artifact: ArtifactResult,
    security: SecurityResult,
    upgrade_safety: UpgradeResult,
    verification: VerifyResult,
    health: HealthResult,
    /// Fuzz evidence attached from a completed campaign
    /// (`--fuzz-evidence`); absent (and JSON-skipped) unless supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fuzz: Option<FuzzEvidenceResult>,
    release_status: String,
    reasons: Vec<String>,
}

/// Release-assurance section for a recorded fuzz campaign evidence file.
#[derive(Debug, serde::Serialize)]
struct FuzzEvidenceResult {
    #[serde(flatten)]
    check: RaCheck,
    evidence: sdkt_fuzz::CampaignEvidence,
}

/// Deterministic status from an audit report:
/// critical findings → FAIL, warnings → REVIEW, otherwise PASS.
fn audit_status(summary: &sdkt_audit::AuditSummary) -> RaStatus {
    if summary.critical > 0 {
        RaStatus::Fail
    } else if summary.warning > 0 {
        RaStatus::Review
    } else {
        RaStatus::Pass
    }
}

/// Deterministic status from an upgrade verdict.
fn upgrade_verdict_status(verdict: &sdkt_wasm::UpgradeVerdict) -> RaStatus {
    if verdict.compatible {
        RaStatus::Pass
    } else {
        RaStatus::Fail
    }
}

/// Deterministic status from a verification report's match result.
fn verification_status(matched: Option<bool>) -> RaStatus {
    match matched {
        Some(true) => RaStatus::Pass,
        Some(false) => RaStatus::Fail,
        None => RaStatus::Review,
    }
}

/// Deterministic status from a contract-health label.
fn health_status(health: &str) -> RaStatus {
    match health {
        "healthy" => RaStatus::Pass,
        "at_risk" => RaStatus::Review,
        "critical" => RaStatus::Fail,
        _ => RaStatus::Error,
    }
}

/// Aggregate the final release status from individual check statuses.
/// FAIL/Error dominate; any REVIEW/SKIPPED (with no hard failure) → REVIEW;
/// otherwise PASS.
fn aggregate_release_status(statuses: &[RaStatus]) -> String {
    if statuses
        .iter()
        .any(|s| *s == RaStatus::Fail || *s == RaStatus::Error)
    {
        return "FAIL".to_string();
    }
    if statuses
        .iter()
        .any(|s| *s == RaStatus::Review || *s == RaStatus::Skipped)
    {
        return "REVIEW".to_string();
    }
    "PASS".to_string()
}

/// Map an on-chain check error string onto a status: a missing contract is a
/// hard FAIL (blocking), any transport/engine error is an ERROR.
fn contract_not_found_status(e: &str) -> RaStatus {
    if e.contains("Contract not found") || e.contains("not found on") {
        RaStatus::Fail
    } else {
        RaStatus::Error
    }
}

/// Validates an operator-supplied byte threshold.
///
/// Thresholds are policy inputs, not network constants: SDKT ships no default
/// and hardcodes no Stellar limit. `0` would reject every artifact, so it is
/// rejected up front.
fn validate_size_threshold_bytes(value: Option<u64>, flag: &str) -> Result<(), String> {
    if value == Some(0) {
        return Err(format!(
            "{flag} must be greater than 0 (0 would reject every artifact)"
        ));
    }
    Ok(())
}

/// Validates an operator-supplied percentage threshold (finite, non-negative).
fn validate_size_threshold_pct(value: Option<f64>, flag: &str) -> Result<(), String> {
    if let Some(v) = value {
        if !v.is_finite() {
            return Err(format!("{flag} must be a finite number (got {v})"));
        }
        if v < 0.0 {
            return Err(format!("{flag} must not be negative (got {v})"));
        }
    }
    Ok(())
}

/// Renders a growth value for policy messages, including the undefined case.
fn format_growth_pct(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{p}%"),
        None => "undefined (zero-byte baseline)".to_string(),
    }
}

/// Evaluate the operator-supplied size policy against a measured size pair.
///
/// Returns the violated rules (empty when satisfied). Pure, shared by
/// `sdkt diff` and `sdkt release-assurance` so both commands enforce the same
/// semantics. Boundaries are inclusive: a violation requires size `>` limit or
/// growth `>` pct — a value exactly at the threshold passes. `old_bytes: None`
/// means no baseline is available (growth requested without one) and is always
/// a violation rather than a silent skip. `size_delta_pct` returning `None`
/// (zero-byte baseline, non-empty candidate) is also a violation: the request
/// cannot be evaluated honestly.
fn evaluate_size_policy(
    old_bytes: Option<usize>,
    new_bytes: usize,
    max_size_bytes: Option<u64>,
    max_growth_pct: Option<f64>,
) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(max) = max_size_bytes {
        if new_bytes as u64 > max {
            violations.push(format!(
                "artifact size {} bytes exceeds --max-size-bytes {}",
                new_bytes, max
            ));
        }
    }
    if let Some(max_pct) = max_growth_pct {
        match old_bytes {
            None => violations.push(
                "--max-growth-pct requires a previous artifact (--previous-wasm) to measure growth"
                    .to_string(),
            ),
            Some(old) => match sdkt_wasm::size_delta_pct(old, new_bytes) {
                Some(pct) if pct > max_pct => violations.push(format!(
                    "size growth {} exceeds --max-growth-pct {}",
                    format_growth_pct(Some(pct)),
                    max_pct
                )),
                Some(_) => {}
                None => violations.push(format!(
                    "size growth is {} — cannot be compared against --max-growth-pct {}",
                    format_growth_pct(None),
                    max_pct
                )),
            },
        }
    }
    violations
}

/// Map a [`sdkt_wasm::SpecDiff`]'s sizes onto the shared size policy.
fn evaluate_size_policy_for_diff(
    diff: &sdkt_wasm::SpecDiff,
    max_size_bytes: Option<u64>,
    max_growth_pct: Option<f64>,
) -> Vec<String> {
    evaluate_size_policy(
        Some(diff.old.size_bytes),
        diff.new.size_bytes,
        max_size_bytes,
        max_growth_pct,
    )
}

/// Apply the opt-in size policy to a finished release-assurance report.
///
/// Mutates the artifact section (status → FAIL, violation text appended to its
/// detail) and then re-runs the existing aggregation so `release_status`,
/// `reasons`, and the process exit code stay internally consistent. No policy
/// flags ⇒ the report is returned untouched.
fn apply_size_policy_to_assurance(
    report: &mut ReleaseAssuranceReport,
    previous_wasm_path: Option<&str>,
    max_size_bytes: Option<u64>,
    max_growth_pct: Option<f64>,
) -> Result<(), String> {
    if max_size_bytes.is_none() && max_growth_pct.is_none() {
        return Ok(());
    }
    // An unparsable candidate has no measurable size; the artifact section is
    // already FAIL and the report must not be softened by policy.
    let Some(new_bytes) = report.artifact.metadata.as_ref().map(|m| m.size_bytes) else {
        return Ok(());
    };
    // Growth needs the baseline size only; a missing/unreadable file is an
    // explicit error (the caller already required --previous-wasm for growth).
    let previous_size_bytes = match (previous_wasm_path, max_growth_pct) {
        (Some(path), Some(_)) => Some(
            fs::read(path)
                .map_err(|e| format!("Failed to read previous WASM {}: {}", path, e))?
                .len(),
        ),
        _ => None,
    };
    let violations = evaluate_size_policy(
        previous_size_bytes,
        new_bytes,
        max_size_bytes,
        max_growth_pct,
    );
    if violations.is_empty() {
        return Ok(());
    }
    for v in &violations {
        report
            .artifact
            .check
            .detail
            .push_str(&format!("; size policy: {v}"));
    }
    report.artifact.check.status = RaStatus::Fail;
    // Re-aggregate with the existing model so status/reasons/exit stay coherent.
    let statuses = [
        report.artifact.check.status,
        report.security.check.status,
        report.upgrade_safety.check.status,
        report.verification.check.status,
        report.health.check.status,
        // A recorded fuzz-evidence status must survive re-aggregation.
        report
            .fuzz
            .as_ref()
            .map(|f| f.check.status)
            .unwrap_or(RaStatus::Pass),
    ];
    report.release_status = aggregate_release_status(&statuses);
    report.reasons.retain(|r| r != "Artifact: FAIL");
    report.reasons.insert(0, "Artifact: FAIL".to_string());
    Ok(())
}

/// Pretty-print the aggregate report to stdout.
fn print_release_assurance_pretty(r: &ReleaseAssuranceReport) {
    println!("Soroban Release Assurance");
    println!("────────────────────────");
    println!("Artifact        {}", r.artifact.check.status);
    println!("Security        {}", r.security.check.status);
    println!("Upgrade Safety  {}", r.upgrade_safety.check.status);
    println!("Verification    {}", r.verification.check.status);
    println!("Contract Health {}", r.health.check.status);
    if let Some(f) = &r.fuzz {
        println!("Fuzz Evidence   {}", f.check.status);
    }
    println!();
    println!("RELEASE STATUS  {}", r.release_status.to_uppercase());
    if !r.reasons.is_empty() {
        println!();
        println!("Reasons:");
        for reason in &r.reasons {
            println!("  - {}", reason);
        }
    }
}

/// Read-only release-assurance composition reusing the existing SDKT engines:
/// artifact (sdkt-wasm metadata) → security (sdkt-audit) → upgrade safety
/// (sdkt-wasm diff/verdict) → deployed verification (`verify_contract`) →
/// contract health (`contract_health`), aggregated into one release status.
///
/// No mutation, signing, submission, TTL extend/restore, or filesystem writes
/// are performed. The returned report is printed and exit-coded by the caller.
//
/// One parameter per `ReleaseAssurance` CLI flag; bundling would only relocate
/// the clap surface into a throwaway struct.
#[allow(clippy::too_many_arguments)]
async fn run_release_assurance(
    client: &SorobanRpcClient,
    wasm_path: &str,
    previous_wasm_path: Option<&str>,
    audit_paths: &[String],
    disable: &[String],
    contract_id: Option<&str>,
    network: &str,
    fuzz_evidence: Option<&str>,
) -> Result<ReleaseAssuranceReport, String> {
    // ---- 1. ARTIFACT (existing sdkt-wasm metadata engine, offline) ----
    let candidate_bytes = fs::read(wasm_path)
        .map_err(|e| format!("Failed to read WASM file {}: {}", wasm_path, e))?;

    let artifact = match sdkt_wasm::parse_metadata(&candidate_bytes) {
        Ok(metadata) => {
            let detail = format!(
                "valid WASM v{} ({} bytes, sha256 {})",
                metadata.version, metadata.size_bytes, metadata.hash
            );
            ArtifactResult {
                check: RaCheck {
                    status: RaStatus::Pass,
                    detail,
                },
                metadata: Some(metadata),
            }
        }
        Err(e) => ArtifactResult {
            check: RaCheck {
                status: RaStatus::Fail,
                detail: format!("{} is not valid WASM: {}", wasm_path, e),
            },
            metadata: None,
        },
    };

    // ---- 2. SECURITY (existing sdkt-audit engine, offline) ----
    let security = run_release_security(audit_paths, disable).await;

    // ---- 3. UPGRADE SAFETY (existing upgrade engine, offline vs previous or live) ----
    let upgrade = run_release_upgrade_safety(
        client,
        &candidate_bytes,
        previous_wasm_path,
        contract_id,
        network,
        &artifact,
    )
    .await;

    // ---- 4. VERIFICATION + 5. HEALTH (existing verify_contract / contract_health) ----
    // Errors carry (status, message): a not-found contract is a blocking FAIL,
    // anything else is an ERROR; both keep the engine's own message.
    let (verification, health): (
        OnChainResult<VerificationReport>,
        OnChainResult<ContractHealthReport>,
    ) = match contract_id {
        None => (
            Err((
                RaStatus::Skipped,
                "--contract not supplied; on-chain check skipped".to_string(),
            )),
            Err((
                RaStatus::Skipped,
                "--contract not supplied; on-chain check skipped".to_string(),
            )),
        ),
        Some(_) if artifact.check.status != RaStatus::Pass => (
            Err((
                RaStatus::Skipped,
                "candidate WASM artifact is invalid; on-chain check skipped".to_string(),
            )),
            Err((
                RaStatus::Skipped,
                "candidate WASM artifact is invalid; on-chain check skipped".to_string(),
            )),
        ),
        Some(cid) => {
            let v = verify_contract(client, cid, Some(&candidate_bytes), network)
                .await
                .map_err(|e| (contract_not_found_status(&e), e));
            let h = contract_health(client, cid, Some(&candidate_bytes), network)
                .await
                .map_err(|e| (contract_not_found_status(&e), e));
            (v, h)
        }
    };

    let verification_result = match verification {
        Ok(report) => VerifyResult {
            check: RaCheck {
                status: verification_status(report.matches),
                detail: if report.explanation.is_empty() {
                    report.verification_status.clone()
                } else {
                    report.explanation.clone()
                },
            },
            report: Some(report),
        },
        Err((status, msg)) => VerifyResult {
            check: RaCheck {
                status,
                detail: msg,
            },
            report: None,
        },
    };

    let health_result = match health {
        Ok(report) => {
            let st = health_status(&report.health);
            let detail = if report.reasons.is_empty() {
                format!("health: {}", report.health)
            } else {
                report.reasons.join(" ")
            };
            HealthResult {
                check: RaCheck { status: st, detail },
                report: Some(report),
            }
        }
        Err((status, msg)) => HealthResult {
            check: RaCheck {
                status,
                detail: msg,
            },
            report: None,
        },
    };

    // ---- 6. FUZZ EVIDENCE (recorded, never executed here) ----
    //
    // `--fuzz-evidence` attaches the evidence JSON written by an already
    // completed `sdkt fuzz` campaign. Release assurance does not run
    // campaigns: it records what a completed one produced, and only
    // `Classification::Finding` entries count as security evidence.
    let (fuzz_evidence, fuzz_status) = read_fuzz_evidence(fuzz_evidence)?;

    let statuses = [
        artifact.check.status,
        security.check.status,
        upgrade.check.status,
        verification_result.check.status,
        health_result.check.status,
    ];
    let release_status = match fuzz_status {
        // Fold the recorded evidence status in as one more section.
        Some(fuzz) => {
            let mut all = statuses.to_vec();
            all.push(fuzz);
            aggregate_release_status(&all)
        }
        None => aggregate_release_status(&statuses),
    };

    let mut reasons = Vec::new();
    for (label, s) in [
        ("Artifact", artifact.check.status),
        ("Security", security.check.status),
        ("Upgrade Safety", upgrade.check.status),
        ("Verification", verification_result.check.status),
        ("Contract Health", health_result.check.status),
    ] {
        if matches!(s, RaStatus::Fail | RaStatus::Error | RaStatus::Review) {
            reasons.push(format!("{}: {}", label, s));
        }
    }
    if let Some(s) = fuzz_status {
        if matches!(s, RaStatus::Fail | RaStatus::Error | RaStatus::Review) {
            reasons.push(format!("Fuzz Evidence: {s}"));
        }
    }

    Ok(ReleaseAssuranceReport {
        contract: contract_id.map(|c| c.to_string()),
        network: network.to_string(),
        artifact,
        security,
        upgrade_safety: upgrade,
        verification: verification_result,
        health: health_result,
        fuzz: fuzz_evidence,
        release_status,
        reasons,
    })
}

/// Read a fuzz evidence JSON file produced by a completed `sdkt fuzz`
/// campaign.
///
/// Returns `(section, status)`: the section is `None` only when
/// `--fuzz-evidence` was not supplied (existing releases with no fuzz
/// section are unaffected), and the status folds into the aggregate. Only
/// `Classification::Finding` entries are security evidence; a completed
/// campaign with zero findings is a clean record and stays SKIPPED.
fn read_fuzz_evidence(
    path: Option<&str>,
) -> Result<(Option<FuzzEvidenceResult>, Option<RaStatus>), String> {
    let Some(path) = path else {
        return Ok((None, None));
    };
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to read fuzz evidence {path}: {e}"))?;
    let evidence: sdkt_fuzz::CampaignEvidence =
        serde_json::from_reader(std::io::BufReader::new(file))
            .map_err(|e| format!("Failed to parse fuzz evidence {path}: {e}"))?;

    let status = if !evidence.campaign_completed {
        RaStatus::Error
    } else if !evidence.findings.is_empty() {
        RaStatus::Fail
    } else {
        // Completed with zero findings: a clean record, not "not executed".
        RaStatus::Pass
    };
    let detail = format!(
        "fuzz campaign: cases={} findings={} minimized={} artifacts={} replayed={} deterministic={}",
        evidence.case_count,
        evidence.findings.len(),
        evidence.minimized_findings,
        evidence.artifacts_available,
        evidence.replay.len(),
        evidence.deterministic,
    );
    Ok((
        Some(FuzzEvidenceResult {
            check: RaCheck { status, detail },
            evidence,
        }),
        Some(status),
    ))
}

/// ---- 2. SECURITY engine reuse ----
async fn run_release_security(audit_paths: &[String], disable: &[String]) -> SecurityResult {
    if audit_paths.is_empty() {
        return SecurityResult {
            check: RaCheck {
                status: RaStatus::Skipped,
                detail: "no --audit supplied; static security audit not executed".to_string(),
            },
            audit: None,
        };
    }

    let disabled_refs: Vec<&str> = disable.iter().map(String::as_str).collect();

    let mut source_paths = Vec::new();
    for input in audit_paths {
        let input_path = std::path::Path::new(input);
        match collect_rust_sources(input_path) {
            Ok(mut files) => source_paths.append(&mut files),
            Err(e) => {
                return SecurityResult {
                    check: RaCheck {
                        status: RaStatus::Error,
                        detail: format!("Failed to discover Rust sources '{}': {}", input, e),
                    },
                    audit: None,
                };
            }
        }
    }
    source_paths.sort();
    source_paths.dedup();

    if source_paths.is_empty() {
        return SecurityResult {
            check: RaCheck {
                status: RaStatus::Error,
                detail: "No Rust source files (.rs) found in the supplied --audit paths"
                    .to_string(),
            },
            audit: None,
        };
    }

    let mut aggregate = sdkt_audit::AuditReport::default();
    for source_path in &source_paths {
        let source = match fs::read_to_string(source_path) {
            Ok(source) => source,
            Err(e) => {
                aggregate.add(sdkt_audit::Finding {
                    rule_id: "AUDIT-IO".to_string(),
                    severity: sdkt_audit::Severity::Critical,
                    message: format!("Failed to read source: {}", e),
                    location: None,
                    file: Some(source_path.display().to_string()),
                });
                continue;
            }
        };

        match sdkt_audit::audit_source_with(&source, &disabled_refs) {
            Ok(report) => {
                for f in &report.findings {
                    aggregate.add(f.clone());
                }
            }
            Err(_) => {
                aggregate.add(sdkt_audit::Finding {
                    rule_id: "AUDIT-PARSE".to_string(),
                    severity: sdkt_audit::Severity::Critical,
                    message: "Failed to parse Rust source".to_string(),
                    location: None,
                    file: Some(source_path.display().to_string()),
                });
            }
        }
    }

    let status = audit_status(&aggregate.summary);
    let detail = format!(
        "{} critical, {} warning, {} info findings across {} file(s)",
        aggregate.summary.critical,
        aggregate.summary.warning,
        aggregate.summary.info,
        source_paths.len()
    );

    SecurityResult {
        check: RaCheck { status, detail },
        audit: Some(aggregate),
    }
}

/// ---- 3. UPGRADE SAFETY engine reuse ----
async fn run_release_upgrade_safety(
    client: &SorobanRpcClient,
    candidate_bytes: &[u8],
    previous_wasm_path: Option<&str>,
    contract_id: Option<&str>,
    network: &str,
    artifact: &ArtifactResult,
) -> UpgradeResult {
    if artifact.check.status != RaStatus::Pass {
        return UpgradeResult {
            check: RaCheck {
                status: RaStatus::Skipped,
                detail: "candidate WASM artifact is invalid; upgrade-safety skipped".to_string(),
            },
            baseline: "none".to_string(),
            verdict: None,
        };
    }

    if let Some(prev_path) = previous_wasm_path {
        match fs::read(prev_path) {
            Ok(prev_bytes) => match sdkt_wasm::upgrade_safety_wasm(&prev_bytes, candidate_bytes) {
                Ok(verdict) => {
                    let detail = if verdict.compatible {
                        format!(
                            "compatible ({} breaking, {} non-breaking)",
                            verdict.breaking_changes.len(),
                            verdict.non_breaking_changes.len()
                        )
                    } else {
                        format!(
                            "INCOMPATIBLE ({} breaking changes)",
                            verdict.breaking_changes.len()
                        )
                    };
                    UpgradeResult {
                        check: RaCheck {
                            status: upgrade_verdict_status(&verdict),
                            detail,
                        },
                        baseline: format!("previous WASM {}", prev_path),
                        verdict: Some(verdict),
                    }
                }
                Err(e) => UpgradeResult {
                    check: RaCheck {
                        status: RaStatus::Error,
                        detail: format!("failed to diff baseline WASM: {}", e),
                    },
                    baseline: format!("previous WASM {}", prev_path),
                    verdict: None,
                },
            },
            Err(e) => UpgradeResult {
                check: RaCheck {
                    status: RaStatus::Error,
                    detail: format!("Failed to read previous WASM {}: {}", prev_path, e),
                },
                baseline: format!("previous WASM {}", prev_path),
                verdict: None,
            },
        }
    } else if let Some(cid) = contract_id {
        // Offline upgrade-safety needs a baseline: fetch the deployed WASM.
        match upgrade_assurance_against_deployed(client, cid, candidate_bytes, network).await {
            Ok((_diff, verdict, wasm_hash)) => {
                let detail = if verdict.compatible {
                    format!(
                        "compatible vs live {} ({} breaking, {} non-breaking)",
                        wasm_hash,
                        verdict.breaking_changes.len(),
                        verdict.non_breaking_changes.len()
                    )
                } else {
                    format!(
                        "INCOMPATIBLE vs live {} ({} breaking changes)",
                        wasm_hash,
                        verdict.breaking_changes.len()
                    )
                };
                UpgradeResult {
                    check: RaCheck {
                        status: upgrade_verdict_status(&verdict),
                        detail,
                    },
                    baseline: format!("live contract {}", cid),
                    verdict: Some(verdict),
                }
            }
            Err(e) => UpgradeResult {
                check: RaCheck {
                    status: contract_not_found_status(&e),
                    detail: e,
                },
                baseline: format!("live contract {}", cid),
                verdict: None,
            },
        }
    } else {
        UpgradeResult {
            check: RaCheck {
                status: RaStatus::Skipped,
                detail: "no --previous-wasm and no --contract; upgrade-safety needs a baseline (off-line diff vs on-chain)".to_string(),
            },
            baseline: "none".to_string(),
            verdict: None,
        }
    }
}

/// Parse a `--salt` value (40 hex chars) into a 20-byte deployment salt.
/// Shared by `deploy` and `project deploy`; validates strictly.
fn parse_salt_hex(s: &str) -> Result<[u8; 20], String> {
    let sh = s.trim();
    if sh.len() != 40 {
        return Err(format!(
            "Invalid --salt: must be 20-byte hex (40 hex chars), got length {}",
            sh.len()
        ));
    }
    if let Some(pos) = sh.chars().position(|c| !c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid --salt: character at index {} is not a hex digit",
            pos
        ));
    }
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = u8::from_str_radix(&sh[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("Invalid --salt hex at byte {}: {}", i, e))?;
    }
    Ok(out)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Warning for `sdkt events --topic` when `symbol` will not match the event's
/// `topic[0]` on the wire. For a `#[contractevent]` that is its first prefix
/// topic (the snake_case event name by default), not the declared event name,
/// so querying by the name (`Transfer`) silently matches nothing when the
/// prefix differs (`transfer`). Events without prefix topics fall back to their
/// name. Returns `None` when the symbol is a wire topic of some declared event.
fn unknown_event_topic_warning(spec: &sdkt_wasm::ContractSpec, symbol: &str) -> Option<String> {
    // (declared name, topic[0] on the wire) per event.
    let events: Vec<(&str, &str)> = spec
        .events
        .iter()
        .map(|ev| {
            let wire = ev.prefix_topics.first().unwrap_or(&ev.name);
            (ev.name.as_str(), wire.as_str())
        })
        .collect();
    if events.iter().any(|(_, wire)| *wire == symbol) {
        return None;
    }
    if let Some((_, wire)) = events.iter().find(|(name, _)| *name == symbol) {
        return Some(format!(
            "Warning: '{symbol}' is the ABI name of an event, not its wire topic; \
             did you mean '{wire}'? (the wire topic is the prefix topic). Querying anyway."
        ));
    }
    let mut msg = format!("Warning: event topic '{symbol}' is not declared in the contract ABI");
    if events.is_empty() {
        msg.push_str(" (the ABI declares no events)");
    } else {
        let close = events.iter().find(|(name, wire)| {
            name.eq_ignore_ascii_case(symbol) || wire.eq_ignore_ascii_case(symbol)
        });
        if let Some((_, wire)) = close {
            msg.push_str(&format!(
                "; did you mean '{wire}'? (topics are case-sensitive)"
            ));
        }
        let mut declared: Vec<&str> = Vec::new();
        for (_, wire) in &events {
            if !declared.contains(wire) {
                declared.push(wire);
            }
        }
        msg.push_str(&format!("; declared: {}", declared.join(", ")));
    }
    msg.push_str(". Querying anyway.");
    Some(msg)
}

fn parse_format_str(s: &str) -> OutputFormat {
    match s.to_lowercase().as_str() {
        "json" => OutputFormat::Json,
        "pretty" => OutputFormat::Pretty,
        other => {
            eprintln!("Invalid format '{}'. Use 'json' or 'pretty'.", other);
            process::exit(1);
        }
    }
}

/// Pretty-print a decoded transaction envelope for `tx decode`.
fn print_envelope_pretty(view: &sdkt_xdr::EnvelopeView) {
    println!("Transaction Envelope ({}):", view.envelope_type);
    println!("  Source:     {}", view.source);
    println!("  Sequence:   {}", view.sequence);
    println!("  Fee:        {} stroops", view.fee);
    if let Some(memo) = &view.memo {
        println!("  Memo:       {}", memo);
    }
    println!("  Operations ({}):", view.operations.len());
    for (i, op) in view.operations.iter().enumerate() {
        println!("    [{}] {}", i, op.kind);
        if let Some(source) = &op.source {
            println!("        Source:    {}", source);
        }
        if let Some(contract) = &op.contract {
            println!("        Contract:  {}", contract);
        }
        if let Some(function) = &op.function {
            println!("        Function:  {}", function);
        }
        if !op.args.is_empty() {
            println!("        Args:      {}", op.args.join(", "));
        }
        if op.auth_entries > 0 {
            println!("        Auth:      {} entries", op.auth_entries);
        }
    }
    if let Some(soroban) = &view.soroban {
        println!("  Soroban Data:");
        println!(
            "    Footprint:    {} read-only, {} read-write",
            soroban.read_only, soroban.read_write
        );
        println!("    Instructions: {}", soroban.instructions);
        println!("    Disk reads:   {} bytes", soroban.disk_read_bytes);
        println!("    Writes:       {} bytes", soroban.write_bytes);
        println!("    Resource fee: {} stroops", soroban.resource_fee);
    }
    print_signatures("  ", &view.signatures);
    if let Some(bump) = &view.fee_bump {
        println!("  Fee Bump:");
        println!("    Fee source: {}", bump.fee_source);
        println!("    Fee:        {} stroops", bump.fee);
        print_signatures("    ", &bump.signatures);
    }
}

fn print_signatures(indent: &str, signatures: &[sdkt_xdr::envelope::SignatureView]) {
    println!("{}Signatures: {}", indent, signatures.len());
    for sig in signatures {
        match &sig.signer {
            Some(signer) => println!("{}  - ed25519, {}", indent, signer),
            None => println!("{}  - hint {} (signer not in envelope)", indent, sig.hint),
        }
    }
}

/// Pretty-print a [`sdkt_storage::SnapshotDiff`] to stdout.
fn print_diff_pretty(diff: &sdkt_storage::SnapshotDiff) {
    use sdkt_storage::DiffStatus;

    println!("Storage Diff for Contract: {}", diff.contract_id);
    println!("Total entries: {}", diff.entries.len());

    let removed: Vec<_> = diff
        .entries
        .iter()
        .filter(|e| e.status == DiffStatus::Removed)
        .collect();
    let expiring: Vec<_> = diff
        .entries
        .iter()
        .filter(|e| e.status == DiffStatus::ExpiringSoon)
        .collect();
    let unchanged: Vec<_> = diff
        .entries
        .iter()
        .filter(|e| e.status == DiffStatus::Unchanged)
        .collect();

    println!(
        "  Removed:       {} | Expiring Soon: {} | Unchanged: {}",
        removed.len(),
        expiring.len(),
        unchanged.len()
    );

    if !removed.is_empty() {
        println!("\nRemoved entries ({}):", removed.len());
        for e in &removed {
            println!(
                "  [removed] key={}{} (old_ttl={})",
                e.key,
                e.label
                    .as_ref()
                    .map_or(String::new(), |l| format!(" label={l}")),
                e.old_ttl.map_or("?".to_string(), |t| t.to_string())
            );
        }
    }
    if !expiring.is_empty() {
        println!("\nExpiring soon ({}):", expiring.len());
        for e in &expiring {
            println!(
                "  [expiring] key={}{} (ttl={})",
                e.key,
                e.label
                    .as_ref()
                    .map_or(String::new(), |l| format!(" label={l}")),
                e.new_ttl.map_or("?".to_string(), |t| t.to_string())
            );
        }
    }
    if unchanged.iter().any(|e| e.label.is_some()) {
        println!("\nUnchanged entries ({}):", unchanged.len());
        for e in &unchanged {
            println!(
                "  [unchanged] key={}{} (ttl={})",
                e.key,
                e.label
                    .as_ref()
                    .map_or(String::new(), |l| format!(" label={l}")),
                e.new_ttl.map_or("?".to_string(), |t| t.to_string())
            );
        }
    }
    if unchanged.is_empty() && removed.is_empty() && expiring.is_empty() {
        println!("  (no entries)");
    }
}

/// Project an ABI key name without changing the raw LedgerKey used for identity.
fn storage_key_label(raw: &str, spec: &sdkt_wasm::ContractSpec) -> String {
    use stellar_xdr::ScVal;

    let Ok(stellar_xdr::LedgerKey::ContractData(data)) = sdkt_xdr::decode_ledger_key(raw) else {
        return raw.to_string();
    };
    if matches!(data.key, ScVal::LedgerKeyContractInstance) {
        return "instance".to_string();
    }
    let (name, args): (String, &[ScVal]) = match &data.key {
        ScVal::Symbol(name) => (name.to_utf8_string_lossy(), &[]),
        ScVal::Vec(Some(items)) => match items.first() {
            Some(ScVal::Symbol(name)) => (name.to_utf8_string_lossy(), &items[1..]),
            _ => return raw.to_string(),
        },
        _ => return raw.to_string(),
    };
    let name = name.as_str();
    let mut matches =
        spec.custom_types
            .iter()
            .filter(|ty| ty.kind == "union")
            .flat_map(|ty| {
                ty.members.iter().filter_map(move |member| {
                    (member.name == name
                        && member.types.len() == args.len()
                        && member.types.iter().zip(args).all(|(ty, value)| {
                            match ty.name.as_str() {
                                "bool" => matches!(value, ScVal::Bool(_)),
                                "u32" => matches!(value, ScVal::U32(_)),
                                "i32" => matches!(value, ScVal::I32(_)),
                                "u64" => matches!(value, ScVal::U64(_)),
                                "i64" => matches!(value, ScVal::I64(_)),
                                "u128" => matches!(value, ScVal::U128(_)),
                                "i128" => matches!(value, ScVal::I128(_)),
                                "string" => matches!(value, ScVal::String(_)),
                                "symbol" => matches!(value, ScVal::Symbol(_)),
                                "address" => matches!(value, ScVal::Address(_)),
                                "bytes" => matches!(value, ScVal::Bytes(_)),
                                _ => true,
                            }
                        }))
                    .then_some((ty.name.as_str(), member))
                })
            });
    let Some((type_name, member)) = matches.next() else {
        return raw.to_string();
    };
    if matches.next().is_some() {
        return raw.to_string();
    }
    if member.types.is_empty() {
        format!("{type_name}::{}", member.name)
    } else {
        let args = member
            .types
            .iter()
            .map(|ty| ty.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{type_name}::{}({args})", member.name)
    }
}

/// Shared typed-argument parser used by `call`, `tx build`, and `invoke`.
///
/// Accepts `TYPE:VALUE` pairs (u32|i32|u64|i64|timepoint|duration|u128|i128|bool|string|bytes|
/// address) and returns base64-encoded `ScVal` strings ready for
/// `InvokeTransactionParams::args`. Values without a recognized `TYPE:` prefix
/// are passed through as-is (assumed pre-encoded base64 ScVal), matching the
/// historical `tx build` behavior.
/// Decode a `bytes:` hex value, shared by `parse_typed_args` and `run_encode`
/// so both report the same error for the same input. Non-ASCII input is
/// rejected up front: slicing at byte offsets would otherwise land inside a
/// multi-byte character and panic.
fn parse_hex_bytes(raw: &str) -> Result<Vec<u8>, String> {
    let hex = raw.trim();
    if !hex.is_ascii() {
        return Err(format!("invalid bytes value: {raw} (expected ASCII hex)"));
    }
    if !hex.len().is_multiple_of(2) {
        return Err(format!(
            "invalid bytes value: {raw} (hex must have an even number of digits)"
        ));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|_| format!("invalid bytes value: {raw} (invalid hex byte)"))
        })
        .collect()
}

fn parse_typed_args(args: &[String], strict: bool) -> Result<Vec<String>, String> {
    use sdkt_xdr::{scval_to_base64, Address, IntoScVal};
    use stellar_xdr::{Duration, TimePoint};
    let mut parsed = Vec::new();
    for a in args.iter() {
        if let Some((t, v)) = a.split_once(':') {
            let b64 = match t.to_lowercase().as_str() {
                "u32" => {
                    let n: u32 = v.parse().map_err(|_| format!("invalid u32 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "i32" => {
                    let n: i32 = v.parse().map_err(|_| format!("invalid i32 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "u64" => {
                    let n: u64 = v.parse().map_err(|_| format!("invalid u64 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "timepoint" => {
                    let n: u64 = v
                        .parse()
                        .map_err(|_| format!("invalid timepoint value: {v}"))?;
                    scval_to_base64(&TimePoint(n).into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "duration" => {
                    let n: u64 = v
                        .parse()
                        .map_err(|_| format!("invalid duration value: {v}"))?;
                    scval_to_base64(&Duration(n).into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "i64" => {
                    let n: i64 = v.parse().map_err(|_| format!("invalid i64 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "u128" => {
                    let n: u128 = v.parse().map_err(|_| format!("invalid u128 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "i128" => {
                    let n: i128 = v.parse().map_err(|_| format!("invalid i128 value: {v}"))?;
                    scval_to_base64(&n.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "bool" => {
                    let b: bool = v.parse().map_err(|_| format!("invalid bool value: {v}"))?;
                    scval_to_base64(&b.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "string" => scval_to_base64(&v.into_scval().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?,
                "bytes" => {
                    let b = parse_hex_bytes(v)?;
                    scval_to_base64(&b.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "address" => {
                    let addr = Address::from_strkey(v)
                        .map_err(|_| format!("invalid Stellar address: {v}"))?;
                    scval_to_base64(&addr.into_scval().map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                "symbol" => {
                    use stellar_xdr::{ScSymbol, ScVal};
                    if v.len() > 32 {
                        return Err(format!("symbol exceeds 32 bytes (got {} bytes)", v.len()));
                    }
                    if !v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                        return Err(
                            "invalid symbol value: use only ASCII letters, digits, and _".into(),
                        );
                    }
                    let sym_val = ScVal::Symbol(
                        ScSymbol::try_from(v).map_err(|_| "invalid symbol value".to_string())?,
                    );
                    scval_to_base64(&sym_val).map_err(|e| e.to_string())?
                }
                _ => {
                    if strict {
                        return Err(format!(
                            "unknown arg type '{t}'. Use u32|i32|u64|i64|timepoint|duration|u128|i128|bool|string|symbol|bytes|address"
                        ));
                    }
                    a.clone() // passthrough: pre-encoded base64 ScVal
                }
            };
            parsed.push(b64);
        } else if strict {
            return Err(format!(
                "invalid arg format '{a}'. Use TYPE:VALUE (e.g. u32:100, address:G...)"
            ));
        } else {
            parsed.push(a.clone());
        }
    }
    Ok(parsed)
}

fn render_contract_interface(spec: &sdkt_wasm::ContractSpec, markdown: bool) -> String {
    let mut out = String::new();
    let heading = if markdown {
        "# Contract Interface"
    } else {
        "Contract Interface"
    };
    out.push_str(heading);
    out.push_str("\n\n");
    out.push_str(if markdown {
        "## Functions\n\n"
    } else {
        "Functions:\n"
    });
    for function in &spec.functions {
        let params = function
            .parameters
            .iter()
            .map(|p| format!("{}: {}", p.name, p.type_.name))
            .collect::<Vec<_>>()
            .join(", ");
        let outputs = function
            .outputs
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let signature = format!(
            "{}({}) -> {}",
            function.name,
            params,
            if outputs.is_empty() { "()" } else { &outputs }
        );
        if markdown {
            out.push_str(&format!("- `{signature}`\n"));
        } else {
            out.push_str(&format!("  {}\n", signature));
        }
        if !function.doc.is_empty() {
            out.push_str(&format!("  {}\n", function.doc));
        }
    }
    if !spec.events.is_empty() {
        out.push_str(if markdown {
            "\n## Events\n\n"
        } else {
            "\nEvents:\n"
        });
        for event in &spec.events {
            out.push_str(&format!(
                "{}{}\n",
                if markdown { "- " } else { "  " },
                event.name
            ));
        }
    }
    if !spec.custom_types.is_empty() {
        out.push_str(if markdown {
            "\n## Types\n\n"
        } else {
            "\nTypes:\n"
        });
        for ty in &spec.custom_types {
            let members = ty
                .members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "{}{}{}{}\n",
                if markdown { "- **" } else { "  " },
                ty.name,
                if markdown { "**" } else { "" },
                if members.is_empty() {
                    format!(" ({})", ty.kind)
                } else {
                    format!(" ({}) {{{}}}", ty.kind, members)
                }
            ));
            if !ty.doc.is_empty() {
                out.push_str(&format!("  {}\n", ty.doc));
            }
        }
    }
    out.trim_end().to_string()
}

/// Parse a `storage read` durability value into `ContractDataDurability`.
fn parse_durability(s: &str) -> Result<stellar_xdr::ContractDataDurability, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "persistent" | "p" => Ok(stellar_xdr::ContractDataDurability::Persistent),
        "temporary" | "temp" | "t" => Ok(stellar_xdr::ContractDataDurability::Temporary),
        other => Err(format!(
            "invalid durability '{other}' (expected persistent|temporary)"
        )),
    }
}

/// Resolve the effective base64 `LedgerKey` for `storage read` from its mutually
/// exclusive key sources: the raw `--key-xdr` escape hatch, a typed
/// `--map-key`/`--key-arg` spec, or `--instance`.
///
/// Pure (no network/I/O) so the key-construction and validation logic is
/// unit-testable without a live RPC.
fn resolve_storage_read_key(
    contract: &str,
    key_xdr: Option<&str>,
    map_key: Option<&str>,
    key_arg: &[String],
    instance: bool,
    durability: &str,
) -> Result<String, String> {
    let sources = [key_xdr.is_some(), map_key.is_some(), instance]
        .into_iter()
        .filter(|b| *b)
        .count();
    if sources == 0 {
        return Err(
            "provide a key via --key-xdr <BASE64|HEX>, --map-key <SYMBOL> \
             [--key-arg TYPE:VALUE ...], or --instance"
                .to_string(),
        );
    }
    if sources > 1 {
        return Err("specify only one of --key-xdr, --map-key, or --instance".to_string());
    }
    if !key_arg.is_empty() && map_key.is_none() {
        return Err("--key-arg requires --map-key".to_string());
    }

    if let Some(raw) = key_xdr {
        if raw.trim().is_empty() {
            return Err("--key-xdr must not be empty".to_string());
        }
        // Escape hatch: pass the raw LedgerKey through unchanged.
        return Ok(raw.to_string());
    }

    // Typed construction.
    let (key, dur) = if instance {
        // Instance storage is a single, always-persistent ledger entry.
        (
            stellar_xdr::ScVal::LedgerKeyContractInstance,
            stellar_xdr::ContractDataDurability::Persistent,
        )
    } else {
        let symbol = map_key.expect("map_key present when instance/key_xdr absent");
        let args_b64 = parse_typed_args(key_arg, true)?;
        let key = sdkt_xdr::build_map_key(symbol, &args_b64).map_err(|e| e.to_string())?;
        (key, parse_durability(durability)?)
    };

    sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
        contract: contract.to_string(),
        key,
        durability: dur,
    })
    .map_err(|e| format!("failed to build LedgerKey: {e}"))
}

/// Resolve the list of extra `LedgerKey`s for `storage analyze` from the supplied
/// CLI arguments.
///
/// Accepts repeatable raw keys via `key_xdr` (`--key-xdr`/`--key`) and/or a typed
/// key specification via `--map-key`/`--key-arg`/`--durability`.
///
/// Validates key specifications offline without network/RPC calls.
fn resolve_storage_analyze_keys(
    contract: &str,
    key_xdr: &[String],
    map_key: Option<&str>,
    key_arg: &[String],
    durability: &str,
) -> Result<Vec<String>, String> {
    if !key_arg.is_empty() && map_key.is_none() {
        return Err("--key-arg requires --map-key".to_string());
    }

    let mut keys = Vec::new();

    for raw in key_xdr {
        if raw.trim().is_empty() {
            return Err("--key-xdr must not be empty".to_string());
        }
        // Validate that raw is valid base64 or hex XDR for a LedgerKey
        sdkt_xdr::decode_ledger_key(raw).map_err(|e| format!("invalid LedgerKey: {e}"))?;
        keys.push(raw.to_string());
    }

    if let Some(symbol) = map_key {
        let typed_key =
            resolve_storage_read_key(contract, None, Some(symbol), key_arg, false, durability)?;
        keys.push(typed_key);
    } else if durability != "persistent" {
        // Validate durability even if map_key is absent, so invalid durability flags error offline.
        parse_durability(durability)?;
    }

    Ok(keys)
}

/// Encode typed `TYPE:VALUE` values to a single base64 XDR `ScVal` string.
///
/// This is the write-direction counterpart to `sdkt decode`. Supported types
/// are the primitives this CLI already encodes elsewhere (`parse_typed_args`):
/// `u32`, `i32`, `u64`, `i64`, `timepoint`, `duration`, `u128`, `i128`, `bool`, `address`, `string`,
/// `symbol`, `bytes`. `json:<JSON>` encodes one composite value through
/// `sdkt_xdr::json_to_scval`. Exactly one value is encoded per invocation;
/// passing more than one is rejected to keep the output unambiguous.
fn run_encode(values: &[String]) -> Result<String, String> {
    if values.is_empty() {
        return Err("no input provided: pass a value like u32:100".to_string());
    }
    if values.len() > 1 {
        return Err(format!(
            "expected exactly one value, got {} — encode one value per invocation",
            values.len()
        ));
    }

    let arg = &values[0];
    let (ty, raw) = arg.split_once(':').ok_or_else(|| {
        format!("invalid arg format '{arg}'. Use TYPE:VALUE (e.g. u32:100, address:G...)")
    })?;

    use sdkt_xdr::{scval_to_base64, Address, IntoScVal};
    use stellar_xdr::{Duration, ScSymbol, ScVal, TimePoint};
    let scval = match ty.to_lowercase().as_str() {
        "u32" => raw
            .parse::<u32>()
            .map_err(|_| format!("invalid u32 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "i32" => raw
            .parse::<i32>()
            .map_err(|_| format!("invalid i32 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "u64" => raw
            .parse::<u64>()
            .map_err(|_| format!("invalid u64 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "timepoint" => TimePoint(
            raw.parse::<u64>()
                .map_err(|_| format!("invalid timepoint value: {raw}"))?,
        )
        .into_scval()
        .map_err(|e| e.to_string())?,
        "duration" => Duration(
            raw.parse::<u64>()
                .map_err(|_| format!("invalid duration value: {raw}"))?,
        )
        .into_scval()
        .map_err(|e| e.to_string())?,
        "i64" => raw
            .parse::<i64>()
            .map_err(|_| format!("invalid i64 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "u128" => raw
            .parse::<u128>()
            .map_err(|_| format!("invalid u128 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "i128" => raw
            .parse::<i128>()
            .map_err(|_| format!("invalid i128 value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "bool" => raw
            .parse::<bool>()
            .map_err(|_| format!("invalid bool value: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "string" => raw.to_string().into_scval().map_err(|e| e.to_string())?,
        "bytes" => parse_hex_bytes(raw)?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "symbol" => {
            if raw.len() > 32 {
                return Err(format!("symbol exceeds 32 bytes (got {} bytes)", raw.len()));
            }
            if !raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("invalid symbol value: use only ASCII letters, digits, and _".into());
            }
            ScVal::Symbol(ScSymbol::try_from(raw).map_err(|_| "invalid symbol value".to_string())?)
        }
        "address" => Address::from_strkey(raw)
            .map_err(|_| format!("invalid Stellar address: {raw}"))?
            .into_scval()
            .map_err(|e| e.to_string())?,
        "json" => {
            let value: serde_json::Value = serde_json::from_str(raw)
                .map_err(|e| format!("invalid JSON in '{arg}': {e}"))?;
            sdkt_xdr::json_to_scval(&value).map_err(|e| format!("cannot encode '{arg}': {e}"))?
        }
        other => {
            return Err(format!(
                "unknown type '{other}'. Use u32|i32|u64|i64|timepoint|duration|u128|i128|bool|string|symbol|bytes|address"
            ))
        }
    };

    scval_to_base64(&scval).map_err(|e| e.to_string())
}

#[cfg(test)]
mod encode_tests {
    use super::{parse_typed_args, run_encode};

    #[test]
    fn new_types_match_runtime_typed_arguments() {
        for input in [
            "u128:0",
            "u128:+42",
            "u128:18446744073709551617",
            "u128:340282366920938463463374607431768211455",
            "timepoint:0",
            "timepoint:1758000000",
            "timepoint:18446744073709551615",
            "duration:0",
            "duration:86400",
            "duration:18446744073709551615",
            "U128:1000000",
            "i128:0",
            "i128:-0",
            "i128:+42",
            "i128:-1000",
            "i128:18446744073709551617",
            "i128:-18446744073709551617",
            "i128:170141183460469231731687303715884105727",
            "i128:-170141183460469231731687303715884105728",
            "I128:-1",
            "bytes:000aFF",
            "ByTeS:DeAdBeEf",
            "bytes:",
            "bytes: \t\n",
            "bytes:\u{2003}000aFF\u{2003}",
            "bytes:\u{2003}",
            // The runtime parser accepts a leading plus in each radix pair.
            "bytes:+f",
            "bytes:0a+F",
        ] {
            let args = [input.to_string()];
            let encoded = run_encode(&args).unwrap_or_else(|e| panic!("{input}: {e}"));
            for strict in [false, true] {
                let runtime =
                    parse_typed_args(&args, strict).unwrap_or_else(|e| panic!("{input}: {e}"));
                assert_eq!(encoded, runtime[0], "{input} (strict={strict})");
            }
        }
    }

    #[test]
    fn parse_typed_args_rejects_non_ascii_bytes_without_panic() {
        // "a€" has an even byte length (1 + 3), so only the ASCII guard stops
        // the pair slicing from cutting into the '€'.
        for value in ["💥", "a€"] {
            let input = [format!("bytes:{value}")];
            let expected = format!("invalid bytes value: {value} (expected ASCII hex)");
            for strict in [false, true] {
                let err = parse_typed_args(&input, strict).unwrap_err();
                assert_eq!(err, expected, "strict={strict}");
            }
            assert_eq!(run_encode(&input).unwrap_err(), expected);
        }
    }

    #[test]
    fn parse_typed_args_bytes_errors_match_encode() {
        for input in ["bytes:abc", "bytes:zz", "bytes: 0g "] {
            let args = [input.to_string()];
            let encode_err = run_encode(&args).unwrap_err();
            for strict in [false, true] {
                assert_eq!(parse_typed_args(&args, strict).unwrap_err(), encode_err);
            }
        }
    }

    /// Encode a `json:` input and decode the result back to an `ScVal`.
    fn encode_json(json: &str) -> stellar_xdr::ScVal {
        let b64 = run_encode(&[format!("json:{json}")]).unwrap_or_else(|e| panic!("{json}: {e}"));
        sdkt_xdr::scval_from_base64(&b64).expect("valid ScVal XDR")
    }

    fn string(s: &str) -> stellar_xdr::ScVal {
        use sdkt_xdr::IntoScVal;
        s.to_string().into_scval().expect("short string")
    }

    #[test]
    fn json_scalars_follow_converter_mapping() {
        use stellar_xdr::ScVal;
        assert_eq!(encode_json("null"), ScVal::Void);
        assert_eq!(encode_json("true"), ScVal::Bool(true));
        assert_eq!(encode_json("false"), ScVal::Bool(false));
        assert_eq!(encode_json("1"), ScVal::U32(1));
        assert_eq!(encode_json("-5"), ScVal::I32(-5));
        assert_eq!(encode_json("4294967296"), ScVal::U64(4_294_967_296));
        assert_eq!(encode_json("18446744073709551615"), ScVal::U64(u64::MAX));
        assert_eq!(encode_json("-2147483649"), ScVal::I64(-2_147_483_649));
        // An integer-valued JSON number matches the explicit scalar form byte for byte.
        assert_eq!(
            run_encode(&["json:-5".to_string()]),
            run_encode(&["i32:-5".to_string()])
        );
    }

    #[test]
    fn json_containers_encode_as_one_value() {
        use stellar_xdr::ScVal;
        let ScVal::Vec(Some(items)) = encode_json("[1,2,3]") else {
            panic!("expected a Vec");
        };
        assert_eq!(
            items.0.to_vec(),
            vec![ScVal::U32(1), ScVal::U32(2), ScVal::U32(3)]
        );
        assert!(matches!(encode_json("[]"), ScVal::Vec(Some(v)) if v.0.is_empty()));
        assert!(matches!(encode_json("{}"), ScVal::Map(Some(m)) if m.0.is_empty()));

        let ScVal::Map(Some(map)) = encode_json(r#"{"bob":"250","alice":"100"}"#) else {
            panic!("expected a Map");
        };
        // Keys are Strings, emitted in sorted order as Soroban maps require.
        let keys: Vec<_> = map.0.iter().map(|e| e.key.clone()).collect();
        assert_eq!(keys, vec![string("alice"), string("bob")]);
        assert_eq!(map.0[0].val, string("100"));
    }

    #[test]
    fn json_nesting_is_preserved() {
        use stellar_xdr::ScVal;
        let ScVal::Vec(Some(outer)) = encode_json(r#"[{"alice":"100"},[null,true]]"#) else {
            panic!("expected a Vec");
        };
        assert!(matches!(&outer.0[0], ScVal::Map(Some(m)) if m.0.len() == 1));
        let ScVal::Vec(Some(inner)) = &outer.0[1] else {
            panic!("expected a nested Vec");
        };
        assert_eq!(inner.0.to_vec(), vec![ScVal::Void, ScVal::Bool(true)]);
    }

    #[test]
    fn json_errors_name_the_input_without_panicking() {
        for input in ["json:[1,2", "json:{alice:1}", "json:", "json:[1] trailing"] {
            let err = run_encode(&[input.to_string()]).unwrap_err();
            assert!(
                err.starts_with(&format!("invalid JSON in '{input}': ")),
                "{input}: {err}"
            );
        }
        // Valid JSON the converter cannot represent (floats) is also reported per input.
        let err = run_encode(&["json:1.5".to_string()]).unwrap_err();
        assert_eq!(err, "cannot encode 'json:1.5': invalid JSON argument: 1.5");
    }

    #[test]
    fn timepoint_and_duration_reject_invalid_u64_values_consistently() {
        for input in [
            "timepoint:abc",
            "timepoint:-1",
            "timepoint:18446744073709551616",
            "duration:abc",
            "duration:-1",
            "duration:18446744073709551616",
        ] {
            let args = [input.to_string()];
            let encode_error = run_encode(&args).unwrap_err();
            for strict in [false, true] {
                assert_eq!(parse_typed_args(&args, strict).unwrap_err(), encode_error);
            }
        }
    }
}

fn load_config() -> DevKitConfig {
    match DevKitConfig::from_file(".sdkt.toml") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading .sdkt.toml: {}", e);
            process::exit(1);
        }
    }
}

/// Resolve a transaction envelope argument (`--input` / `--envelope`) to
/// envelope text.
///
/// Filesystem rules shared by `tx sign`, `tx validate`, `tx simulate` and
/// `tx submit`:
/// - If the path exists, read it as a file.
/// - If it does not exist but looks like a (missing) path, report a clear
///   "invalid file" error instead of silently mis-parsing it as base64.
/// - Otherwise treat the value as an inline base64 string.
///
/// `/` is part of the standard base64 alphabet, so a value that is
/// well-formed base64 is never treated as a path, even if it contains `/`.
fn resolve_tx_input(input: &str) -> Result<String, String> {
    if fs::metadata(input).is_ok() {
        return fs::read_to_string(input)
            .map_err(|e| format!("invalid file '{}': cannot read ({})", input, e));
    }
    let looks_like_path = input.contains('/')
        || input.contains('\\')
        || input.ends_with(".xdr")
        || input.ends_with(".txt");
    if looks_like_path && !is_standard_base64(input.trim()) {
        return Err(format!(
            "invalid file '{}': no such file or directory",
            input
        ));
    }
    Ok(input.to_string())
}

/// Whether `s` is padded standard base64 (`A-Z a-z 0-9 + /`, length a multiple
/// of four, at most two trailing `=`).
fn is_standard_base64(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    !s.is_empty()
        && s.len().is_multiple_of(4)
        && s.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

#[cfg(test)]
mod tx_input_tests {
    use super::{is_standard_base64, resolve_tx_input};

    #[test]
    fn base64_check_accepts_padded_standard_alphabet() {
        assert!(is_standard_base64("AAAA/+8="));
        assert!(is_standard_base64("AA=="));
        assert!(!is_standard_base64(""));
        assert!(!is_standard_base64("AAA"));
        assert!(!is_standard_base64("A==="));
        assert!(!is_standard_base64("tx/unsigned.xdr"));
        assert!(!is_standard_base64("AA=A"));
    }

    #[test]
    fn missing_path_is_reported_but_slashed_base64_passes_through() {
        let err = resolve_tx_input("no/such/tx.xdr").unwrap_err();
        assert_eq!(
            err,
            "invalid file 'no/such/tx.xdr': no such file or directory"
        );
        assert_eq!(resolve_tx_input("AAAA/+8=").unwrap(), "AAAA/+8=");
        assert_eq!(resolve_tx_input("not-base64").unwrap(), "not-base64");
    }
}

/// Render a `ContractFunction`'s signature as `name(params) -> outputs`.
fn sig_string(f: &sdkt_wasm::ContractFunction) -> String {
    let params: Vec<String> = f
        .parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, p.type_.name))
        .collect();
    let outs: Vec<String> = f.outputs.iter().map(|o| o.name.clone()).collect();
    let out = if outs.is_empty() {
        "void".to_string()
    } else {
        outs.join(", ")
    };
    format!("{}({}) -> {}", f.name, params.join(", "), out)
}

/// Pretty-print an upgrade-safety verdict (used by `sdkt diff --upgrade-safety`).
fn print_upgrade_verdict(v: &sdkt_wasm::UpgradeVerdict) {
    println!("Upgrade Safety");
    println!("==============");
    println!();
    println!("Compatible: {}", if v.compatible { "YES" } else { "NO" });
    println!();
    println!("Breaking:");
    print_verdict_changes(&v.breaking_changes);
    println!();
    println!("Non-breaking:");
    print_verdict_changes(&v.non_breaking_changes);
}

/// Print one verdict change per line, indented with its old/new shape when the
/// engine recorded a detail (a signature, event, or type-definition change).
fn print_verdict_changes(changes: &[sdkt_wasm::VerdictChange]) {
    if changes.is_empty() {
        println!("  (none)");
        return;
    }
    for c in changes {
        println!("  - {}", c.label());
        for line in c.detail.lines() {
            println!("    {}", line.trim_start());
        }
    }
}

/// — On-chain upgrade-safety verification.
///
/// Fetches the deployed contract's on-chain WASM via the path
/// (`inspect_contract` -> `get_wasm_bytecode`), parses its `ContractSpec`, and
/// runs the existing `SpecDiff`/`UpgradeVerdict` engine against a local
/// candidate WASM. No new RPC method, no new parser, no new verdict engine — it
/// reuses `sdkt-rpc` retrieval and `sdkt-wasm` diffing verbatim. Read-only; the
/// network/mainnet-safety guard is inherited from `resolve_rpc_client` in the
/// caller.
/// Minimal extraction: run the existing upgrade-safety engine
/// (`inspect_contract` → `get_wasm_bytecode` → `diff_wasm` →
/// `UpgradeVerdict::from_diff`) against the *deployed* contract.
///
/// Shared verbatim by `sdkt verify --upgrade-safety` and the release-assurance
/// composition so neither duplicates the comparison. Read-only.
async fn upgrade_assurance_against_deployed(
    client: &SorobanRpcClient,
    contract_id: &str,
    candidate_bytes: &[u8],
    network: &str,
) -> Result<(sdkt_wasm::SpecDiff, sdkt_wasm::UpgradeVerdict, String), String> {
    // Candidate WASM is parsed offline first (fail-fast on malformed input).
    sdkt_wasm::parse_metadata(candidate_bytes)
        .map_err(|e| format!("candidate is not valid WASM: {}", e))?;

    let inspection = inspect_contract(client, contract_id)
        .await
        .map_err(|e| match e {
            sdkt_rpc::RpcError::ContractNotFound => {
                format!("contract {} not found on {}", contract_id, network)
            }
            other => format!("{}", other),
        })?;
    let wasm_hash = inspection.wasm_hash;

    let deployed_bytes = get_wasm_bytecode(client, &wasm_hash)
        .await
        .map_err(|e| format!("could not fetch on-chain WASM for {}: {}", contract_id, e))?;

    // "old" side is the deployed contract; "new" side is the local candidate.
    let diff = sdkt_wasm::diff_wasm(&deployed_bytes, candidate_bytes)
        .map_err(|e| format!("failed to diff contracts: {}", e))?;
    let verdict = sdkt_wasm::UpgradeVerdict::from_diff(&diff);

    Ok((diff, verdict, wasm_hash))
}

async fn run_upgrade_safety(
    client: &SorobanRpcClient,
    contract_id: &str,
    candidate_bytes: &[u8],
    network: &str,
    fmt: OutputFormat,
) -> Result<(), String> {
    let (_diff, verdict, wasm_hash) =
        upgrade_assurance_against_deployed(client, contract_id, candidate_bytes, network).await?;

    if fmt == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&verdict).map_err(|e| format!("{}", e))?
        );
    } else {
        print_upgrade_verdict(&verdict);
        println!();
        println!(
            "Baseline: live contract {} on {} (WASM {})",
            contract_id, network, wasm_hash
        );
    }
    Ok(())
}

fn collect_rust_sources(path: &std::path::Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    if path.is_file() {
        // Preserve the single-file audit behavior for explicitly supplied source
        // files, including temporary files without a `.rs` extension. Directory
        // discovery below remains restricted to Rust source files.
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("path does not exist: {}", path.display()),
        ));
    }

    let mut files = Vec::new();
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let entry_path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                stack.push(entry_path);
            } else if file_type.is_file() && entry_path.extension().is_some_and(|ext| ext == "rs") {
                files.push(entry_path);
            }
        }
    }

    files.sort();
    Ok(files)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Windows debug builds overflow the default 1 MB main-thread stack
    // (introduced by the invoke command's deeper async call chain). Unix
    // platforms default to 8 MB, so only Windows users hit this. Run the
    // runtime on an explicitly sized thread so all platforms use the same
    // effective stack size. The runtime is created inside the spawned thread
    // so the CLI's async execution actually occurs on the larger stack.
    let result = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build Tokio runtime");

            // The boxed error is not `Send`, so string-ify it inside the
            // thread and rebuild a `Send` error on this side.
            match rt.block_on(async_main()) {
                Ok(()) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })?
        .join()
        .map_err(|_| "main thread panicked".to_string())?;

    result.map_err(|e| -> Box<dyn std::error::Error> { e.into() })
}

// — `sdkt doctor`: baseline environment/project diagnostics.
//
// Core scope only: runtime, toolchain, project/config validity, and clear
// handling of non-project directories. Network/identity/plugin checks are
// intentionally out of scope for the doctor core.

/// Severity of a single diagnostic check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Ok,
    Warning,
    Error,
}

/// One diagnostic check result: stable ID, status, message, optional fix hint.
#[derive(Debug, Clone, serde::Serialize)]
struct DoctorCheck {
    /// Stable machine-readable identifier (e.g. `runtime-version`).
    id: &'static str,
    status: CheckStatus,
    /// Human-readable one-line summary. Never contains secret material.
    message: String,
    /// Optional remediation hint shown only when status is not Ok.
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<String>,
}

impl DoctorCheck {
    fn ok(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Ok,
            message: message.into(),
            remediation: None,
        }
    }
    fn warn(id: &'static str, message: impl Into<String>, remediation: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Warning,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }
    fn err(id: &'static str, message: impl Into<String>, remediation: impl Into<String>) -> Self {
        Self {
            id,
            status: CheckStatus::Error,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }
}

/// Aggregate doctor report.
#[derive(Debug, serde::Serialize)]
struct DoctorReport {
    checks: Vec<DoctorCheck>,
    /// true when no check has status Error (warnings allowed).
    healthy: bool,
}

impl DoctorReport {
    fn from_checks(checks: Vec<DoctorCheck>) -> Self {
        let healthy = checks.iter().all(|c| c.status != CheckStatus::Error);
        Self { checks, healthy }
    }
}

/// Pure executable lookup: true when `program` — or `program.exe`, the
/// Windows naming — exists as a file in any PATH entry. Checking the `.exe`
/// form on every platform is harmless (those files never exist as bare names
/// elsewhere) and keeps discovery correct on Windows without cfg hacks.
fn path_contains_command(path_var: &std::ffi::OsStr, program: &str) -> bool {
    std::env::split_paths(path_var)
        .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
}

/// Look for a runnable program on PATH (name only; no output capture).
fn command_on_path(program: &str) -> bool {
    path_contains_command(&std::env::var_os("PATH").unwrap_or_default(), program)
}

/// Read a raw 32-byte Ed25519 public key file, exiting with a clear error on failure.
fn read_public_key_or_exit(
    key_path: &str,
) -> sdkt_audit::plugin_store::ed25519_dalek::VerifyingKey {
    let bytes = std::fs::read(key_path).unwrap_or_else(|e| {
        eprintln!("Error reading public key: {}", e);
        process::exit(1);
    });
    let arr: [u8; 32] = bytes.try_into().unwrap_or_else(|_| {
        eprintln!("Error: public key must be exactly 32 bytes");
        process::exit(1);
    });
    sdkt_audit::plugin_store::ed25519_dalek::VerifyingKey::from_bytes(&arr).unwrap_or_else(|_| {
        eprintln!("Error: invalid Ed25519 public key");
        process::exit(1);
    })
}

/// True when `installed` (the line-separated output of
/// `rustup target list --installed`) contains a WASM target that `sdkt build`
/// can compile contracts with. The build engine hardcodes
/// `wasm32-unknown-unknown` (see `sdkt_core::build`); `wasm32v1-none` is the
/// modern equivalent used by current soroban-sdk toolchains. `wasm32-wasip1`
/// is a plugin/playground target, NOT a contract build target, so it must not
/// count.
fn wasm_target_present(installed: &str) -> bool {
    installed.lines().any(|l| {
        let t = l.trim();
        t == "wasm32-unknown-unknown" || t == "wasm32v1-none"
    })
}

/// Detect the Rust WASM build target via `rustup target list --installed`
/// (offline, no compilation). Returns None when rustup cannot be executed.
fn wasm_target_installed() -> Option<bool> {
    let output = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()?;
    Some(wasm_target_present(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Collect baseline diagnostic checks. Purely offline and side-effect free.
fn collect_doctor_checks() -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    // 1. sdkt runtime availability (running means installed; report version).
    checks.push(DoctorCheck::ok(
        "sdkt-runtime",
        format!("sdkt {} is running", env!("CARGO_PKG_VERSION")),
    ));

    // 2. Rust/Cargo availability — required for `sdkt build` / source installs.
    let cargo = command_on_path("cargo");
    let rustc = command_on_path("rustc");
    if cargo && rustc {
        checks.push(DoctorCheck::ok(
            "rust-toolchain",
            "cargo and rustc found on PATH",
        ));
    } else {
        let missing = match (cargo, rustc) {
            (false, false) => "cargo and rustc are both missing".to_string(),
            (false, _) => "cargo is missing".to_string(),
            (_, false) => "rustc is missing".to_string(),
            _ => unreachable!(),
        };
        checks.push(DoctorCheck::warn(
            "rust-toolchain",
            format!("{missing} — `sdkt build` and source installs will not work"),
            "Install Rust via rustup: https://rustup.rs (prebuilt-release users can ignore this warning)",
        ));
    }

    // 3. WASM build target availability — required for `sdkt build`.
    if !cargo {
        checks.push(DoctorCheck::warn(
            "wasm-target",
            "skipped: cargo/rustup not available",
            "Install Rust first; then run `rustup target add wasm32-unknown-unknown`",
        ));
    } else {
        match wasm_target_installed() {
            Some(true) => {
                checks.push(DoctorCheck::ok(
                    "wasm-target",
                    "a WASM build target is installed",
                ));
            }
            Some(false) => {
                checks.push(DoctorCheck::err(
                    "wasm-target",
                    "no WASM build target installed — `sdkt build` will fail",
                    "Run: rustup target add wasm32-unknown-unknown",
                ));
            }
            None => {
                checks.push(DoctorCheck::warn(
                    "wasm-target",
                    "could not verify WASM target (rustup not available or not managing the toolchain)",
                    "Verify with: rustup target list --installed",
                ));
            }
        }
    }

    // 4. Project/config validity — only when a `.sdkt.toml` is present.
    let config_path = std::path::Path::new(".sdkt.toml");
    if !config_path.exists() {
        checks.push(DoctorCheck::warn(
            "project-config",
            "no .sdkt.toml in the current directory (not inside an sdkt project)",
            "Run `sdkt init <name>` to create a project, or cd into one",
        ));
    } else {
        match DevKitConfig::from_file(config_path) {
            Ok(config) => {
                // Reuse the shared project graph resolver as the validity oracle.
                match sdkt_core::project::validate_project(&config) {
                    Ok(()) => {
                        checks.push(DoctorCheck::ok(
                            "project-config",
                            format!(
                                ".sdkt.toml is valid ({} contract(s) configured)",
                                config.contracts.len()
                            ),
                        ));
                    }
                    Err(e) => {
                        checks.push(DoctorCheck::err(
                            "project-config",
                            format!(".sdkt.toml is invalid: {e}"),
                            "Fix the [contracts] dependency graph errors reported above",
                        ));
                    }
                }
            }
            Err(e) => {
                checks.push(DoctorCheck::err(
                    "project-config",
                    format!(".sdkt.toml failed to parse: {e}"),
                    "Fix the TOML syntax errors reported above",
                ));
            }
        }
    }

    checks
}

/// Pretty-print the doctor report (human-readable output).
fn print_doctor_pretty(report: &DoctorReport) {
    println!("sdkt doctor");
    println!("===========");
    for check in &report.checks {
        let icon = match check.status {
            CheckStatus::Ok => "✓",
            CheckStatus::Warning => "!",
            CheckStatus::Error => "✗",
        };
        println!("  [{icon}] {}: {}", check.id, check.message);
        if let Some(rem) = &check.remediation {
            println!("        fix: {rem}");
        }
    }
    println!();
    if report.healthy {
        println!("Result: HEALTHY (no errors; warnings are non-fatal)");
    } else {
        println!("Result: UNHEALTHY (one or more checks failed)");
    }
}

/// Execute `sdkt doctor`. Exit code: 0 healthy/warnings, 1 any error.
fn run_doctor(fmt: OutputFormat) {
    let report = DoctorReport::from_checks(collect_doctor_checks());
    match fmt {
        OutputFormat::Json => {
            // Machine-readable; contains no secret material by construction —
            // messages are built from fixed strings + version/counts only.
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("doctor report serializes")
            );
        }
        OutputFormat::Pretty => print_doctor_pretty(&report),
    }
    if !report.healthy {
        process::exit(1);
    }
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Decode {
            payload,
            r#type,
            format,
            file,
        } => {
            let input = match (file, payload) {
                (Some(path), _) => fs::read_to_string(&path)?,
                (None, Some(p)) => p,
                (None, None) => {
                    return Err("no input provided: pass XDR as an argument or use --file".into());
                }
            };

            let fmt = parse_format_str(&format);
            let json = decode(&input, r#type.as_deref(), fmt)?;
            println!("{}", json);
        }
        Commands::Encode { values } => match run_encode(&values) {
            Ok(b64) => println!("{}", b64),
            Err(e) => {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        },
        Commands::Storage {
            action,
            abi,
            abi_contract,
            net,
        } => {
            // Storage estimate is completely offline and self-contained; dispatch
            // before shared storage RPC client setup or on-chain ABI resolution.
            if let StorageAction::Estimate {
                wasm,
                ledgers,
                format,
            } = &action
            {
                if abi.is_some() || abi_contract.is_some() {
                    eprintln!(
                        "Error: --abi and --abi-contract options do not apply to 'storage estimate'"
                    );
                    process::exit(1);
                }

                let fmt = parse_format_str(format);
                let wasm_bytes = match fs::read(wasm) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("Error: cannot read WASM '{}': {}", wasm, e);
                        process::exit(1);
                    }
                };

                let spec = match parse_contract_spec(&wasm_bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}: {}", wasm, e);
                        process::exit(1);
                    }
                };

                let estimate = sdkt_storage::estimate_storage_from_spec(&spec, wasm, *ledgers);

                match fmt {
                    OutputFormat::Json => {
                        let json_str = serde_json::to_string_pretty(&estimate)
                            .expect("storage cost estimate serializes");
                        println!("{}", json_str);
                    }
                    _ => {
                        print!("{}", estimate);
                    }
                }
                return Ok(());
            }

            // Storage storage-diff is completely offline and self-contained; dispatch
            // before shared storage RPC client setup or on-chain ABI resolution.
            if let StorageAction::StorageDiff {
                old,
                new,
                extend_plan,
                format,
            } = &action
            {
                if let Err(e) =
                    commands::abi::check_abi_mutual_exclusion(abi.as_ref(), abi_contract.as_ref())
                {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }

                let fmt = parse_format_str(format);
                // Resolve the RPC client lazily: only `--abi-contract` needs the
                // network. The local `--abi` and no-ABI paths are fully offline
                // so an unavailable or unconfigured network profile must not
                // cause the command to fail before either snapshot is read.
                let contract_spec = if let Some(id) = abi_contract.as_ref() {
                    let diff_client = resolve_rpc_client(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    );
                    commands::abi::resolve_abi_spec(None, Some(id), &diff_client).await?
                } else if let Some(wasm_path) = abi.as_ref() {
                    Some(commands::abi::load_local_abi(wasm_path)?)
                } else {
                    None
                };

                // Load old snapshot JSON (output of `sdkt storage analyze --format json`).
                let old_bytes = match fs::read(old) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("Failed to read --old snapshot '{old}': {e}");
                        process::exit(1);
                    }
                };
                let old_report: sdkt_storage::StorageReport =
                    match serde_json::from_slice(&old_bytes) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Failed to parse --old snapshot '{old}': {e}");
                            process::exit(1);
                        }
                    };

                // Load new snapshot JSON.
                let new_bytes = match fs::read(new) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("Failed to read --new snapshot '{new}': {e}");
                        process::exit(1);
                    }
                };
                let new_report: sdkt_storage::StorageReport =
                    match serde_json::from_slice(&new_bytes) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Failed to parse --new snapshot '{new}': {e}");
                            process::exit(1);
                        }
                    };

                let mut old_snap = match sdkt_storage::StorageSnapshot::from_report(&old_report) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Failed to load --old snapshot '{old}': {e}");
                        process::exit(1);
                    }
                };
                let mut new_snap = match sdkt_storage::StorageSnapshot::from_report(&new_report) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Failed to load --new snapshot '{new}': {e}");
                        process::exit(1);
                    }
                };
                if let Some(spec) = contract_spec.as_ref() {
                    for entry in old_snap
                        .entries
                        .iter_mut()
                        .chain(new_snap.entries.iter_mut())
                    {
                        entry.label = Some(storage_key_label(&entry.key, spec));
                    }
                }
                let diff = match sdkt_storage::diff_snapshots(&old_snap, &new_snap) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    }
                };

                if *extend_plan {
                    let plan = sdkt_storage::derive_extend_plan(&diff);
                    if fmt == OutputFormat::Json {
                        let out = serde_json::json!({
                            "diff": &diff,
                            "extend_plan": &plan,
                        });
                        println!("{}", serde_json::to_string(&out)?);
                    } else {
                        print_diff_pretty(&diff);
                        println!();
                        println!("Extension Plan");
                        println!("  Contract:            {}", plan.contract_id);
                        if plan.keys.is_empty() {
                            println!("  Keys:                (none — nothing to remediate)");
                        } else {
                            println!("  Keys ({}):", plan.keys.len());
                            for (i, k) in plan.keys.iter().enumerate() {
                                println!("    #{} {}", i + 1, k);
                            }
                        }
                        println!("  Suggested --ledgers: {}", plan.suggested_ledgers);
                        println!("  Reason:              {}", plan.suggested_ledgers_reason);
                        if !plan.keys.is_empty() {
                            println!();
                            println!("  Ready-to-run:");
                            let mut net_args = String::new();
                            if let Some(ref p) = net.network_profile {
                                net_args
                                    .push_str(&format!(" --network-profile {}", shell_quote(p)));
                            }
                            if let Some(ref u) = net.rpc_url {
                                net_args.push_str(&format!(" --rpc-url {}", shell_quote(u)));
                            }
                            if let Some(ref pass) = net.network_passphrase {
                                net_args.push_str(&format!(
                                    " --network-passphrase {}",
                                    shell_quote(pass)
                                ));
                            }

                            let key_args: String = plan
                                .keys
                                .iter()
                                .map(|k| format!(" --key {}", shell_quote(k)))
                                .collect::<Vec<_>>()
                                .join("");
                            println!(
                                "    sdkt storage extend --contract {} --ledgers {}{}{}",
                                shell_quote(&plan.contract_id),
                                plan.suggested_ledgers,
                                net_args,
                                key_args
                            );
                        }
                    }
                } else if fmt == OutputFormat::Json {
                    println!("{}", serde_json::to_string(&diff)?);
                } else {
                    print_diff_pretty(&diff);
                }
                return Ok(());
            }

            if let Err(e) =
                commands::abi::check_abi_mutual_exclusion(abi.as_ref(), abi_contract.as_ref())
            {
                eprintln!("Error: {e}");
                process::exit(1);
            }

            let analyze_extra_keys = if let StorageAction::Analyze {
                contract_id,
                key_xdr,
                map_key,
                key_arg,
                durability,
                ..
            } = &action
            {
                match resolve_storage_analyze_keys(
                    contract_id,
                    key_xdr,
                    map_key.as_deref(),
                    key_arg,
                    durability,
                ) {
                    Ok(k) => Some(k),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    }
                }
            } else {
                None
            };

            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

            // Load ABI spec if provided. Two mutually exclusive sources:
            // a local WASM file (`--abi`) or a deployed contract's on-chain WASM
            // fetched via the path (`--abi-contract`).
            let contract_spec: Option<sdkt_wasm::ContractSpec> =
                commands::abi::resolve_abi_spec(abi.as_ref(), abi_contract.as_ref(), &client)
                    .await?;

            match action {
                StorageAction::Check {
                    contract_id,
                    format,
                } => {
                    let fmt = parse_format_str(&format);
                    let client = resolve_rpc_client(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    );

                    // Load ABI spec if provided for storage decoding
                    match get_ttl_info(&client, &contract_id).await {
                        Ok(ttl_info) => {
                            if fmt == OutputFormat::Json {
                                let mut output = serde_json::to_value(&ttl_info)?;
                                if let Some(spec) = contract_spec.as_ref() {
                                    output["abi"] = serde_json::json!({
                                            "functions": spec.functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                                            "events": spec.events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                                            "custom_types": spec.custom_types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
                                    });
                                }
                                println!("{}", serde_json::to_string(&output)?);
                            } else {
                                println!("Storage Check for Contract ID: {}", contract_id);
                                println!("Total Entries: {}", ttl_info.entries.len());
                                for (i, entry) in ttl_info.entries.iter().enumerate() {
                                    println!("\nEntry #{}", i + 1);
                                    println!("  Key: {}", entry.key);
                                    println!("  Current TTL: {} ledgers", entry.current_ttl);
                                    println!("  Remaining: {}", entry.expiration_time);
                                    println!(
                                        "  Est. Extension Cost: {} stroops",
                                        entry.extension_cost_stroops
                                    );
                                }

                                if let Some(spec) = contract_spec {
                                    println!("\nABI Functions:");
                                    for f in &spec.functions {
                                        println!("  - {} ({})", f.name, f.doc);
                                    }
                                    println!("\nABI Events:");
                                    for e in &spec.events {
                                        println!("  - {}", e.name);
                                    }
                                    if !spec.custom_types.is_empty() {
                                        println!("\nABI Custom Types:");
                                        for t in &spec.custom_types {
                                            println!("  - {} ({})", t.name, t.kind);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error fetching storage TTL: {}", e);
                            process::exit(1);
                        }
                    }
                }
                StorageAction::Estimate { .. } => unreachable!(),
                StorageAction::Analyze {
                    contract_id,
                    format,
                    ..
                } => {
                    let fmt = parse_format_str(&format);
                    let extra_keys = analyze_extra_keys.expect("resolved for Analyze");

                    let client = resolve_rpc_client(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    );
                    let analyzer = sdkt_storage::StorageAnalyzer::new(client);

                    match analyzer
                        .inspect_contract_storage_keys(&contract_id, &extra_keys)
                        .await
                    {
                        Ok(mut report) => {
                            if let Some(spec) = contract_spec.as_ref() {
                                for entry in &mut report.entries {
                                    entry.label = Some(storage_key_label(&entry.key, spec));
                                }
                            }
                            if fmt == OutputFormat::Json {
                                println!("{}", serde_json::to_string(&report)?);
                            } else {
                                println!("Storage Analysis for Contract: {}", report.contract_id);
                                println!("Total Entries: {}", report.total_entries);
                                println!("  Instance:    {}", report.instance_entries);
                                println!("  Persistent: {}", report.persistent_entries);
                                println!("  Temporary:   {}", report.temporary_entries);
                                if report.other_entries > 0 {
                                    println!("  Other:      {}", report.other_entries);
                                }
                                if let Some(summary) = &report.ttl_summary {
                                    println!("\nTTL Summary:");
                                    println!("  Min TTL:        {}", summary.minimum_ttl);
                                    println!("  Max TTL:        {}", summary.maximum_ttl);
                                    println!("  Average TTL:    {}", summary.average_ttl);
                                    println!(
                                        "  Expiring Soon:  {}",
                                        summary.expiring_entries_count
                                    );
                                    if let Some(cost) = summary.estimated_rent_cost {
                                        println!("  Est. Rent Cost: {} stroops", cost);
                                    }
                                }
                                if !report.entries.is_empty() {
                                    println!("\nEntries:");
                                    for (i, entry) in report.entries.iter().enumerate() {
                                        println!(
                                            "  #{:<3} [{}] ttl={} (~{}d) cost={} stroops",
                                            i + 1,
                                            entry.class.label(),
                                            entry.current_ttl,
                                            entry.days_remaining,
                                            entry.extension_cost_stroops
                                        );
                                        if let Some(label) = &entry.label {
                                            println!("        key={} label={}", entry.key, label);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error analyzing storage: {}", e);
                            process::exit(1);
                        }
                    }
                }
                StorageAction::Extend {
                    contract,
                    ledgers,
                    key,
                    identity,
                    format,
                } => {
                    let fmt = parse_format_str(&format);

                    if contract.trim().is_empty() {
                        eprintln!("Error: --contract / contract id must not be empty");
                        process::exit(1);
                    }
                    if ledgers == 0 {
                        eprintln!("Error: --ledgers must be greater than 0");
                        process::exit(1);
                    }
                    if let Err(e) = sdkt_rpc::collect_extend_keys(&contract, &key) {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }

                    let network_config = resolve_network_config(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    )?;
                    let network = match network_config.passphrase.as_str() {
                        "Test SDF Network ; September 2015" => sdkt_xdr::sign::Network::Testnet,
                        "Public Global Stellar Network ; September 2015" => {
                            sdkt_xdr::sign::Network::Mainnet
                        }
                        "Test SDF Future Network ; October 2022" => {
                            sdkt_xdr::sign::Network::Futurenet
                        }
                        other => sdkt_xdr::sign::Network::Custom(other.to_string()),
                    };
                    let network_is_explicit = net.rpc_url.is_some()
                        || net.network_passphrase.is_some()
                        || net.network_profile.is_some();
                    if let Err(e) =
                        sdkt_core::guard_mutating_network(&network_config, network_is_explicit)
                    {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }

                    let identity_store = sdkt_storage::IdentityStore::new()
                        .map_err(|e| format!("Failed to access identity store: {}", e))?;
                    // `--identity` defaults to the reserved "default" sentinel;
                    // resolve it to the configured default identity, if any.
                    let identity_obj = identity_store
                        .resolve_signing_identity(&identity)
                        .map_err(|e| format!("Failed to resolve signing identity: {}", e))?;
                    let signing_key = identity_store
                        .load_signing_key(&identity_obj.name)
                        .map_err(|e| {
                            format!(
                                "Failed to load signing key for '{}': {}",
                                identity_obj.name, e
                            )
                        })?;
                    let signer = sdkt_xdr::sign::Ed25519Signer::from_seed(&signing_key.to_bytes());
                    let source_account = identity_obj.public_key.clone();
                    let client = SorobanRpcClient::from_config(&network_config);

                    match extend_footprint(
                        &client,
                        &contract,
                        &key,
                        ledgers,
                        &source_account,
                        &signer,
                        network,
                    )
                    .await
                    {
                        Ok(res) => {
                            if fmt == OutputFormat::Json {
                                println!("{}", serde_json::to_string(&res)?);
                            } else {
                                println!("Storage TTL Extend");
                                println!("  Contract:       {}", res.contract_id);
                                println!("  Extend To:      {} (ledger)", res.extend_to);
                                println!("  Footprint Keys: {}", res.footprint_keys.len());
                                for (i, k) in res.footprint_keys.iter().enumerate() {
                                    println!("    #{} {}", i + 1, k);
                                }
                                println!("  TX Hash:        {}", res.hash);
                                println!("  Fee:            {} stroops", res.fee);
                                println!("  Status:         {}", res.status);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error extending storage TTL: {}", e);
                            process::exit(1);
                        }
                    }
                }
                StorageAction::Restore {
                    contract,
                    envelope,
                    identity,
                    dry_run,
                    format,
                } => {
                    let fmt = parse_format_str(&format);

                    if contract.trim().is_empty() {
                        eprintln!("Error: --contract / contract id must not be empty");
                        process::exit(1);
                    }
                    if let Err(e) = sdkt_xdr::decode_contract_id(contract.trim()) {
                        eprintln!("Error: invalid --contract '{}': {}", contract, e);
                        process::exit(1);
                    }
                    if envelope.trim().is_empty() {
                        eprintln!("Error: --envelope must not be empty");
                        process::exit(1);
                    }
                    if let Err(e) =
                        sdkt_xdr::decode(envelope.trim(), Some("TransactionEnvelope"), fmt)
                    {
                        eprintln!(
                            "Error: --envelope is not a base64 XDR TransactionEnvelope: {}",
                            e
                        );
                        process::exit(1);
                    }

                    let network_config = resolve_network_config(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    )?;
                    let network = match network_config.passphrase.as_str() {
                        "Test SDF Network ; September 2015" => sdkt_xdr::sign::Network::Testnet,
                        "Public Global Stellar Network ; September 2015" => {
                            sdkt_xdr::sign::Network::Mainnet
                        }
                        "Test SDF Future Network ; October 2022" => {
                            sdkt_xdr::sign::Network::Futurenet
                        }
                        other => sdkt_xdr::sign::Network::Custom(other.to_string()),
                    };
                    let network_is_explicit = net.rpc_url.is_some()
                        || net.network_passphrase.is_some()
                        || net.network_profile.is_some();
                    if let Err(e) =
                        sdkt_core::guard_mutating_network(&network_config, network_is_explicit)
                    {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }

                    let identity_store = sdkt_storage::IdentityStore::new()
                        .map_err(|e| format!("Failed to access identity store: {}", e))?;
                    // `--identity` defaults to the reserved "default" sentinel;
                    // resolve it to the configured default identity, if any.
                    let identity_obj = identity_store
                        .resolve_signing_identity(&identity)
                        .map_err(|e| format!("Failed to resolve signing identity: {}", e))?;
                    let signing_key = identity_store
                        .load_signing_key(&identity_obj.name)
                        .map_err(|e| {
                            format!(
                                "Failed to load signing key for '{}': {}",
                                identity_obj.name, e
                            )
                        })?;
                    let signer = sdkt_xdr::sign::Ed25519Signer::from_seed(&signing_key.to_bytes());
                    let source_account = identity_obj.public_key.clone();
                    let client = SorobanRpcClient::from_config(&network_config);

                    match restore_footprint(
                        &client,
                        contract.trim(),
                        envelope.trim(),
                        &source_account,
                        &signer,
                        network,
                        dry_run,
                    )
                    .await
                    {
                        Ok(res) => {
                            if fmt == OutputFormat::Json {
                                println!("{}", serde_json::to_string(&res)?);
                            } else {
                                if dry_run {
                                    println!("Storage Restore (dry run, not submitted)");
                                } else {
                                    println!("Storage Restore");
                                }
                                println!("  Contract:       {}", res.contract_id);
                                println!("  Restored Keys:  {}", res.restored_keys);
                                for (i, k) in res.footprint_keys.iter().enumerate() {
                                    println!("    #{} {}", i + 1, k);
                                }
                                println!(
                                    "  TX Hash:        {}",
                                    res.hash.as_deref().unwrap_or("(not submitted)")
                                );
                                println!(
                                    "  Fee:            {} stroops (min resource fee {})",
                                    res.fee, res.min_resource_fee
                                );
                                println!("  Status:         {}", res.status);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error restoring storage: {}", e);
                            process::exit(1);
                        }
                    }
                }
                StorageAction::Read {
                    contract,
                    key_xdr,
                    map_key,
                    key_arg,
                    instance,
                    durability,
                    format,
                } => {
                    let fmt = parse_format_str(&format);

                    if contract.trim().is_empty() {
                        eprintln!("Error: --contract must not be empty");
                        process::exit(1);
                    }

                    // Resolve the LedgerKey from exactly one source: the raw
                    // `--key-xdr` escape hatch, a typed `--map-key` spec, or
                    // `--instance`.
                    let key_xdr = match resolve_storage_read_key(
                        &contract,
                        key_xdr.as_deref(),
                        map_key.as_deref(),
                        &key_arg,
                        instance,
                        &durability,
                    ) {
                        Ok(k) => k,
                        Err(e) => {
                            eprintln!("Error: {e}");
                            process::exit(1);
                        }
                    };

                    let network_config = resolve_network_config(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    )?;
                    let client = SorobanRpcClient::from_config(&network_config);

                    let contract_spec: Option<sdkt_wasm::ContractSpec> =
                        commands::abi::resolve_abi_spec(
                            abi.as_ref(),
                            abi_contract.as_ref(),
                            &client,
                        )
                        .await?;

                    match read_contract_state(&client, &contract, &key_xdr, contract_spec.as_ref())
                        .await
                    {
                        Ok(res) => {
                            // TTL context is optional enrichment. If fetching
                            // the current ledger fails, keep the absolute TTL
                            // display and existing JSON shape.
                            let ttl_context = if let Some(live_until) = res.live_until_ledger {
                                client.get_ledger().await.ok().map(|ledger| {
                                    let remaining = live_until.saturating_sub(ledger.sequence);
                                    (remaining, remaining <= EXPIRING_SOON_LEDGERS)
                                })
                            } else {
                                None
                            };

                            if fmt == OutputFormat::Json {
                                let mut output = serde_json::to_value(&res)?;
                                if let Some((remaining, expiring_soon)) = ttl_context {
                                    output["ledgers_remaining"] = serde_json::json!(remaining);
                                    output["expiring_soon"] = serde_json::json!(expiring_soon);
                                }
                                println!("{}", serde_json::to_string(&output)?);
                            } else {
                                println!("Contract State Read");
                                println!("  Contract:       {}", res.contract_id);
                                println!("  Key:            {}", res.key);
                                println!("  Entry Type:     {}", res.entry_type);
                                if let Some(dur) = &res.durability {
                                    println!("  Durability:     {dur}");
                                }
                                if let Some(ttl) = res.live_until_ledger {
                                    if let Some((remaining, expiring_soon)) = ttl_context {
                                        let approx_days = remaining as f64 * 5.0 / 86_400.0;
                                        println!(
                                            "  Live Until:     {ttl} (ledger; {remaining} ledgers remaining, ~{approx_days:.2} days at 5s/ledger)"
                                        );
                                        if expiring_soon {
                                            println!(
                                                "  Caution:        Entry is expiring soon. Extend it with: sdkt storage extend --contract {} --key {} --ledgers {}",
                                                res.contract_id,
                                                res.key,
                                                DEFAULT_SUGGESTED_LEDGERS
                                            );
                                        }
                                    } else {
                                        println!("  Live Until:     {ttl} (ledger)");
                                    }
                                }
                                println!(
                                    "  Value:          {}",
                                    serde_json::to_string(&res.value)?
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading contract state: {e}");
                            process::exit(1);
                        }
                    }
                }
                StorageAction::StorageDiff { .. } => unreachable!(),
            }
        }
        Commands::Inspect {
            contract_id,
            format,
            interface,
            abi,
            abi_contract,
            net,
        } => {
            if abi.is_some() && abi_contract.is_some() {
                return Err("specify only one of --abi or --abi-contract".into());
            }
            let markdown = format.eq_ignore_ascii_case("markdown");
            let fmt = if markdown {
                OutputFormat::Pretty
            } else {
                parse_format_str(&format)
            };
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

            // Load ABI spec if provided for storage decoding
            let contract_spec = if let Some(wasm_path) = abi.as_ref() {
                let wasm_bytes =
                    fs::read(wasm_path).map_err(|e| format!("Failed to read WASM: {}", e))?;
                Some(
                    parse_contract_spec(&wasm_bytes)
                        .map_err(|e| format!("Failed to parse ABI: {}", e))?,
                )
            } else if let Some(id) = abi_contract.as_ref() {
                let inspection = inspect_contract(&client, id)
                    .await
                    .map_err(|e| format!("Failed to inspect ABI contract {}: {}", id, e))?;
                let deployed = get_wasm_bytecode(&client, &inspection.wasm_hash)
                    .await
                    .map_err(|e| format!("Failed to fetch ABI contract {}: {}", id, e))?;
                Some(
                    parse_contract_spec(&deployed)
                        .map_err(|e| format!("Failed to parse ABI: {}", e))?,
                )
            } else {
                None
            };

            match inspect_contract(&client, &contract_id).await {
                Ok(inspection) => {
                    if fmt == OutputFormat::Json {
                        let mut output = serde_json::to_value(&inspection)?;
                        if let Some(spec) = contract_spec.as_ref() {
                            output["abi_spec"] = serde_json::to_value(spec)?;
                        }
                        println!("{}", serde_json::to_string(&output)?);
                    } else {
                        if interface {
                            if let Some(spec) = contract_spec.as_ref() {
                                println!("{}", render_contract_interface(spec, markdown));
                                return Ok(());
                            }
                            return Err(
                                "--interface requires --abi or --abi-contract so the contract spec can be rendered"
                                    .into(),
                            );
                        }
                        println!("Contract Inspection");
                        println!("Contract ID: {}", inspection.contract_id);
                        println!("WASM Hash: {}", inspection.wasm_hash);
                        println!("Storage Keys: {}", inspection.storage_keys.len());

                        if let Some(spec) = contract_spec {
                            println!("\nABI Functions:");
                            for f in &spec.functions {
                                println!("  - {} ({})", f.name, f.doc);
                            }
                            println!("\nABI Events:");
                            for e in &spec.events {
                                println!("  - {}", e.name);
                            }
                            if !spec.custom_types.is_empty() {
                                println!("\nABI Custom Types:");
                                for t in &spec.custom_types {
                                    println!("  - {} ({})", t.name, t.kind);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error inspecting contract: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Verify {
            contract,
            wasm,
            network,
            format,
            upgrade_safety,
            net,
        } => {
            let fmt = parse_format_str(&format);
            let target = match resolve_target_network(network.as_deref(), &net) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };
            let client = target.client;
            let network = target.network_name;

            // On-chain upgrade-safety verification: compare the live deployed
            // contract's interface against a local candidate WASM.
            if upgrade_safety {
                match wasm.as_ref() {
                    None => {
                        eprintln!("Error: --upgrade-safety requires --wasm <candidate.wasm>");
                        process::exit(1);
                    }
                    Some(path) => {
                        let candidate_bytes = fs::read(path).unwrap_or_else(|e| {
                            eprintln!("Error reading WASM file {}: {}", path, e);
                            process::exit(1);
                        });
                        match run_upgrade_safety(
                            &client,
                            &contract,
                            &candidate_bytes,
                            &network,
                            fmt,
                        )
                        .await
                        {
                            Ok(()) => return Ok(()),
                            Err(e) => {
                                eprintln!("Error verifying upgrade safety: {}", e);
                                process::exit(1);
                            }
                        }
                    }
                }
            }

            // Read + hash the local WASM fully offline (no RPC).
            let local_bytes = match wasm.as_ref() {
                Some(path) => {
                    let bytes = fs::read(path).unwrap_or_else(|e| {
                        eprintln!("Error reading WASM file {}: {}", path, e);
                        process::exit(1);
                    });
                    Some(bytes)
                }
                None => None,
            };

            match verify_contract(&client, &contract, local_bytes.as_deref(), &network).await {
                Ok(report) => {
                    if fmt == OutputFormat::Json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report).unwrap_or_else(|e| {
                                eprintln!("Error serializing report: {}", e);
                                process::exit(1);
                            })
                        );
                    } else {
                        println!("Contract Verification Report");
                        println!("============================");
                        println!("Contract ID : {}", report.contract_id);
                        println!("Network     : {}", report.network);
                        println!("On-chain WASM: {}", report.on_chain_wasm_hash);
                        if let (Some(lh), Some(sz)) =
                            (&report.local_wasm_hash, &report.local_wasm_size_bytes)
                        {
                            println!("Local WASM   : {}   ({} bytes)", lh, sz);
                        }
                        let match_str = match report.matches {
                            Some(true) => "YES".to_string(),
                            Some(false) => "NO".to_string(),
                            None => "N/A (no local WASM provided)".to_string(),
                        };
                        println!("Match        : {}", match_str);
                        println!("Status       : {}", report.verification_status);
                        if !report.explanation.is_empty() {
                            println!();
                            println!("{}", report.explanation);
                        }
                    }
                    // Exit-code contract: a reported Mismatch is a failed
                    // verification and must fail a caller/CI step. `Verified`
                    // and `OnChainOnly` keep exiting 0; genuine RPC/IO errors
                    // keep exiting non-zero via the `Err` arm below. Mirrors
                    // `release-assurance`, which already fails on mismatch.
                    if report.matches == Some(false) {
                        eprintln!("Error: local WASM does NOT match the deployed contract code");
                        process::exit(1);
                    }
                }
                Err(e) => {
                    // Surface actionable messages per _PLAN.md §9.
                    if let Some(path) = wasm.as_ref() {
                        if e.contains("WASM parse error") {
                            eprintln!("Error: {} is not valid WASM", path);
                            process::exit(1);
                        }
                        if e.contains("Empty") {
                            eprintln!("Error: {} is empty", path);
                            process::exit(1);
                        }
                    }
                    eprintln!("Error verifying contract: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Health {
            contract,
            wasm,
            network,
            format,
            net,
        } => {
            let fmt = parse_format_str(&format);
            let target = match resolve_target_network(network.as_deref(), &net) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };
            let client = target.client;
            let network = target.network_name;

            // Read + hash the local WASM fully offline (no RPC).
            let local_bytes = match wasm.as_ref() {
                Some(path) => {
                    let bytes = fs::read(path).unwrap_or_else(|e| {
                        eprintln!("Error reading WASM file {}: {}", path, e);
                        process::exit(1);
                    });
                    Some(bytes)
                }
                None => None,
            };

            match contract_health(&client, &contract, local_bytes.as_deref(), &network).await {
                Ok(report) => {
                    if fmt == OutputFormat::Json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report).unwrap_or_else(|e| {
                                eprintln!("Error serializing report: {}", e);
                                process::exit(1);
                            })
                        );
                    } else {
                        println!("Contract Health Report");
                        println!("=======================");
                        println!("Contract ID : {}", report.contract_id);
                        println!("Network     : {}", report.network);
                        println!("Health      : {}", report.health.to_uppercase());
                        if let Some(v) = report.verified {
                            let local_str = report.local_wasm_hash.as_deref().unwrap_or("");
                            println!(
                                "On-chain WASM : {} (verified against local: {})",
                                report.on_chain_wasm_hash,
                                if v { "YES" } else { "NO — MISMATCH" }
                            );
                            if !local_str.is_empty() {
                                println!("Local WASM   : {}", local_str);
                            }
                        } else {
                            println!("On-chain WASM : {}", report.on_chain_wasm_hash);
                        }
                        println!("Storage:");
                        println!("  Total Entries: {}", report.storage.total_entries);
                        println!("    Instance:    {}", report.storage.instance_entries);
                        println!("    Persistent: {}", report.storage.persistent_entries);
                        println!("    Temporary:   {}", report.storage.temporary_entries);
                        if report.storage.other_entries > 0 {
                            println!("    Other:      {}", report.storage.other_entries);
                        }
                        if let Some(ttl) = &report.storage.ttl {
                            println!("TTL:");
                            println!("  Min TTL:       {}", ttl.minimum_ttl);
                            println!("  Max TTL:       {}", ttl.maximum_ttl);
                            println!("  Average TTL:   {}", ttl.average_ttl);
                            println!("  Expiring Soon: {}", ttl.expiring_entries_count);
                            if let Some(cost) = ttl.estimated_rent_cost {
                                println!("  Est. Rent Cost: {} stroops", cost);
                            }
                        }
                        if !report.reasons.is_empty() {
                            println!();
                            println!("Verdict: {}", report.reasons.join(" "));
                        } else {
                            println!();
                            let verified_note = match report.verified {
                                Some(true) => "WASM verified, ",
                                Some(false) => "MISMATCH; ",
                                None => "No local WASM supplied; verification skipped. ",
                            };
                            println!(
                                "Verdict: Contract posture is healthy. {}no entries expiring soon.",
                                verified_note
                            );
                        }
                    }
                    // A `critical` verdict (mismatched deployed WASM, per
                    // `derive_verdict`) is a real failure and must be
                    // observable from the exit code alone, so CI and agents
                    // cannot read a green run over a mismatched artifact.
                    // `at_risk` keeps its existing non-blocking meaning, and
                    // `release-assurance` treats both the same way via
                    // `health_status()` (Fail vs Review).
                    if health_status(&report.health) == RaStatus::Fail {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    // Surface actionable messages per _PLAN.md §11.
                    if let Some(path) = wasm.as_ref() {
                        if e.contains("WASM parse error") {
                            eprintln!("Error: {} is not valid WASM", path);
                            process::exit(1);
                        }
                        if e.contains("Empty") {
                            eprintln!("Error: {} is empty", path);
                            process::exit(1);
                        }
                    }
                    if e.contains("not found on") {
                        eprintln!("Error: contract {} not found on {}", contract, network);
                        process::exit(1);
                    }
                    eprintln!("Error fetching contract: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Tx { action, net } => match action {
            TxAction::Inspect { hash, format } => {
                let fmt = parse_format_str(&format);
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                match inspect_transaction(&client, &hash).await {
                    Ok(tx_info) => {
                        if fmt == OutputFormat::Json {
                            let json_str = serde_json::to_string(&tx_info)?;
                            println!("{}", json_str);
                        } else {
                            println!("Transaction:");
                            println!();
                            println!("Hash: {}", tx_info.hash);
                            println!("Status: {}", tx_info.status.as_deref().unwrap_or("Unknown"));
                            println!(
                                "Ledger: {}",
                                tx_info.ledger.map_or("N/A".to_string(), |v| v.to_string())
                            );
                            println!(
                                "Fee: {}",
                                tx_info
                                    .fee_charged
                                    .map_or("N/A".to_string(), |v| format!("{v} stroops"))
                            );
                            println!(
                                "Operations: {}",
                                tx_info
                                    .operation_count
                                    .map_or("N/A".to_string(), |v| v.to_string())
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("Error inspecting transaction: {}", e);
                        process::exit(1);
                    }
                }
            }
            TxAction::Validate { envelope, format } => {
                let fmt = parse_format_str(&format);
                let env_data = match resolve_tx_input(&envelope) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };

                use sdkt_core::validation::validate_base64;
                let report = validate_base64(env_data.trim());

                if fmt == OutputFormat::Json {
                    let json_str = serde_json::to_string(&report)?;
                    println!("{}", json_str);
                } else {
                    println!("Validation Report:");
                    if report.valid {
                        println!("  Status: VALID");
                    } else {
                        println!("  Status: INVALID");
                    }
                    if !report.errors.is_empty() {
                        println!("  Errors:");
                        for err in &report.errors {
                            println!("    - {}", err.message());
                        }
                    }
                    if !report.warnings.is_empty() {
                        println!("  Warnings:");
                        for warn in &report.warnings {
                            println!("    - {:?}", warn);
                        }
                    }
                }

                if !report.valid {
                    process::exit(1);
                }
            }
            TxAction::Decode { envelope, format } => {
                let fmt = parse_format_str(&format);
                let env_data = match resolve_tx_input(&envelope) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };
                let view = match sdkt_xdr::decode_envelope(&env_data) {
                    Ok(view) => view,
                    Err(e) => {
                        eprintln!("Error: invalid transaction envelope: {}", e);
                        process::exit(1);
                    }
                };
                if fmt == OutputFormat::Json {
                    println!("{}", serde_json::to_string_pretty(&view)?);
                } else {
                    print_envelope_pretty(&view);
                }
            }
            TxAction::Simulate {
                envelope,
                format,
                abi,
                abi_contract,
            } => {
                // `--abi` (local WASM) and `--abi-contract` (on-chain WASM) are
                // mutually exclusive sources for result decoding.
                if let Err(e) =
                    commands::abi::check_abi_mutual_exclusion(abi.as_ref(), abi_contract.as_ref())
                {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }

                let fmt = parse_format_str(&format);
                let env_data = match resolve_tx_input(&envelope) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                match simulate_transaction(&client, env_data.trim()).await {
                    Ok(sim) => {
                        // A failed simulation reports its own error immediately.
                        // Resolving --abi / --abi-contract after a failure could
                        // mask the real simulation error with a secondary ABI
                        // lookup/fetch/parse error, so the failure path never
                        // consults the ABI sources.
                        if let Some(sim_err) = &sim.error {
                            if fmt == OutputFormat::Json {
                                let json_obj = serde_json::json!({
                                    "error": sim.error,
                                    "latestLedger": sim.latest_ledger,
                                    "minResourceFee": sim.min_resource_fee,
                                    "restorePreamble": sim.restore_preamble,
                                    "cost": sim.cost,
                                    "events": sim.events,
                                    "stateChanges": sim.state_changes,
                                    "results": sim.results,
                                });
                                println!("{}", serde_json::to_string(&json_obj)?);
                                process::exit(1);
                            } else {
                                println!("Simulation Result:");
                                println!("  Status: FAILED");
                                println!("  Error: {sim_err}");
                                process::exit(1);
                            }
                        } else {
                            // Load ABI spec from one of two sources: a local WASM
                            // file (`--abi`) or a deployed contract's on-chain WASM
                            // fetched via the path (`--abi-contract`).
                            let abi_spec = commands::abi::resolve_abi_spec(
                                abi.as_ref(),
                                abi_contract.as_ref(),
                                &client,
                            )
                            .await?;

                            // Decode primary result if ABI available
                            let decoded_result = abi_spec.as_ref().and_then(|spec| {
                                sim.results.first().and_then(|first_result| {
                                    sdkt_xdr::scval_from_base64(&first_result.xdr).map(|scval| {
                                        sdkt_xdr::abi_decode::decode_with_abi(spec, &scval, None)
                                    })
                                })
                            });

                            if fmt == OutputFormat::Json {
                                let mut json_obj = serde_json::json!({
                                    "error": sim.error,
                                    "latestLedger": sim.latest_ledger,
                                    "minResourceFee": sim.min_resource_fee,
                                    "restorePreamble": sim.restore_preamble,
                                    "cost": sim.cost,
                                    "events": sim.events,
                                    "stateChanges": sim.state_changes,
                                    "results": sim.results,
                                });

                                if let Some(decoded) = &decoded_result {
                                    json_obj["decodedResult"] = serde_json::json!({
                                        "raw": decoded.raw,
                                        "label": decoded.label,
                                        "matchedType": decoded.matched_type,
                                        "fields": decoded.fields,
                                    });
                                }

                                println!("{}", serde_json::to_string(&json_obj)?);
                            } else {
                                println!("Simulation Result:");
                                println!("  Status: SUCCESS");
                                println!(
                                    "  Ledger: {}",
                                    sim.latest_ledger.as_deref().unwrap_or("N/A")
                                );
                                println!("  Min Resource Fee: {} stroops", sim.min_resource_fee);

                                if let Some(preamble) = &sim.restore_preamble {
                                    println!("  Restore Preamble Required:");
                                    println!(
                                        "    Min Resource Fee: {} stroops",
                                        preamble.min_resource_fee
                                    );
                                    println!(
                                        "    Transaction Data: ({} bytes)",
                                        preamble.transaction_data.len()
                                    );
                                }

                                if let Some(cost) = &sim.cost {
                                    println!("  Cost:");
                                    println!("    CPU Instructions: {}", cost.cpu_insns);
                                    println!("    Memory Bytes: {}", cost.mem_bytes);
                                }

                                // Show decoded result if ABI was provided
                                if let Some(decoded) = &decoded_result {
                                    println!("  Decoded Result: {}", decoded.label);
                                    if let Some(matched) = &decoded.matched_type {
                                        println!("    ABI Type: {matched}");
                                    }
                                    if let Some(fields) = &decoded.fields {
                                        if !fields.is_empty() {
                                            println!("    Fields:");
                                            for (k, v) in fields {
                                                println!("      {k}: {v}");
                                            }
                                        }
                                    }
                                }

                                if !sim.events.is_empty() {
                                    println!("  Events: {} emitted", sim.events.len());
                                }
                                if !sim.state_changes.is_empty() {
                                    println!(
                                        "  State Changes: {} entries modified",
                                        sim.state_changes.len()
                                    );
                                }
                                if !sim.results.is_empty() {
                                    println!("  Operations: {} results", sim.results.len());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error simulating transaction: {e}");
                        process::exit(1);
                    }
                }
            }
            TxAction::Submit {
                envelope,
                wait,
                timeout,
                interval,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let env_data = match resolve_tx_input(&envelope) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };
                let client = resolve_rpc_client_mutating(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                use sdkt_rpc::{submit_and_wait, PollConfig, TransactionStatus};
                use std::time::Duration;

                let poll_cfg = PollConfig {
                    timeout: Duration::from_secs(timeout),
                    interval: Duration::from_secs(interval),
                };

                match submit_and_wait(&client, env_data.trim(), wait, &poll_cfg).await {
                    Ok(res) => {
                        if fmt == OutputFormat::Json {
                            println!("{}", serde_json::to_string(&res)?);
                        } else {
                            println!("Submission Result:");
                            println!("  Hash:   {}", res.hash);
                            println!("  Status: {:?}", res.status);
                            if let Some(ledger) = &res.latest_ledger {
                                println!("  Ledger: {}", ledger);
                            }
                            if let Some(xdr) = &res.result_xdr {
                                println!("  Result XDR: {}", xdr);
                            }
                            if !res.events.is_empty() {
                                println!("  Events:");
                                for event in &res.events {
                                    println!("    {}", event);
                                }
                            }
                            if let Some(code) = &res.error_code {
                                println!("  Error:    {}", code);
                            }
                            if let Some(xdr) = &res.error_result_xdr {
                                println!("  Error Result XDR: {}", xdr);
                            }
                            if !res.diagnostic_events.is_empty() {
                                println!("  Diagnostics:");
                                for event in &res.diagnostic_events {
                                    println!("    {}", event);
                                }
                            }
                        }
                        // A settled on-chain failure must not look like a
                        // success to scripts: match `invoke`, which already
                        // exits 1 unless the status is SUCCESS (a no-wait
                        // PENDING submission still exits 0).
                        if res.status == TransactionStatus::Failed {
                            process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error submitting transaction: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            TxAction::Build {
                source,
                sequence,
                fee,
                contract,
                function,
                arg,
                format,
                output,
                memo_text,
                memo_id,
            } => {
                let fmt = parse_format_str(&format);

                // `--memo-text` and `--memo-id` are mutually exclusive. Fail
                // deterministically before any network I/O (same pre-check style
                // as the `--abi` / `--abi-contract` flags elsewhere in this file).
                if memo_text.is_some() && memo_id.is_some() {
                    eprintln!("Error: specify only one of --memo-text or --memo-id");
                    process::exit(1);
                }
                let memo = match (memo_text.as_deref(), memo_id) {
                    (Some(text), _) => match sdkt_xdr::memo_text(text) {
                        Ok(memo) => Some(memo),
                        Err(e) => {
                            eprintln!("Error: {e}");
                            process::exit(1);
                        }
                    },
                    (None, Some(id)) => Some(sdkt_xdr::memo_id(id)),
                    (None, None) => None,
                };

                // If source doesn't start with 'G' and isn't 56 chars, try to load it as an identity
                let mut source_account = source.clone();
                if !source_account.starts_with('G') || source_account.len() != 56 {
                    use sdkt_storage::IdentityStore;
                    if let Ok(store) = IdentityStore::new() {
                        if let Ok(identity) = store.get(&source_account) {
                            source_account = identity.public_key;
                        }
                    }
                }

                let parsed_args = parse_typed_args(&arg, false)?;

                // A build only stays fully offline when the caller pinned the
                // sequence *and* named no network: resolving the sequence from
                // the account is itself an RPC round trip, so once that happens
                // there is a reachable network to price against too. `client`
                // is `Some` exactly when this build may talk to one.
                let client = if sequence.is_none()
                    || network_is_explicit(
                        &net.rpc_url,
                        &net.network_passphrase,
                        &net.network_profile,
                    ) {
                    Some(resolve_rpc_client(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    ))
                } else {
                    None
                };

                // Resolve the sequence: use the explicit `--sequence` override
                // verbatim, otherwise fetch the account's next sequence from the
                // network (same mechanism the deploy path already uses).
                let sequence = match (sequence, client.as_ref()) {
                    (Some(explicit), _) => explicit,
                    (None, Some(client)) => {
                        match get_next_sequence(client, &source_account).await {
                            Ok(seq) => seq,
                            Err(e) => {
                                eprintln!("Error resolving sequence for {source_account}: {e}");
                                process::exit(1);
                            }
                        }
                    }
                    // Unreachable: `client` is built whenever `sequence` is None.
                    (None, None) => unreachable!("sequence lookup requires an RPC client"),
                };

                let params = InvokeTransactionParams {
                    source_account,
                    sequence,
                    fee: fee.unwrap_or(sdkt_rpc::INCLUSION_FEE),
                    contract_id: contract.clone(),
                    function: function.clone(),
                    args: parsed_args,
                    memo,
                };

                // Fee precedence:
                //   1. an explicit --fee wins outright, online or not, and
                //      costs no extra round trip;
                //   2. otherwise, when this build already has a network to
                //      talk to, simulate and adopt the resource fee and
                //      footprint it reports, mirroring `invoke`;
                //   3. otherwise stay fully offline and warn, because the base
                //      inclusion fee alone will be rejected on submission.
                //
                // A simulation that cannot reach the network degrades to (3)
                // rather than failing the build.
                let built = match (fee, client.as_ref()) {
                    (None, Some(client)) => {
                        match sdkt_rpc::simulate_invoke(client, &params).await {
                            Ok(sim) => {
                                let priced = InvokeTransactionParams {
                                    fee: sim.total_fee,
                                    ..params.clone()
                                };
                                if fmt != OutputFormat::Json {
                                    eprintln!(
                                    "Fee from simulation: {} inclusion + {} resource = {} stroops",
                                    sdkt_rpc::INCLUSION_FEE,
                                    sim.min_resource_fee,
                                    sim.total_fee
                                );
                                }
                                sdkt_xdr::build_invoke_transaction_with_data(
                                    &priced,
                                    sim.soroban_data,
                                    sim.auth_entries,
                                )
                                .map_err(|e| e.to_string())
                            }
                            // The operator named a network but we could not
                            // reach it. Refusing here would turn a build that
                            // used to succeed offline into a failure, so fall
                            // back to the offline envelope and say plainly
                            // that it is not priced for submission.
                            Err(e) => {
                                eprintln!(
                                    "Warning: could not derive the fee from simulation \
                                     ({e}); falling back to the base inclusion fee ({} \
                                     stroops), which does not cover the Soroban resource \
                                     fee. Pass --fee to set it explicitly.",
                                    sdkt_rpc::INCLUSION_FEE
                                );
                                build_invoke_transaction(&params).map_err(|e| e.to_string())
                            }
                        }
                    }
                    // An explicit --fee, or nothing to simulate against: build
                    // offline and only warn when the fee was not chosen.
                    (explicit, _) => {
                        if explicit.is_none() {
                            eprintln!(
                                "Warning: fee is the base inclusion fee ({} stroops) only and \
                                 does not cover the Soroban resource fee. Re-run with \
                                 --network-profile or --rpc-url to derive it from simulation, \
                                 or pass --fee explicitly.",
                                sdkt_rpc::INCLUSION_FEE
                            );
                        }
                        build_invoke_transaction(&params).map_err(|e| e.to_string())
                    }
                };

                match built {
                    Ok(env) => {
                        if let Some(ref path) = output {
                            if let Err(e) = fs::write(path, &env) {
                                eprintln!("Error writing to file: {}", e);
                                process::exit(1);
                            }
                            if fmt != OutputFormat::Json {
                                println!("Transaction envelope written to {}", path);
                            }
                        }

                        if fmt == OutputFormat::Json {
                            println!(r#"{{"envelope": "{}"}}"#, env);
                        } else if output.is_none() {
                            println!("Transaction Envelope (Base64):");
                            println!("{}", env);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error building transaction: {}", e);
                        process::exit(1);
                    }
                }
            }
            TxAction::Sign {
                input,
                output,
                identity,
                network,
                format,
            } => {
                let fmt = parse_format_str(&format);

                // --- Network resolution (strict; reject unknown labels) ---
                let network = match network.trim().to_ascii_lowercase().as_str() {
                    "testnet" => Network::Testnet,
                    "mainnet" => Network::Mainnet,
                    "futurenet" => Network::Futurenet,
                    other if other.starts_with("custom:") => Network::parse(other),
                    _ => {
                        eprintln!(
                            "Error: invalid network '{}' (expected testnet|mainnet|futurenet|custom:<passphrase>)",
                            network
                        );
                        process::exit(1);
                    }
                };

                // --- Input resolution (file or inline base64) ---
                let env_data = match resolve_tx_input(&input) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };

                // --- Identity resolution (keystore) ---
                if identity.trim().is_empty() {
                    eprintln!("Error: missing identity (use --identity <name>)");
                    process::exit(1);
                }
                use sdkt_storage::IdentityStore;
                let store = match IdentityStore::new() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: cannot open identity store: {}", e);
                        process::exit(1);
                    }
                };
                // The flag defaults to the reserved "default" sentinel, which
                // resolves to the configured default identity; an explicit name
                // is looked up verbatim.
                let identity_obj = match store.resolve_signing_identity(&identity) {
                    Ok(obj) => obj,
                    Err(e) => {
                        if identity == sdkt_storage::DEFAULT_IDENTITY_NAME {
                            eprintln!("Error: {}", e);
                        } else {
                            eprintln!("Error: unknown identity '{}'", identity);
                        }
                        process::exit(1);
                    }
                };
                let signing_key = match store.load_signing_key(&identity_obj.name) {
                    Ok(k) => k,
                    Err(e) => {
                        eprintln!("Error: cannot load identity '{}': {}", identity_obj.name, e);
                        process::exit(1);
                    }
                };
                let signer = Ed25519Signer::from_seed(&signing_key.to_bytes());

                let opts = SigningOptions::with(network);
                match sign_transaction(env_data.trim(), &signer, &opts) {
                    Ok(signed) => {
                        if let Some(path) = &output {
                            if let Err(e) = fs::write(path, &signed) {
                                eprintln!("Error: cannot write output to '{}': {}", path, e);
                                process::exit(1);
                            }
                            if fmt != OutputFormat::Json {
                                println!("Signed transaction envelope written to {}", path);
                            }
                        }
                        if fmt == OutputFormat::Json {
                            println!(r#"{{"envelope": "{}"}}"#, signed);
                        } else if output.is_none() {
                            println!("Signed Transaction Envelope (Base64):");
                            println!("{}", signed);
                        }
                    }
                    Err(e) => {
                        let msg = match e {
                            SigningError::Base64(_) => "invalid base64 input".to_string(),
                            SigningError::Xdr(_) => {
                                "invalid envelope: does not parse as a transaction envelope"
                                    .to_string()
                            }
                            SigningError::EmptyEnvelope => {
                                "invalid envelope: input is empty".to_string()
                            }
                            SigningError::InvalidKeyLength(_)
                            | SigningError::InvalidSecretKey(_)
                            | SigningError::Sign(_) => "internal signing error".to_string(),
                        };
                        eprintln!("Error signing transaction: {}", msg);
                        process::exit(1);
                    }
                }
            }
        },
        Commands::Events {
            contract_id,
            format,
            start_ledger,
            end_ledger,
            abi,
            abi_contract,
            topic,
            net,
        } => {
            let fmt = parse_format_str(&format);
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

            // Reject inverted ranges before making RPC calls
            if let (Some(start), Some(end)) = (start_ledger, end_ledger) {
                if start > end {
                    eprintln!(
                        "Error: start ledger ({start}) cannot be greater than end ledger ({end})"
                    );
                    process::exit(1);
                }
            }

            // Encode `--topic` before any RPC call so an invalid symbol fails fast.
            let topic_filters = match topic.as_deref() {
                Some(symbol) => match sdkt_xdr::symbol_topic_base64(symbol) {
                    Ok(b64) => Some(vec![first_topic_filter(b64)]),
                    Err(e) => {
                        eprintln!("Error: --topic: {e}");
                        process::exit(1);
                    }
                },
                None => None,
            };

            // Resolve the ABI ContractSpec from one of two sources (mutually
            // exclusive): a local WASM file (`--abi`) or a deployed contract's
            // on-chain WASM fetched via the path (`--abi-contract`).
            if let Err(e) =
                commands::abi::check_abi_mutual_exclusion(abi.as_ref(), abi_contract.as_ref())
            {
                eprintln!("Error: {e}");
                process::exit(1);
            }

            let contract_spec: Option<sdkt_wasm::ContractSpec> =
                commands::abi::resolve_abi_spec(abi.as_ref(), abi_contract.as_ref(), &client)
                    .await?;

            if let (Some(spec), Some(symbol)) = (contract_spec.as_ref(), topic.as_deref()) {
                if let Some(warning) = unknown_event_topic_warning(spec, symbol) {
                    eprintln!("{warning}");
                }
            }

            match get_contract_events_filtered(
                &client,
                &contract_id,
                start_ledger,
                end_ledger,
                topic_filters,
            )
            .await
            {
                Ok(events) => {
                    if let Some(spec) = contract_spec {
                        // ABI-aware decoding: topics[0] is the event symbol,
                        // remaining topics + the data value carry the payload.
                        if fmt == OutputFormat::Json {
                            let decoded_events: Vec<serde_json::Value> = events
                                .iter()
                                .map(|ev| {
                                    let topic_scvals: Vec<stellar_xdr::ScVal> = ev
                                        .topics
                                        .iter()
                                        .filter_map(|t| sdkt_xdr::scval_from_base64(t))
                                        .collect();
                                    let data_scvals: Vec<stellar_xdr::ScVal> = ev
                                        .value
                                        .as_deref()
                                        .and_then(sdkt_xdr::scval_from_base64)
                                        .into_iter()
                                        .collect();
                                    let decoded =
                                        decode_event_topics(&spec, &topic_scvals, &data_scvals);
                                    serde_json::json!({
                                        "contract_id": ev.contract_id,
                                        "ledger": ev.ledger,
                                        "topics": ev.topics,
                                        "value": ev.value,
                                        "decoded": decoded.iter().map(|d| serde_json::json!({
                                            "raw": d.raw,
                                            "label": d.label,
                                            "matched_type": d.matched_type,
                                            "fields": d.fields
                                        })).collect::<Vec<_>>()
                                    })
                                })
                                .collect();
                            let json_str = serde_json::to_string(&decoded_events)?;
                            println!("{}", json_str);
                        } else {
                            println!("Contract Events (ABI-decoded):");
                            if events.is_empty() {
                                println!("No events found.");
                            } else {
                                for (i, ev) in events.iter().enumerate() {
                                    println!("\nEvent #{}", i + 1);
                                    println!(
                                        "Ledger: {}",
                                        ev.ledger.map_or("Unknown".to_string(), |v| v.to_string())
                                    );
                                    println!("Topics: {:?}", ev.topics);
                                    println!("Value: {}", ev.value.as_deref().unwrap_or("N/A"));

                                    // Decode with ABI using the real topics/value
                                    let topic_scvals: Vec<stellar_xdr::ScVal> = ev
                                        .topics
                                        .iter()
                                        .filter_map(|t| sdkt_xdr::scval_from_base64(t))
                                        .collect();
                                    let data_scvals: Vec<stellar_xdr::ScVal> = ev
                                        .value
                                        .as_deref()
                                        .and_then(sdkt_xdr::scval_from_base64)
                                        .into_iter()
                                        .collect();
                                    let decoded =
                                        decode_event_topics(&spec, &topic_scvals, &data_scvals);
                                    for d in decoded {
                                        println!("  Decoded: {}", d.label);
                                        if let Some(fields) = d.fields {
                                            for (k, v) in fields {
                                                println!("    {}: {}", k, v);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        // Original raw output
                        if fmt == OutputFormat::Json {
                            let json_str = serde_json::to_string(&events)?;
                            println!("{}", json_str);
                        } else {
                            println!("Contract Events:");
                            if events.is_empty() {
                                println!("No events found.");
                            } else {
                                for (i, ev) in events.iter().enumerate() {
                                    println!("\nEvent #{}", i + 1);
                                    println!(
                                        "Ledger: {}",
                                        ev.ledger.map_or("Unknown".to_string(), |v| v.to_string())
                                    );
                                    println!("Topics: {:?}", ev.topics);
                                    println!("Value: {}", ev.value.as_deref().unwrap_or("N/A"));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error fetching events: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Account {
            address,
            format,
            net,
        } => {
            let fmt = parse_format_str(&format);
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

            match inspect_account(&client, &address).await {
                Ok(account) => {
                    if fmt == OutputFormat::Json {
                        let json_str = serde_json::to_string(&account)?;
                        println!("{}", json_str);
                    } else {
                        println!("Account:");
                        println!();
                        println!("Address: {}", account.address);
                        println!(
                            "Sequence: {}",
                            account.sequence.as_deref().unwrap_or("Unknown")
                        );
                        println!("\nBalances:");
                        if account.balances.is_empty() {
                            println!("  (none)");
                        } else {
                            for b in account.balances {
                                if let (Some(code), Some(issuer)) = (&b.asset_code, &b.asset_issuer)
                                {
                                    println!("  Asset: {}:{} ({})", code, issuer, b.asset_type);
                                } else if let Some(code) = &b.asset_code {
                                    println!("  Asset: {} ({})", code, b.asset_type);
                                } else {
                                    println!("  Asset: {}", b.asset_type);
                                }
                                println!("  Balance: {}", b.balance);
                            }
                        }
                        println!("\nSigners:");
                        if account.signers.is_empty() {
                            println!("  (none)");
                        } else {
                            for s in account.signers {
                                println!("  Type: {}", s.key_type);
                                println!("  Key: {}", s.key);
                                println!(
                                    "  Weight: {}",
                                    s.weight.map_or("Unknown".to_string(), |w| w.to_string())
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error inspecting account: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Fee { action, net } => match action {
            FeeAction::Estimate {
                network,
                base_fees,
                rpc,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let network_kind = match network.parse::<NetworkKind>() {
                    Ok(nk) => nk,
                    Err(e) => {
                        eprintln!("Invalid network: {}", e);
                        std::process::exit(1);
                    }
                };
                let fee_config = FeeConfig {
                    network: network_kind,
                    multiplier_override: None,
                };

                let (stroops, xlm) = if rpc {
                    let client = resolve_rpc_client(
                        net.rpc_url.clone(),
                        net.network_passphrase.clone(),
                        net.network_profile.clone(),
                    );
                    match estimate_dynamic_fee(&client, fee_config).await {
                        Ok(result) => result,
                        Err(e) => {
                            eprintln!("Error fetching RPC fee stats: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    let base_fees_str = match base_fees {
                        Some(bf) => bf,
                        None => {
                            eprintln!("--base-fees is required when not using --rpc");
                            std::process::exit(1);
                        }
                    };
                    let samples: Result<Vec<LedgerFeeSample>, _> = base_fees_str
                        .split(',')
                        .map(|s| {
                            s.trim()
                                .parse::<u32>()
                                .map(|base_fee| LedgerFeeSample { base_fee })
                        })
                        .collect();
                    let samples = match samples {
                        Ok(s) => s,
                        Err(_) => {
                            eprintln!("Invalid base_fees. Must be comma-separated integers.");
                            std::process::exit(1);
                        }
                    };
                    let estimator = FeeEstimator::new(fee_config);
                    match estimator.estimate(&samples) {
                        Ok(result) => result,
                        Err(e) => {
                            eprintln!("Error estimating fee: {}", e);
                            std::process::exit(1);
                        }
                    }
                };

                if fmt == OutputFormat::Json {
                    println!("{{\"stroops\":{},\"xlm\":\"{}\"}}", stroops, xlm);
                } else {
                    println!("Fee Estimate ({}):", network_kind);
                    if rpc {
                        println!("Source: RPC");
                    }
                    println!("Stroops: {}", stroops);
                    println!("XLM: {}", xlm);
                }
            }
        },
        Commands::Diff {
            old_wasm,
            new_wasm,
            format,
            upgrade_safety,
            max_growth_pct,
            max_size_bytes,
        } => {
            let fmt = parse_format_str(&format);
            // Size policy is evaluated only in diff output mode: mixing it
            // into --upgrade-safety would conflate size budget with ABI
            // compatibility, so the combination is rejected explicitly.
            if upgrade_safety && (max_size_bytes.is_some() || max_growth_pct.is_some()) {
                eprintln!(
                    "Error: size-policy flags apply to `sdkt diff` output only; \
                     run without --upgrade-safety to enforce them (ABI verdict is a separate check)"
                );
                return Err("size policy cannot be combined with --upgrade-safety".into());
            }
            if let Err(e) = validate_size_threshold_bytes(max_size_bytes, "--max-size-bytes") {
                eprintln!("Error: {e}");
                return Err(e.into());
            }
            if let Err(e) = validate_size_threshold_pct(max_growth_pct, "--max-growth-pct") {
                eprintln!("Error: {e}");
                return Err(e.into());
            }
            let old_bytes = fs::read(&old_wasm)
                .map_err(|e| format!("Failed to read OLD WASM '{}': {}", old_wasm, e))?;
            let new_bytes = fs::read(&new_wasm)
                .map_err(|e| format!("Failed to read NEW WASM '{}': {}", new_wasm, e))?;

            match sdkt_wasm::diff_wasm(&old_bytes, &new_bytes) {
                Ok(report) => {
                    if upgrade_safety {
                        // Upgrade-safety verdict mode: reuse the diff, classify.
                        let verdict = sdkt_wasm::UpgradeVerdict::from_diff(&report);
                        if fmt == OutputFormat::Json {
                            println!("{}", serde_json::to_string(&verdict)?);
                        } else {
                            print_upgrade_verdict(&verdict);
                        }
                        return Ok(());
                    }
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        println!("Contract WASM Diff");
                        println!(
                            "  OLD: {} ({} bytes)",
                            report.old.hash, report.old.size_bytes
                        );
                        println!(
                            "  NEW: {} ({} bytes)",
                            report.new.hash, report.new.size_bytes
                        );
                        println!();
                        if report.is_identical() {
                            println!("No ABI differences detected.");
                        } else {
                            if !report.added_functions.is_empty() {
                                println!("Added functions ({}):", report.added_functions.len());
                                for f in &report.added_functions {
                                    println!("  + {} ({})", f.name, sig_string(f));
                                }
                            }
                            if !report.removed_functions.is_empty() {
                                println!("Removed functions ({}):", report.removed_functions.len());
                                for f in &report.removed_functions {
                                    println!("  - {} ({})", f.name, sig_string(f));
                                }
                            }
                            if !report.changed_functions.is_empty() {
                                println!(
                                    "Changed signatures ({}):",
                                    report.changed_functions.len()
                                );
                                for c in &report.changed_functions {
                                    println!("  ~ {} :", c.name);
                                    println!("      old: {}", sig_string(&c.old));
                                    println!("      new: {}", sig_string(&c.new));
                                }
                            }
                            if !report.added_events.is_empty() {
                                println!("Added events ({}):", report.added_events.len());
                                for e in &report.added_events {
                                    println!("  + {}", e);
                                }
                            }
                            if !report.removed_events.is_empty() {
                                println!("Removed events ({}):", report.removed_events.len());
                                for e in &report.removed_events {
                                    println!("  - {}", e);
                                }
                            }
                            if !report.changed_events.is_empty() {
                                println!("Changed events ({}):", report.changed_events.len());
                                for c in &report.changed_events {
                                    println!("  ~ {} :", c.name);
                                    println!("      old: {}", sdkt_wasm::event_sig(&c.old));
                                    println!("      new: {}", sdkt_wasm::event_sig(&c.new));
                                }
                            }
                            if !report.added_types.is_empty() {
                                println!("Added types ({}):", report.added_types.len());
                                for t in &report.added_types {
                                    println!("  + {}", t);
                                }
                            }
                            if !report.removed_types.is_empty() {
                                println!("Removed types ({}):", report.removed_types.len());
                                for t in &report.removed_types {
                                    println!("  - {}", t);
                                }
                            }
                            if !report.changed_types.is_empty() {
                                println!("Changed types ({}):", report.changed_types.len());
                                for c in &report.changed_types {
                                    println!("  ~ {} :", c.name);
                                    println!("      old: {}", sdkt_wasm::type_sig(&c.old));
                                    println!("      new: {}", sdkt_wasm::type_sig(&c.new));
                                }
                            }
                        }
                        println!();
                        println!("Total changes: {}", report.total_changes());
                    }
                    // Size policy (opt-in): evaluated only after the report is
                    // emitted so JSON consumers keep a clean document; violations
                    // go to stderr and block the run (exit 1). ABI compatibility
                    // is unaffected — the report itself is still produced.
                    let violations =
                        evaluate_size_policy_for_diff(&report, max_size_bytes, max_growth_pct);
                    if !violations.is_empty() {
                        for v in &violations {
                            eprintln!("Error: size policy violation: {v}");
                        }
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error diffing WASM: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Audit {
            paths,
            format,
            list_rules,
            disable,
            rules,
            no_plugins,
            abi,
            abi_contract,
            net,
        } => {
            // Audit supports three output formats: pretty, json, sarif.
            // We parse the format here rather than through the shared
            // `parse_format_str` helper so we can extend it without changing
            // the shared OutputFormat enum used by other commands.
            #[derive(PartialEq)]
            enum AuditFormat {
                Pretty,
                Json,
                Sarif,
            }
            let audit_fmt = match format.to_lowercase().as_str() {
                "pretty" => AuditFormat::Pretty,
                "json" => AuditFormat::Json,
                "sarif" => AuditFormat::Sarif,
                other => {
                    eprintln!(
                        "Invalid format '{}'. Use 'pretty', 'json', or 'sarif'.",
                        other
                    );
                    process::exit(1);
                }
            };
            // Keep `fmt` as OutputFormat for the list_rules branch which uses
            // the same pretty/JSON distinction.
            let fmt = match &audit_fmt {
                AuditFormat::Json => OutputFormat::Json,
                _ => OutputFormat::Pretty,
            };

            if list_rules {
                // SARIF is not meaningful for listing rules — reject early so
                // automation never receives unexpected plain text on stdout.
                if audit_fmt == AuditFormat::Sarif {
                    eprintln!(
                        "Error: --format sarif is not supported with --list-rules. \
                         Use --format json or --format pretty."
                    );
                    process::exit(1);
                }
                let all = sdkt_audit::all_rules();
                if fmt == OutputFormat::Json {
                    let items: Vec<sdkt_audit::RuleInfo> = all
                        .iter()
                        .map(|r| sdkt_audit::RuleInfo {
                            id: r.id().to_string(),
                            severity: r.severity(),
                            description: r.description().to_string(),
                        })
                        .collect();
                    println!("{}", serde_json::to_string(&items)?);
                } else {
                    println!("Available audit rules ({}):", all.len());
                    let id_width = all.iter().map(|r| r.id().len()).max().unwrap_or(8).max(8);
                    let sev_width = all
                        .iter()
                        .map(|r| r.severity().to_string().len())
                        .max()
                        .unwrap_or(8)
                        .max(8);
                    for r in &all {
                        println!(
                            "  {:<id_width$}  {:<sev_width$}  {}",
                            r.id(),
                            r.severity(),
                            r.description()
                        );
                    }
                }
                return Ok(());
            }

            if abi.is_some() && abi_contract.is_some() {
                return Err("specify only one of --abi or --abi-contract".into());
            }
            let audit_spec = if let Some(wasm_path) = abi.as_ref() {
                let bytes = fs::read(wasm_path)
                    .map_err(|e| format!("Failed to read ABI WASM '{}': {}", wasm_path, e))?;
                Some(
                    parse_contract_spec(&bytes)
                        .map_err(|e| format!("Failed to parse ABI WASM: {}", e))?,
                )
            } else if let Some(contract_id) = abi_contract.as_ref() {
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );
                let inspection = inspect_contract(&client, contract_id).await.map_err(|e| {
                    format!("Failed to inspect ABI contract {}: {}", contract_id, e)
                })?;
                let bytes = get_wasm_bytecode(&client, &inspection.wasm_hash)
                    .await
                    .map_err(|e| format!("Failed to fetch ABI contract {}: {}", contract_id, e))?;
                Some(
                    parse_contract_spec(&bytes)
                        .map_err(|e| format!("Failed to parse ABI WASM: {}", e))?,
                )
            } else {
                None
            };

            if !rules.is_empty() {
                // Validate/resolve each --rules entry before reading source.
                for r in &rules {
                    if let Some(meta) = sdkt_audit::plugin_store::show(r) {
                        if meta.abi_major != sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR {
                            eprintln!(
                                "Warning: skipping plugin '{}': ABI mismatch (plugin v{}.x, host v{}.x)",
                                r, meta.abi_major, sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR
                            );
                            continue;
                        }
                    }

                    let resolved = sdkt_audit::plugin_store::resolve(r)
                        .unwrap_or_else(|| std::path::PathBuf::from(r));
                    if !resolved.exists() {
                        eprintln!("Error: rule path '{}' does not exist", r);
                        process::exit(1);
                    }
                }
            }

            let mut source_paths = Vec::new();

            for input in &paths {
                let input_path = std::path::Path::new(input);
                let discovered = collect_rust_sources(input_path).map_err(|e| {
                    if paths.len() == 1 && input_path.extension().is_some_and(|ext| ext == "rs") {
                        format!("Failed to read source '{}': {}", input, e)
                    } else {
                        format!("Failed to discover Rust sources '{}': {}", input, e)
                    }
                })?;

                source_paths.extend(discovered);
            }

            source_paths.sort();
            source_paths.dedup();

            if source_paths.is_empty() {
                return Err("No Rust source files (.rs) found in the supplied paths".into());
            }

            let single_file = paths.len() == 1
                && source_paths.len() == 1
                && std::path::Path::new(&paths[0]).is_file();
            let multi_file = !single_file;

            let multi_file_plugin_paths: Vec<(std::path::PathBuf, String)> = {
                #[allow(unused_mut)]
                let mut paths_out = Vec::new();
                if rules.is_empty() {
                    if !no_plugins {
                        let installed = sdkt_audit::plugin_store::list();
                        for meta in installed {
                            if meta.abi_major != sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR {
                                eprintln!(
                                    "Warning: skipping plugin '{}': ABI mismatch (plugin v{}.x, host v{}.x)",
                                    meta.id, meta.abi_major, sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR
                                );
                                continue;
                            }

                            let Some(artifact_path) = sdkt_audit::plugin_store::resolve(&meta.id)
                            else {
                                eprintln!(
                                    "Warning: skipping plugin '{}': artifact not found",
                                    meta.id
                                );
                                continue;
                            };

                            let ext = artifact_path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|e| e.to_ascii_lowercase())
                                .unwrap_or_default();

                            match ext.as_str() {
                                "so" | "dylib" | "dll" => {
                                    #[cfg(feature = "plugins")]
                                    {
                                        paths_out.push((artifact_path, meta.id.clone()));
                                    }
                                    #[cfg(not(feature = "plugins"))]
                                    {
                                        eprintln!(
                                            "Warning: skipping native plugin '{}': build compiled without `plugins` feature",
                                            meta.id
                                        );
                                    }
                                }
                                "wasm" => {
                                    #[cfg(feature = "wasm-plugins")]
                                    {
                                        paths_out.push((artifact_path, meta.id.clone()));
                                    }
                                    #[cfg(not(feature = "wasm-plugins"))]
                                    {
                                        eprintln!(
                                            "Warning: skipping WASM plugin '{}': build compiled without `wasm-plugins` feature",
                                            meta.id
                                        );
                                    }
                                }
                                _ => {
                                    eprintln!(
                                        "Warning: skipping plugin '{}': unsupported artifact format",
                                        meta.id
                                    );
                                }
                            }
                        }
                    }
                } else {
                    for r in &rules {
                        if let Some(meta) = sdkt_audit::plugin_store::show(r) {
                            if meta.abi_major != sdkt_audit::plugin_abi::SDKT_AUDIT_ABI_MAJOR {
                                continue;
                            }
                        }

                        let resolved = sdkt_audit::plugin_store::resolve(r)
                            .unwrap_or_else(|| std::path::PathBuf::from(r));
                        let path_r = resolved.as_path();

                        if path_r.is_dir() {
                            continue;
                        }

                        let ext = path_r
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase())
                            .unwrap_or_default();

                        match ext.as_str() {
                            "so" | "dylib" | "dll" => {
                                #[cfg(feature = "plugins")]
                                {
                                    paths_out.push((path_r.to_path_buf(), r.clone()));
                                }
                                #[cfg(not(feature = "plugins"))]
                                {
                                    eprintln!(
                                        "Error: '{}' is a native plugin artifact but this build was compiled \
                                         without the `plugins` feature. Rebuild with --features plugins.",
                                        r
                                    );
                                    process::exit(1);
                                }
                            }
                            "wasm" => {
                                #[cfg(feature = "wasm-plugins")]
                                {
                                    paths_out.push((path_r.to_path_buf(), r.clone()));
                                }
                                #[cfg(not(feature = "wasm-plugins"))]
                                {
                                    eprintln!(
                                        "Error: '{}' is a WASM plugin artifact but this build was compiled \
                                         without the `wasm-plugins` feature. Rebuild with --features wasm-plugins.",
                                        r
                                    );
                                    process::exit(1);
                                }
                            }
                            _ => {
                                eprintln!(
                                    "Warning: skipping plugin '{}': unsupported artifact format",
                                    r
                                );
                            }
                        }
                    }
                }
                paths_out
            };

            let disabled_refs: Vec<&str> = disable.iter().map(String::as_str).collect();

            let mut aggregate = sdkt_audit::AuditReport::default();
            let mut per_file = Vec::new();
            let mut had_file_errors = false;
            #[allow(unused_mut)]
            let mut loaded_plugins = 0usize;

            for source_path in &source_paths {
                let source = match fs::read_to_string(source_path) {
                    Ok(source) => source,
                    Err(e) => {
                        had_file_errors = true;

                        let finding = sdkt_audit::Finding {
                            rule_id: "AUDIT-IO".to_string(),
                            severity: sdkt_audit::Severity::Critical,
                            message: format!("Failed to read source: {}", e),
                            location: None,
                            file: Some(source_path.display().to_string()),
                        };

                        let mut report = sdkt_audit::AuditReport::default();
                        report.add(finding.clone());

                        aggregate.add(finding);
                        per_file.push((source_path.clone(), report));
                        continue;
                    }
                };

                let mut local_reg = sdkt_audit::RuleRegistry::new();
                if multi_file {
                    local_reg.register_builtin_rules();
                }

                for (plugin_path, plugin_id) in &multi_file_plugin_paths {
                    let ext = plugin_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .unwrap_or_default();

                    match ext.as_str() {
                        "so" | "dylib" | "dll" => {
                            #[cfg(feature = "plugins")]
                            {
                                let result = if single_file {
                                    sdkt_audit::load_and_register(plugin_path, &source).map(|_| ())
                                } else {
                                    sdkt_audit::PluginRule::load(plugin_path, &source)
                                        .map(|rule| local_reg.register_rule(Box::new(rule)))
                                };

                                match result {
                                    Ok(()) => {
                                        if single_file {
                                            loaded_plugins += 1;
                                        }
                                    }
                                    Err(e) => {
                                        if matches!(
                                            e,
                                            sdkt_audit::PluginLoadError::AbiMismatch { .. }
                                        ) {
                                            eprintln!(
                                                "Warning: skipping native plugin '{}': {}",
                                                plugin_id, e
                                            );
                                            continue;
                                        }
                                        eprintln!(
                                            "Error loading native plugin '{}': {}",
                                            plugin_id, e
                                        );
                                        process::exit(1);
                                    }
                                }
                            }
                            #[cfg(not(feature = "plugins"))]
                            {
                                eprintln!(
                                    "Error: '{}' is a native plugin artifact but this build was compiled \
                                     without the `plugins` feature. Rebuild with --features plugins.",
                                    plugin_id
                                );
                                process::exit(1);
                            }
                        }
                        "wasm" => {
                            #[cfg(feature = "wasm-plugins")]
                            {
                                let result = if single_file {
                                    sdkt_audit::load_and_register_wasm(plugin_path, &source)
                                        .map(|_| ())
                                } else {
                                    sdkt_audit::WasmPluginRule::load(plugin_path, &source)
                                        .map(|rule| local_reg.register_rule(Box::new(rule)))
                                };

                                match result {
                                    Ok(()) => {
                                        if single_file {
                                            loaded_plugins += 1;
                                        }
                                    }
                                    Err(e) => {
                                        if matches!(
                                            e,
                                            sdkt_audit::WasmPluginLoadError::AbiMismatch { .. }
                                        ) {
                                            eprintln!(
                                                "Warning: skipping WASM plugin '{}': {}",
                                                plugin_id, e
                                            );
                                            continue;
                                        }
                                        eprintln!(
                                            "Error loading WASM plugin '{}': {}",
                                            plugin_id, e
                                        );
                                        process::exit(1);
                                    }
                                }
                            }
                            #[cfg(not(feature = "wasm-plugins"))]
                            {
                                eprintln!(
                                    "Error: '{}' is a WASM plugin artifact but this build was compiled \
                                     without the `wasm-plugins` feature. Rebuild with --features wasm-plugins.",
                                    plugin_id
                                );
                                process::exit(1);
                            }
                        }
                        _ => {
                            eprintln!(
                                "Warning: skipping plugin '{}': unsupported artifact format",
                                plugin_id
                            );
                        }
                    }
                }

                if single_file {
                    // Load dynamic rules first so registry ID deduplication
                    // preserves the original single-file behavior.
                    #[cfg(feature = "plugins")]
                    sdkt_audit_example_rule::register();

                    let result = match audit_spec.as_ref() {
                        Some(spec) => {
                            sdkt_audit::audit_source_with_spec(&source, spec, &disabled_refs)
                        }
                        None => sdkt_audit::audit_source_with(&source, &disabled_refs),
                    };
                    match result {
                        Ok(report) => {
                            for finding in &report.findings {
                                aggregate.add(finding.clone());
                            }
                            per_file.push((source_path.clone(), report));
                        }
                        Err(_) => {
                            eprintln!("Error auditing source: Failed to parse Rust source");
                            process::exit(1);
                        }
                    }
                    continue;
                }

                match sdkt_audit::scan_all_functions_str(&source) {
                    Some(scans) => {
                        let ctx = sdkt_audit::AuditContext {
                            spec: audit_spec.as_ref(),
                        };
                        let mut report = sdkt_audit::AuditReport::default();
                        local_reg.run_all(&scans, &ctx, &disabled_refs, &mut report);

                        if multi_file {
                            for finding in &mut report.findings {
                                finding.file = Some(source_path.display().to_string());
                            }
                        }

                        for finding in &report.findings {
                            aggregate.add(finding.clone());
                        }

                        per_file.push((source_path.clone(), report));
                    }
                    None => {
                        let e = "Failed to parse Rust source";
                        if single_file {
                            eprintln!("Error auditing source: {}", e);
                            process::exit(1);
                        }

                        had_file_errors = true;

                        let finding = sdkt_audit::Finding {
                            rule_id: "AUDIT-PARSE".to_string(),
                            severity: sdkt_audit::Severity::Critical,
                            message: format!("Failed to audit source: {}", e),
                            location: None,
                            file: Some(source_path.display().to_string()),
                        };

                        let mut report = sdkt_audit::AuditReport::default();
                        report.add(finding.clone());

                        aggregate.add(finding);
                        per_file.push((source_path.clone(), report));
                    }
                }
            }
            if audit_fmt == AuditFormat::Sarif {
                let rules_info: Vec<sdkt_audit::RuleInfo> = sdkt_audit::all_rules()
                    .iter()
                    .map(|r| sdkt_audit::RuleInfo {
                        id: r.id().to_string(),
                        severity: r.severity(),
                        description: r.description().to_string(),
                    })
                    .collect();
                if multi_file {
                    let sarif_str = sdkt_audit::sarif::report_to_sarif_string_multi_file(
                        &aggregate,
                        sdkt_version_string(),
                        &rules_info,
                    )
                    .map_err(|e| e.to_string())?;
                    println!("{}", sarif_str);
                } else {
                    let source_file = paths[0].as_str();
                    let sarif_str = sdkt_audit::sarif::report_to_sarif_string(
                        &aggregate,
                        source_file,
                        sdkt_version_string(),
                        &rules_info,
                    )
                    .map_err(|e| e.to_string())?;
                    println!("{}", sarif_str);
                }
                return Ok(());
            }

            if fmt == OutputFormat::Json {
                if multi_file {
                    #[derive(serde::Serialize)]
                    struct MultiFileAuditReport {
                        files: Vec<serde_json::Value>,
                        summary: sdkt_audit::AuditSummary,
                        spec_correlated: bool,
                    }

                    let files = per_file
                        .iter()
                        .map(|(path, report)| {
                            serde_json::json!({
                                "file": path.display().to_string(),
                                "report": report,
                            })
                        })
                        .collect();

                    println!(
                        "{}",
                        serde_json::to_string(&MultiFileAuditReport {
                            files,
                            summary: aggregate.summary.clone(),
                            spec_correlated: audit_spec.is_some(),
                        })?
                    );
                } else {
                    let mut report = serde_json::to_value(&aggregate)?;
                    report["spec_correlated"] = serde_json::Value::Bool(audit_spec.is_some());
                    println!("{}", serde_json::to_string(&report)?);
                }
            } else {
                for (path, report) in &per_file {
                    println!("Static Analysis Report: {}", path.display());

                    if audit_spec.is_some() {
                        println!("  Spec-correlated analysis: enabled");
                    }

                    if loaded_plugins > 0 {
                        println!(
                            "Rules loaded: 5 built-in, {} plugin{}",
                            loaded_plugins,
                            if loaded_plugins == 1 { "" } else { "s" }
                        );
                    }

                    println!(
                        "Severity: {} critical, {} warning, {} info ({} total)",
                        report.summary.critical,
                        report.summary.warning,
                        report.summary.info,
                        report.summary.total
                    );

                    if report.is_clean() {
                        println!("No issues found.");
                    } else {
                        println!();
                        for f in &report.findings {
                            let loc = f
                                .location
                                .as_ref()
                                .map(|l| format!(" [{}]", l))
                                .unwrap_or_default();
                            println!("  [{}] {} {}: {}", f.severity, f.rule_id, loc, f.message);
                        }
                    }

                    println!();
                }

                if multi_file {
                    println!(
                        "Aggregate Severity: {} critical, {} warning, {} info ({} total)",
                        aggregate.summary.critical,
                        aggregate.summary.warning,
                        aggregate.summary.info,
                        aggregate.summary.total
                    );
                }
            }
            if had_file_errors {
                process::exit(1);
            }
        }
        Commands::Wasm { action, net } => match action {
            WasmAction::Inspect { file, format } => {
                let fmt = parse_format_str(&format);

                let wasm_bytes = fs::read(&file).unwrap_or_else(|e| {
                    eprintln!("Error reading WASM file {}: {}", file, e);
                    process::exit(1);
                });

                let metadata = sdkt_wasm::parse_metadata(&wasm_bytes).unwrap_or_else(|e| {
                    eprintln!("Error parsing WASM metadata: {}", e);
                    process::exit(1);
                });

                // Attempt to parse contract spec, but it's optional
                let spec = parse_contract_spec(&wasm_bytes).ok();

                if fmt == OutputFormat::Json {
                    // Compatibility: emit basename only — callers must not
                    // depend on directory layout (issue #33 acceptance).
                    let file_basename = std::path::Path::new(&file)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&file);
                    let json = serde_json::json!({
                        "file": file_basename,
                        "metadata": metadata,
                        "spec": spec,
                    });
                    println!("{}", serde_json::to_string_pretty(&json).unwrap());
                } else {
                    println!("WASM Inspection Report: {}", file);
                    println!("========================================");
                    println!("Size: {} bytes", metadata.size_bytes);
                    println!("SHA-256 Hash: {}", metadata.hash);
                    println!("Version: {}", metadata.version);

                    println!("\nCustom Sections ({}):", metadata.custom_sections.len());
                    for section in &metadata.custom_sections {
                        println!("  - {}", section);
                    }

                    // Contract metadata (contractmetav0): generic key/value
                    // entries, section order, duplicates preserved.
                    println!("\nContract Metadata ({}):", metadata.contract_meta.len());
                    if metadata.contract_meta.is_empty() {
                        println!("  (none)");
                    }
                    for entry in &metadata.contract_meta {
                        println!("  {} = {}", entry.key, entry.value);
                    }

                    println!("\nExported Functions ({}):", metadata.exports.len());
                    for export in &metadata.exports {
                        println!("  - {} [{}]", export.name, export.kind);
                    }

                    if let Some(spec) = spec {
                        println!("\nContract Spec Available: Yes");
                        println!("  Functions: {}", spec.functions.len());
                        for f in &spec.functions {
                            println!(
                                "    - fn {}({}) -> {}",
                                f.name,
                                f.parameters.len(),
                                f.outputs.len()
                            );
                        }
                        println!("  Custom Types: {}", spec.custom_types.len());
                        println!("  Events: {}", spec.events.len());
                    } else {
                        println!("\nContract Spec Available: No");
                    }
                }
            }
            WasmAction::Metadata {
                contract,
                network,
                refresh,
                format,
                net: sub_net,
            } => {
                let fmt = parse_format_str(&format);
                let net = NetworkArgs {
                    network_profile: sub_net.network_profile.or(net.network_profile),
                    rpc_url: sub_net.rpc_url.or(net.rpc_url),
                    network_passphrase: sub_net.network_passphrase.or(net.network_passphrase),
                };
                let target = match resolve_target_network(network.as_deref(), &net) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };
                let client = target.client;
                let network = target.network_name;

                // Initialize cache
                let cache = match WasmCache::new() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Warning: could not initialize cache: {}", e);
                        WasmCache::with_dir(std::env::temp_dir().join("sdkt-fallback-cache"))
                    }
                };

                // First, inspect the contract to get its WASM hash
                let inspection = match inspect_contract(&client, &contract).await {
                    Ok(ins) => ins,
                    Err(e) => {
                        eprintln!("Error inspecting contract {}: {}", contract, e);
                        process::exit(1);
                    }
                };

                let wasm_hash = &inspection.wasm_hash;

                // Check cache if not forcing refresh
                let mut metadata = None;
                let mut cache_status = "Miss";

                if !refresh {
                    match cache.get(&network, wasm_hash) {
                        Ok(Some(m)) => {
                            metadata = Some(m);
                            cache_status = "Hit";
                        }
                        Ok(None) => {} // normal miss
                        Err(e) => {
                            eprintln!("Warning: Cache read error: {}", e);
                        }
                    }
                }

                // If no metadata from cache, fetch it
                let meta = if let Some(m) = metadata {
                    m
                } else {
                    let fetched = match get_wasm_metadata(&client, wasm_hash).await {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Error fetching WASM metadata: {}", e);
                            process::exit(1);
                        }
                    };

                    // Put into cache for future
                    if let Err(e) = cache.put(&network, &fetched, &[]) {
                        eprintln!("Warning: Failed to write to cache: {}", e);
                    }

                    fetched
                };

                // Enrich with storage/TTL/storage-key posture using the existing
                // StorageAnalyzer (available in the CLI layer). This populates the
                // remaining ContractInspection fields without adding a circular
                // sdkt-rpc -> sdkt-storage dependency.
                let mut inspection = inspection;
                if let Ok(report) = StorageAnalyzer::new(client.clone())
                    .inspect_contract_storage(&contract)
                    .await
                {
                    inspection.storage_summary = StorageSummary {
                        instance_entries: report.instance_entries,
                        persistent_entries: report.persistent_entries,
                        temporary_entries: report.temporary_entries,
                    };
                    inspection.ttl_info = report.ttl_summary.clone().map(|t| TtlInfoSummary {
                        minimum_ttl: t.minimum_ttl,
                        maximum_ttl: t.maximum_ttl,
                        average_ttl: t.average_ttl,
                        expiring_entries_count: t.expiring_entries_count,
                        estimated_rent_cost: t.estimated_rent_cost,
                    });
                    inspection.storage_keys = report
                        .entries
                        .iter()
                        .map(|e| StorageKeyInfo {
                            key: e.key.clone(),
                            key_type: format!("{:?}", e.class),
                            permissions: "read_write".to_string(),
                        })
                        .collect();
                }

                if fmt == OutputFormat::Json {
                    // Additive: the decoded contractmetav0 entries travel with
                    // the report so the on-chain path carries the same
                    // provenance the offline `wasm inspect` prints. The field
                    // is appended to the serialized object rather than added
                    // to `ContractInspection`, keeping that type's public
                    // shape (and its existing consumers) unchanged.
                    let mut value = serde_json::to_value(&inspection)?;
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "contract_meta".to_string(),
                            serde_json::to_value(&meta.contract_meta)?,
                        );
                    }
                    println!("{}", serde_json::to_string(&value)?);
                } else {
                    println!("WASM Metadata:");
                    println!("Contract ID: {}", contract);
                    println!("Network: {}", network);
                    println!("WASM Hash: {}", inspection.wasm_hash);
                    println!("Cache Status: {}", cache_status);
                    let size = inspection.wasm_size.unwrap_or(meta.size_bytes);
                    println!("Size: {} bytes", size);
                    println!("Exports: {}", meta.exports.len());
                    println!("Imports: {}", meta.imports.len());
                    println!("Custom Sections: {}", meta.custom_sections.len());
                    println!("Contract Metadata ({}):", meta.contract_meta.len());
                    if meta.contract_meta.is_empty() {
                        println!("  (none)");
                    }
                    for entry in &meta.contract_meta {
                        println!("  {} = {}", entry.key, entry.value);
                    }
                    if let Some(abi) = &inspection.abi {
                        println!(
                            "Functions ({}): {}",
                            abi.functions.len(),
                            abi.functions.join(", ")
                        );
                        println!("Events ({}): {}", abi.events.len(), abi.events.join(", "));
                        println!("Types ({}): {}", abi.types.len(), abi.types.join(", "));
                    } else {
                        println!(
                            "ABI: (unavailable — on-chain WASM not fetched or no contractspecv0)"
                        );
                    }
                    let s = &inspection.storage_summary;
                    println!(
                        "Storage: instance={} persistent={} temporary={}",
                        s.instance_entries, s.persistent_entries, s.temporary_entries
                    );
                    if let Some(ttl) = &inspection.ttl_info {
                        println!(
                            "TTL: min={} max={} avg={} expiring={}",
                            ttl.minimum_ttl,
                            ttl.maximum_ttl,
                            ttl.average_ttl,
                            ttl.expiring_entries_count
                        );
                    }
                }
            }
            WasmAction::Cache { action } => {
                // Initialize cache; fall back to a temp dir if the OS cache
                // directory cannot be resolved (e.g. fresh CI runner).
                let cache = match WasmCache::new() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Warning: could not initialize cache: {}", e);
                        WasmCache::with_dir(std::env::temp_dir().join("sdkt-fallback-cache"))
                    }
                };

                match action {
                    CacheAction::Info { network, format } => {
                        let fmt = parse_format_str(&format);
                        match cache.cache_info(&network) {
                            Ok(info) => {
                                if fmt == OutputFormat::Json {
                                    // In a real app we'd derive Serialize for CacheInfo,
                                    // but we can manually output JSON here or derive it in sdkt-storage
                                    println!(
                                        "{{\"network\":\"{}\",\"entry_count\":{},\"total_metadata_size_bytes\":{},\"total_wasm_size_bytes\":{}}}",
                                        info.network,
                                        info.entry_count,
                                        info.total_metadata_size_bytes,
                                        info.total_wasm_size_bytes
                                    );
                                } else {
                                    println!("Cache Info for Network '{}':", info.network);
                                    println!("Entries: {}", info.entry_count);
                                    println!(
                                        "Metadata Size: {} bytes",
                                        info.total_metadata_size_bytes
                                    );
                                    println!("WASM Size: {} bytes", info.total_wasm_size_bytes);
                                }
                            }
                            Err(e) => {
                                eprintln!("Error getting cache info: {}", e);
                                process::exit(1);
                            }
                        }
                    }
                    CacheAction::Remove { hash, network } => match cache.remove(&network, &hash) {
                        Ok(_) => {
                            println!("Removed {} from {} cache.", hash, network);
                        }
                        Err(e) => {
                            eprintln!("Error removing cache entry: {}", e);
                            process::exit(1);
                        }
                    },
                    CacheAction::Clear { network } => match cache.clear(&network) {
                        Ok(_) => {
                            println!("Cleared all cache entries for {}.", network);
                        }
                        Err(e) => {
                            eprintln!("Error clearing cache: {}", e);
                            process::exit(1);
                        }
                    },
                }
            }
        },
        Commands::Identity { action } => {
            use sdkt_storage::IdentityStore;
            let store = IdentityStore::new()?;
            match action {
                IdentityAction::Generate { name, format } => {
                    let fmt = parse_format_str(&format);
                    let identity = store.generate(&name)?;
                    if fmt == OutputFormat::Json {
                        // Public metadata only (name + public_key); Identity never holds secrets.
                        println!("{}", serde_json::to_string(&identity)?);
                    } else {
                        println!("Identity '{}' generated successfully.", identity.name);
                        println!("Public Key: {}", identity.public_key);
                    }
                }
                IdentityAction::Import { name, secret } => {
                    // `-` means: read the secret from stdin. A secret passed on
                    // argv is visible to any process listing on the machine;
                    // piping it in is the CI-safe path.
                    let secret = if secret == "-" {
                        use std::io::Read;
                        let mut buf = String::new();
                        std::io::stdin()
                            .read_to_string(&mut buf)
                            .map_err(|e| format!("Failed to read secret from stdin: {e}"))?;
                        buf.trim().to_string()
                    } else {
                        secret
                    };
                    let identity = store.import(&name, &secret)?;
                    println!("Identity '{}' imported successfully.", identity.name);
                    println!("Public Key: {}", identity.public_key);
                }
                IdentityAction::List { format } => {
                    let fmt = parse_format_str(&format);
                    let mut list = store.list()?;
                    list.sort_by(|a, b| a.name.cmp(&b.name));
                    let default_id = store.get_default().ok();

                    if fmt == OutputFormat::Json {
                        let items: Vec<serde_json::Value> = list
                            .iter()
                            .map(|id| {
                                let is_def = default_id.as_ref().is_some_and(|d| d.name == id.name);
                                serde_json::json!({
                                    "name": id.name,
                                    "public_key": id.public_key,
                                    "default": is_def,
                                })
                            })
                            .collect();
                        println!("{}", serde_json::to_string(&items)?);
                    } else if list.is_empty() {
                        println!("No identities found.");
                    } else {
                        println!("Identities:");
                        for id in list {
                            let is_def = default_id.as_ref().is_some_and(|d| d.name == id.name);
                            println!(
                                "  {} {} ({})",
                                if is_def { "*" } else { " " },
                                id.name,
                                id.public_key
                            );
                        }
                    }
                }
                IdentityAction::Show { name, format } => {
                    let fmt = parse_format_str(&format);
                    let identity = store.get(&name)?;
                    if fmt == OutputFormat::Json {
                        // Public metadata only — mirrors pretty fields (name + public_key).
                        println!("{}", serde_json::to_string(&identity)?);
                    } else {
                        println!("Identity: {}", identity.name);
                        println!("Public Key: {}", identity.public_key);
                    }
                }
                IdentityAction::Delete { name } => {
                    store.remove(&name)?;
                    println!("Identity '{}' removed.", name);
                }
                IdentityAction::Default { name } => {
                    store.set_default(&name)?;
                    println!("Identity '{}' set as default.", name);
                }
                IdentityAction::Fund {
                    name,
                    network_profile,
                    format,
                } => {
                    let fmt = parse_format_str(&format);
                    let identity = store
                        .get(&name)
                        .map_err(|e| format!("Identity '{}' not found: {}", name, e))?;

                    let net_store = NetworkStore::new()
                        .map_err(|e| format!("Failed to access network store: {}", e))?;
                    let profile = net_store.get(&network_profile).map_err(|e| {
                        format!("Network profile '{}' not found: {}", network_profile, e)
                    })?;

                    let friendbot_url = profile.friendbot_url.clone().ok_or_else(|| {
                        format!("Network profile '{}' has no Friendbot URL. Use 'sdkt network add --friendbot <url>' first.", network_profile)
                    })?;

                    match sdkt_rpc::fund_account(&friendbot_url, &identity.public_key).await {
                        Ok(res) => {
                            if fmt == OutputFormat::Json {
                                println!("{}", serde_json::to_string(&res)?);
                            } else {
                                println!("Identity Funded via Friendbot");
                                println!("  Identity:   {}", name);
                                println!("  Address:    {}", res.address);
                                println!("  Network:    {}", network_profile);
                                println!("  Endpoint:   {}", friendbot_url);
                                println!("  Status:     {}", res.status);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error funding identity: {}", e);
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Commands::Network { action } => {
            commands::network::run_network_action(action).await?;
        }
        Commands::DeploymentVerify {
            contract,
            wasm,
            network,
            format,
            net,
        } => {
            let fmt = parse_format_str(&format);
            commands::deployment_verify::run_deployment_verify(
                &contract,
                wasm.as_deref(),
                network,
                &net,
                &fmt,
            )
            .await?;
        }
        Commands::Init {
            name,
            minimal,
            force,
            format,
        } => {
            use sdkt_core::scaffold::{generate_project, ScaffoldConfig};

            let fmt = parse_format_str(&format);
            let scaffold_cfg = ScaffoldConfig {
                name: name.clone(),
                minimal,
                force,
            };

            match generate_project(&scaffold_cfg) {
                Ok(result) => {
                    if fmt == OutputFormat::Json {
                        let json = serde_json::json!({
                            "status": "created",
                            "project": name,
                            "files": result.files_created,
                        });
                        println!("{}", serde_json::to_string(&json)?);
                    } else {
                        println!("✓ Project '{}' created", name);
                        for f in &result.files_created {
                            println!("  ✓ {}", f);
                        }
                        println!("✓ Ready to build — run: sdkt build");
                    }
                }
                Err(e) => {
                    if fmt == OutputFormat::Json {
                        let json = serde_json::json!({
                            "status": "error",
                            "message": e.to_string(),
                        });
                        println!("{}", serde_json::to_string(&json)?);
                    } else {
                        eprintln!("Error: {}", e);
                    }
                    process::exit(1);
                }
            }
        }
        Commands::Deploy {
            wasm,
            wasm_hash,
            salt,
            show_address,
            dry_run,
            format,
            identity,
            arg,
            deny_breaking,
            old_wasm,
            net,
        } => {
            let fmt = parse_format_str(&format);

            // Exactly one code source: --wasm (upload + create) or --wasm-hash
            // (create-only from already-uploaded code). Validate before any I/O.
            match (wasm.is_some(), wasm_hash.is_some()) {
                (true, true) => return Err("specify only one of --wasm or --wasm-hash".into()),
                (false, false) => {
                    return Err("provide either --wasm <FILE> or --wasm-hash <HASH>".into())
                }
                _ => {}
            }
            // --deny-breaking diffs two WASM binaries, so it needs the new WASM
            // file; it is meaningless on the create-only --wasm-hash path.
            if deny_breaking && wasm_hash.is_some() {
                return Err(
                    "--deny-breaking compares WASM binaries and cannot be used with --wasm-hash"
                        .into(),
                );
            }
            if wasm_hash.is_some() && (dry_run || show_address) {
                return Err(
                    "--dry-run and --show-address are not supported with --wasm-hash".into(),
                );
            }

            // Local helper: parse 40-character hex into a 20-byte salt; validate strictly
            fn parse_salt_hex(s: &str) -> Result<[u8; 20], String> {
                let sh = s.trim();
                if sh.len() != 40 {
                    return Err(format!(
                        "Invalid --salt: must be 20-byte hex (40 hex chars), got length {}",
                        sh.len()
                    ));
                }
                if let Some(pos) = sh.chars().position(|c| !c.is_ascii_hexdigit()) {
                    return Err(format!(
                        "Invalid --salt: character at index {} is not a hex digit",
                        pos
                    ));
                }
                let mut out = [0u8; 20];
                for i in 0..20 {
                    out[i] = u8::from_str_radix(&sh[i * 2..i * 2 + 2], 16)
                        .map_err(|e| format!("Invalid --salt hex at byte {}: {}", i, e))?;
                }
                Ok(out)
            }

            // Resolve network config FIRST for safety guard
            let network_config = resolve_network_config(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            )?;

            // Determine network from passphrase
            let network = match network_config.passphrase.as_str() {
                "Test SDF Network ; September 2015" => sdkt_xdr::sign::Network::Testnet,
                "Public Global Stellar Network ; September 2015" => {
                    sdkt_xdr::sign::Network::Mainnet
                }
                "Test SDF Future Network ; October 2022" => sdkt_xdr::sign::Network::Futurenet,
                other => sdkt_xdr::sign::Network::Custom(other.to_string()),
            };

            // Apply network safety guard BEFORE loading identity
            let network_is_explicit = net.rpc_url.is_some()
                || net.network_passphrase.is_some()
                || net.network_profile.is_some();
            if let Err(e) = sdkt_core::guard_mutating_network(&network_config, network_is_explicit)
            {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }

            // Parse and validate salt (if provided) BEFORE identity lookup (fail fast on bad input)
            let salt_bytes = salt.as_ref().map(|s| parse_salt_hex(s)).transpose()?;

            // Parse and validate constructor arguments (fail fast on bad input)
            let parsed_args = parse_typed_args(&arg, false)?;
            sdkt_xdr::parse_scval_args(&parsed_args)
                .map_err(|e| format!("Invalid constructor argument: {}", e))?;

            // Read the new WASM before looking up the identity. This preserves
            // the fail-fast behavior for a missing or unreadable file on the
            // full-deploy path; create-only recovery does not need a file.
            let preloaded_wasm = wasm
                .as_ref()
                .map(|path| {
                    fs::read(path).map_err(|e| format!("Error reading WASM file {}: {}", path, e))
                })
                .transpose()?;

            // The upgrade-safety check only needs the two local WASM files and
            // must run before identity lookup so incompatible upgrades fail
            // deterministically even without a configured identity.
            if deny_breaking {
                let baseline = old_wasm.as_ref().ok_or_else(|| {
                    "The --deny-breaking flag requires --old-wasm <deployed.wasm> (the currently deployed contract)".to_string()
                })?;
                let old_bytes = fs::read(baseline)
                    .map_err(|e| format!("Failed to read OLD WASM '{}': {}", baseline, e))?;
                let new_bytes = preloaded_wasm
                    .as_ref()
                    .expect("full-deploy path preloads the new WASM");
                match sdkt_wasm::upgrade_safety_wasm(&old_bytes, new_bytes) {
                    Ok(verdict) => {
                        if !verdict.compatible {
                            eprintln!("Deployment aborted: upgrade is NOT backwards-compatible.");
                            print_upgrade_verdict(&verdict);
                            process::exit(1);
                        }
                        eprintln!(
                            "Upgrade-safety check passed: deployment is backwards-compatible."
                        );
                    }
                    Err(e) => {
                        eprintln!("Upgrade-safety check failed to compute verdict: {}", e);
                        process::exit(1);
                    }
                }
            }

            // Load identity for signing (shared by both code sources). The
            // `--identity` flag defaults to the reserved "default" sentinel, so
            // resolve it to the configured default identity.
            let identity_store = sdkt_storage::IdentityStore::new()
                .map_err(|e| format!("Failed to access identity store: {}", e))?;
            let identity_obj = identity_store
                .resolve_signing_identity(&identity)
                .map_err(|e| format!("Failed to resolve signing identity: {}", e))?;
            let signing_key = identity_store
                .load_signing_key(&identity_obj.name)
                .map_err(|e| {
                    format!(
                        "Failed to load signing key for '{}': {}",
                        identity_obj.name, e
                    )
                })?;
            let signer = sdkt_xdr::sign::Ed25519Signer::from_seed(&signing_key.to_bytes());

            let client = SorobanRpcClient::from_config(&network_config);
            // Source account is the identity's public key
            let source_account = identity_obj.public_key.clone();

            let outcome_result = if let Some(hash) = wasm_hash.as_ref() {
                // Create-only: deploy from already-uploaded code; no upload step.
                sdkt_rpc::deploy_contract_from_hash(
                    &client,
                    hash,
                    &source_account,
                    &signer,
                    network,
                    salt_bytes,
                    parsed_args,
                )
                .await
            } else {
                // Full deploy from a WASM file (present per the mutual-exclusion
                // check above).
                let wasm_bytes = preloaded_wasm
                    .as_ref()
                    .expect("full-deploy path preloads the WASM");

                // Contract IDs are deterministic. Calculate the prediction
                // only for the full-WASM path; hash-only recovery already
                // delegates its address handling to the RPC helper.
                let prediction_salt = if show_address || dry_run {
                    Some(salt_bytes.unwrap_or_else(sdkt_rpc::deploy::generate_salt))
                } else {
                    salt_bytes
                };
                let predicted = if let Some(prediction_salt) = prediction_salt {
                    let wasm_digest: [u8; 32] = Sha256::digest(wasm_bytes).into();
                    let contract_id = sdkt_xdr::derive_contract_id(
                        &network.network_id(),
                        &source_account,
                        &prediction_salt,
                        &wasm_digest,
                    )
                    .map_err(|e| format!("Failed to derive predicted contract ID: {}", e))?;
                    Some((contract_id, hex::encode(wasm_digest), prediction_salt))
                } else {
                    None
                };
                if let Some((contract_id, wasm_hash, prediction_salt)) = &predicted {
                    if dry_run {
                        if fmt == OutputFormat::Json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "status": "dry_run",
                                    "contractId": contract_id,
                                    "wasmHash": wasm_hash,
                                    "salt": hex::encode(prediction_salt),
                                    "submitted": false,
                                })
                            );
                        } else {
                            println!("Predicted Contract ID: {}", contract_id);
                            println!("WASM Hash: {}", wasm_hash);
                            println!("Salt: {}", hex::encode(prediction_salt));
                            println!("No transactions submitted.");
                        }
                        return Ok(());
                    }
                    eprintln!("Predicted Contract ID: {}", contract_id);
                    eprintln!("WASM Hash: {}", wasm_hash);
                    eprintln!("Salt: {}", hex::encode(prediction_salt));
                }
                sdkt_rpc::deploy_contract_with_args(
                    &client,
                    wasm_bytes,
                    &source_account,
                    &signer,
                    network,
                    prediction_salt,
                    parsed_args,
                )
                .await
            };

            match outcome_result {
                Ok(outcome) => match &outcome {
                    sdkt_rpc::DeployOutcome::Success(res) => {
                        if fmt == OutputFormat::Json {
                            println!("{}", sdkt_rpc::format_json(res));
                        } else {
                            println!("{}", sdkt_rpc::format_pretty(res));
                        }
                    }
                    sdkt_rpc::DeployOutcome::Partial(p) => {
                        if fmt == OutputFormat::Json {
                            println!(
                                r#"{{"status":"partial","wasmHash":"{}","uploadHash":"{}","error":"{}"}}"#,
                                p.wasm_hash, p.upload_hash, p.error
                            );
                        } else {
                            eprintln!("Partial deployment: upload succeeded but create failed");
                            eprintln!("  WASM Hash: {}", p.wasm_hash);
                            eprintln!("  Upload TX: {}", p.upload_hash);
                            eprintln!("  Error: {}", p.error);
                        }
                        process::exit(1);
                    }
                    sdkt_rpc::DeployOutcome::Failure(e) => {
                        eprintln!("Deployment failed: {}", e);
                        process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("Deployment error: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Build { format } => {
            let fmt = parse_format_str(&format);
            let config = load_config();
            match sdkt_core::build::build_workspace(&config) {
                Ok(results) => {
                    if fmt == OutputFormat::Json {
                        // Deterministic, additive machine-readable result: the
                        // success flag plus one entry per built artifact.
                        // Existing pretty output is unchanged.
                        let artifacts: Vec<serde_json::Value> = results
                            .iter()
                            .map(|res| {
                                serde_json::json!({
                                    "alias": res.alias,
                                    "path": res.path,
                                    "wasm_artifact": res.wasm_artifact.display().to_string(),
                                })
                            })
                            .collect();
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "success": true,
                                "artifacts": artifacts,
                            }))?
                        );
                    } else {
                        println!("✓ Workspace built successfully");
                        for res in results {
                            println!("  ✓ {} -> {}", res.alias, res.wasm_artifact.display());
                        }
                    }
                }
                Err(e) => {
                    if fmt == OutputFormat::Json {
                        // Error envelope on stdout with a non-zero exit, so an
                        // agent can parse the failure deterministically instead
                        // of reading stderr text. Mirrors `package validate`.
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "success": false,
                                "error": e.to_string(),
                            }))?
                        );
                        std::process::exit(1);
                    }
                    eprintln!("Error building workspace: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Call {
            contract_id,
            function,
            args,
            args_json,
            format,
            abi,
            abi_contract,
            net,
        } => {
            use sdkt_rpc::simulate_transaction;
            use sdkt_xdr::scval_from_base64;

            // `--abi` (local WASM) and `--abi-contract` (on-chain WASM) are
            // mutually exclusive. Validate before any network I/O so the
            // error is deterministic regardless of RPC reachability.
            if let Err(e) =
                commands::abi::check_abi_mutual_exclusion(abi.as_ref(), abi_contract.as_ref())
            {
                return Err(e.into());
            }

            let fmt = parse_format_str(&format);

            // Resolve network config
            let network_config = resolve_network_config(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            )?;

            // Parse typed args into base64-encoded ScVal (reuse existing parser)
            let mut parsed_args = parse_typed_args(&args, true)?;
            for json in &args_json {
                parsed_args.extend(sdkt_xdr::json_args_to_base64(json).map_err(|e| e.to_string())?);
            }

            // Read-only: use a zero-fake sequence + arbitrary fee + identity placeholder
            // This tx will NOT be signed or submitted — only simulated.
            let params = sdkt_xdr::builder::InvokeTransactionParams {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
                sequence: 0,
                fee: 0,
                contract_id: contract_id.clone(),
                function: function.clone(),
                args: parsed_args,
                memo: None,
            };

            let envelope = sdkt_xdr::builder::build_invoke_transaction(&params)?;

            let client = SorobanRpcClient::from_config(&network_config);
            match simulate_transaction(&client, &envelope).await {
                Ok(resp) => {
                    if let Some(err) = &resp.error {
                        return Err(format!("simulation error: {err}").into());
                    }

                    // Extract raw result from simulation
                    let result_raw = resp
                        .results
                        .first()
                        .map(|r| r.xdr.clone())
                        .unwrap_or_default();

                    // Load ABI spec from one of two mutually exclusive sources:
                    // a local WASM file (--abi) or a deployed contract's
                    // on-chain WASM fetched via RPC (--abi-contract).
                    let abi_spec = commands::abi::resolve_abi_spec(
                        abi.as_ref(),
                        abi_contract.as_ref(),
                        &client,
                    )
                    .await
                    .map_err(Box::<dyn std::error::Error>::from)?;

                    // Decode result with ABI if available
                    let (result_display, result_decoded) = if result_raw.is_empty() {
                        ("(void)".into(), None)
                    } else if let Some(spec) = &abi_spec {
                        // Parse ScVal from base64 XDR
                        match scval_from_base64(&result_raw) {
                            Some(scval) => {
                                // Find the function in the spec
                                let func = spec.functions.iter().find(|f| f.name == function);
                                if func.is_none() {
                                    eprintln!("warning: function '{function}' not found in ABI — showing raw result");
                                }
                                let decoded =
                                    sdkt_xdr::abi_decode::decode_with_abi(spec, &scval, None);
                                (decoded.label.clone(), Some(decoded))
                            }
                            None => {
                                eprintln!(
                                    "warning: could not parse result ScVal — showing raw result"
                                );
                                (result_raw.clone(), None)
                            }
                        }
                    } else {
                        (result_raw.clone(), None)
                    };

                    // Format events
                    let events_json: Vec<String> =
                        resp.events.iter().map(|e| e.to_string()).collect();

                    if fmt == OutputFormat::Json {
                        // Build JSON: always include raw result
                        let mut json_obj = serde_json::json!({
                            "contract": contract_id,
                            "function": function,
                            "result": result_display,
                            "events": events_json,
                        });

                        // If ABI was used and decoding succeeded, include decoded info
                        if let Some(decoded) = &result_decoded {
                            json_obj["result_raw"] = serde_json::json!(result_raw);
                            json_obj["decoded"] = serde_json::json!({
                                "raw": decoded.raw,
                                "label": decoded.label,
                                "matched_type": decoded.matched_type,
                                "fields": decoded.fields,
                            });
                        }

                        println!("{}", serde_json::to_string(&json_obj)?);
                    } else {
                        // Pretty output
                        println!("Contract:  {}", contract_id);
                        println!("Function:  {}", function);
                        println!("Result:    {}", result_display);

                        if !events_json.is_empty() {
                            println!("Events:");
                            for ev in &events_json {
                                println!("  {}", ev);
                            }
                        }
                    }
                }
                Err(e) => return Err(format!("RPC simulation failed: {e}").into()),
            }
        }
        Commands::Invoke {
            contract_id,
            function,
            args,
            args_json,
            identity,
            format,
            no_wait,
            build_only,
            net,
        } => {
            let fmt = parse_format_str(&format);

            // 1. Resolve network (mutating operation → mainnet safety guard).
            let network_config = resolve_network_config(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            )?;
            let network_is_explicit =
                network_is_explicit(&net.rpc_url, &net.network_passphrase, &net.network_profile);
            if let Err(e) = sdkt_core::guard_mutating_network(&network_config, network_is_explicit)
            {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
            let network = match network_config.passphrase.as_str() {
                "Public Global Stellar Network ; September 2015" => Network::Mainnet,
                "Test SDF Future Network ; October 2022" => Network::Futurenet,
                other => Network::Custom(other.to_string()),
            };

            // 2. Load the signing identity (keystore; no secret on argv). The
            //    `--identity` flag defaults to the reserved "default" sentinel,
            //    which resolves to the configured default identity.
            let identity_store = sdkt_storage::IdentityStore::new()
                .map_err(|e| format!("Failed to access identity store: {e}"))?;
            let identity_obj = identity_store
                .resolve_signing_identity(&identity)
                .map_err(|e| format!("Failed to resolve signing identity: {e}"))?;
            let signing_key = identity_store
                .load_signing_key(&identity_obj.name)
                .map_err(|e| {
                    format!(
                        "Failed to load signing key for '{}': {e}",
                        identity_obj.name
                    )
                })?;
            let signer = Ed25519Signer::from_seed(&signing_key.to_bytes());

            // 3. Parse typed args (shared parser; strict — typos must not be
            //    silently treated as pre-encoded ScVal on a state-changing path).
            let mut parsed_args = parse_typed_args(&args, true)?;
            for json in &args_json {
                parsed_args.extend(sdkt_xdr::json_args_to_base64(json).map_err(|e| e.to_string())?);
            }

            let params = InvokeTransactionParams {
                source_account: identity_obj.public_key.clone(),
                sequence: 0, // fetched by invoke_contract from the network
                fee: 0,      // computed from simulation inside invoke_contract
                contract_id: contract_id.clone(),
                function: function.clone(),
                args: parsed_args,
                memo: None,
            };

            let client = SorobanRpcClient::from_config(&network_config);
            let poll = sdkt_rpc::PollConfig::default();

            // `--build-only`: stop after the envelope is built and signed. Nothing
            // is submitted, so the prepared envelope can be inspected (or handed to
            // `sdkt tx validate` / `sdkt tx submit`) without a signed transaction
            // ever reaching the network from this path.
            if build_only {
                match sdkt_rpc::build_invoke_envelope(&client, &params, &signer, network).await {
                    Ok(res) => {
                        if fmt == OutputFormat::Json {
                            println!(
                                "{}",
                                serde_json::json!({
                                    "envelopeXdr": res.envelope_xdr,
                                    "fee": res.fee,
                                    "sequence": res.sequence,
                                    "contractId": res.contract_id,
                                    "function": res.function,
                                    "submitted": false,
                                })
                            );
                        } else {
                            println!("Transaction Envelope (NOT submitted):");
                            println!("  Contract: {}", res.contract_id);
                            println!("  Function: {}", res.function);
                            println!("  Fee:      {} stroops", res.fee);
                            println!("  Sequence: {}", res.sequence);
                            println!("{}", res.envelope_xdr);
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("Error building invoke transaction: {}", e);
                        process::exit(1);
                    }
                }
            }

            match sdkt_rpc::invoke_contract(&client, &params, &signer, network, &poll, !no_wait)
                .await
            {
                Ok(res) => {
                    if fmt == OutputFormat::Json {
                        let mut json = serde_json::json!({
                            "hash": res.hash,
                            "status": res.status,
                            "contractId": res.contract_id,
                            "function": res.function,
                            "fee": res.fee,
                            "resultXdr": res.result_xdr,
                            "errorCode": res.error_code,
                            "errorResultXdr": res.error_result_xdr,
                            "diagnosticEvents": res.diagnostic_events,
                        });
                        if !res.events.is_empty() {
                            json["events"] = serde_json::json!(res.events);
                        }
                        println!("{}", json);
                    } else {
                        println!("Invocation Result:");
                        println!("  Contract: {}", res.contract_id);
                        println!("  Function: {}", res.function);
                        println!("  Hash:     {}", res.hash);
                        println!("  Status:   {}", res.status);
                        println!("  Fee:      {} stroops", res.fee);
                        if let Some(ledger) = &res.result_xdr {
                            println!("  Result XDR: {}", ledger);
                        }
                        if !res.events.is_empty() {
                            println!("  Events:");
                            for event in &res.events {
                                println!("    {}", event);
                            }
                        }
                        if let Some(code) = &res.error_code {
                            println!("  Error:    {}", code);
                        }
                        if let Some(xdr) = &res.error_result_xdr {
                            println!("  Error Result XDR: {}", xdr);
                        }
                        for ev in &res.diagnostic_events {
                            println!("  Diagnostic: {}", ev);
                        }
                    }
                    if res.status != "SUCCESS" && !(no_wait && res.status == "PENDING") {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error invoking contract: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Lock { action } => match action {
            LockCommand::Generate { format } => {
                let fmt = parse_format_str(&format);
                let config = load_config();
                match sdkt_core::lock::generate_lock(Path::new("."), &config) {
                    Ok(lock) => match sdkt_core::lock::write_lock(Path::new("."), &lock) {
                        Ok(path) => {
                            if fmt != OutputFormat::Json {
                                println!("✓ Wrote {}", path.display());
                                println!("{}", sdkt_core::lock::lock_to_toml(&lock).unwrap());
                            } else {
                                println!(
                                    "{}",
                                    serde_json::to_string_pretty(&serde_json::json!({
                                        "lock_file": path.display().to_string(),
                                        "version": lock.version,
                                        "deploy_order": lock.deploy_order,
                                        "contracts": lock.contracts,
                                    }))
                                    .unwrap()
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("Error generating lock: {}", e);
                            std::process::exit(1);
                        }
                    },
                    Err(e) => {
                        eprintln!("Error generating lock: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            LockCommand::Verify { format } => {
                let fmt = parse_format_str(&format);
                let config = load_config();
                let base = Path::new(".");
                let report = sdkt_core::lock::verify_lock(base, &config);
                let dep_report = sdkt_core::lock::verify_dependencies(base, &config);
                if fmt != OutputFormat::Json {
                    // --- Contract artifact verification (existing behavior) ---
                    if report.present {
                        if report.consistent {
                            println!("✓ sdkt.lock is consistent with current artifacts");
                        } else {
                            if !report.mismatched.is_empty() {
                                println!(
                                    "⚠ sdkt.lock drift — artifact hash changed for: {}",
                                    report.mismatched.join(", ")
                                );
                            }
                            if !report.missing_in_lock.is_empty() {
                                println!(
                                    "⚠ sdkt.lock missing entries for: {}",
                                    report.missing_in_lock.join(", ")
                                );
                            }
                        }
                    } else {
                        println!("⚠ No sdkt.lock found; run `sdkt build` to generate one");
                    }

                    // --- Package dependency verification () ---
                    if dep_report.present {
                        if dep_report.consistent {
                            println!("✓ package dependencies verified");
                        } else {
                            for m in &dep_report.mismatches {
                                println!(
                                    "⚠ dependency '{}' drift ({:?}): {}",
                                    m.name, m.kind, m.detail
                                );
                            }
                        }
                    } else if !config.dependencies.is_empty() {
                        println!("⚠ No sdkt.lock present; package dependencies unverified");
                        for m in &dep_report.mismatches {
                            println!(
                                "⚠ dependency '{}' not locked ({:?}): {}",
                                m.name, m.kind, m.detail
                            );
                        }
                    }
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "present": report.present,
                            "consistent": report.consistent,
                            "mismatched": report.mismatched,
                            "missing_in_lock": report.missing_in_lock,
                            "dependencies": {
                                "present": dep_report.present,
                                "consistent": dep_report.consistent,
                                "checked": dep_report.checked,
                                "mismatches": dep_report.mismatches.iter().map(|m| serde_json::json!({
                                    "name": m.name,
                                    "kind": format!("{:?}", m.kind),
                                    "detail": m.detail,
                                })).collect::<Vec<_>>(),
                            },
                        }))
                        .unwrap()
                    );
                }
            }
            LockCommand::Show { format } => {
                let fmt = parse_format_str(&format);
                match sdkt_core::lock::read_lock(Path::new(".")) {
                    Ok(lock) => {
                        if fmt != OutputFormat::Json {
                            println!("{}", sdkt_core::lock::lock_to_toml(&lock).unwrap());
                        } else {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "version": lock.version,
                                    "deploy_order": lock.deploy_order,
                                    "contracts": lock.contracts,
                                }))
                                .unwrap()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("Error reading lock: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        },
        Commands::Package { action } => match action {
            PackageCommand::Validate { format } => {
                let fmt = parse_format_str(&format);
                let config = load_config();
                let base = Path::new(".");
                let result = sdkt_core::package::validate_manifest(base, &config);
                if let Some(pkg) = &config.package {
                    if fmt != OutputFormat::Json {
                        println!("Package: {}", pkg.name.as_deref().unwrap_or("(unnamed)"));
                        println!("Version: {}", pkg.version.as_deref().unwrap_or("(none)"));
                        if let Some(d) = &pkg.description {
                            println!("Description: {}", d);
                        }
                        println!("Dependencies: {}", config.dependencies.len());
                    }
                } else if fmt != OutputFormat::Json {
                    println!("No [package] section present.");
                }
                match result {
                    Ok(()) => {
                        if fmt != OutputFormat::Json {
                            println!("Package manifest is valid");
                        } else {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({ "valid": true }))
                                    .unwrap()
                            );
                        }
                    }
                    Err(e) => {
                        if fmt != OutputFormat::Json {
                            eprintln!("Package validation failed: {}", e);
                        } else {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(
                                    &serde_json::json!({ "valid": false, "error": e.to_string() })
                                )
                                .unwrap()
                            );
                        }
                        std::process::exit(1);
                    }
                }
            }
            PackageCommand::Fetch { format, force } => {
                let fmt = parse_format_str(&format);
                let config = load_config();
                // Validation first, so a malformed manifest never triggers a fetch.
                let base = Path::new(".");
                if let Err(e) = sdkt_core::package::validate_manifest(base, &config) {
                    eprintln!("Package validation failed: {}", e);
                    std::process::exit(1);
                }

                // Deterministic cache at `.sdkt-cache` (workspace-local).
                // Use an absolute path so `git clone <url> <checkout>` is not
                // resolved relative to the fetcher's working dir (which would
                // double the path). Fall back to the relative form only if the
                // current dir cannot be resolved.
                let cache = std::env::current_dir()
                    .map(|c| c.join(".sdkt-cache"))
                    .unwrap_or_else(|_| std::path::PathBuf::from(".sdkt-cache"));
                let fetcher = sdkt_core::fetch::GitFetcher::new(cache);

                if config.dependencies.is_empty() {
                    if fmt != OutputFormat::Json {
                        println!("No dependencies to fetch.");
                    }
                    return Ok(());
                }

                let mut fetched = Vec::new();
                for (name, dep) in &config.dependencies {
                    let outcome = if dep.git.is_some() {
                        fetcher.fetch(name, dep, force)
                    } else {
                        sdkt_core::fetch::PathResolver.fetch(name, dep, force)
                    };
                    match outcome {
                        Ok(o) => {
                            fetched.push(o);
                        }
                        Err(e) => {
                            eprintln!("Failed to fetch '{}': {}", name, e);
                            std::process::exit(1);
                        }
                    }
                }

                // — record resolved dependency state into sdkt.lock so
                // `sdkt lock verify` can enforce reproducibility offline. We
                // update the lock in place (preserving contract artifacts) when
                // one already exists; otherwise we generate a fresh lock.
                {
                    use sdkt_core::lock::LockFile;
                    let mut lock = sdkt_core::lock::read_lock(base).unwrap_or_else(|_| LockFile {
                        version: sdkt_core::lock::LOCK_VERSION,
                        deploy_order: vec![],
                        contracts: vec![],
                        dependencies: vec![],
                    });
                    // Single source of truth shared with `sdkt package update`.
                    lock.dependencies =
                        sdkt_core::lock::lock_dependencies_resolved(base, &config, &fetched);
                    if let Err(e) = sdkt_core::lock::write_lock(base, &lock) {
                        eprintln!("Warning: could not write sdkt.lock: {}", e);
                    }
                }

                if fmt != OutputFormat::Json {
                    for o in &fetched {
                        let rev = if o.resolved_rev.is_empty() {
                            "(local)".to_string()
                        } else {
                            o.resolved_rev.chars().take(12).collect()
                        };
                        println!(
                            "Fetched '{}' -> {} @ {}",
                            o.name,
                            o.local_path.display(),
                            rev
                        );
                    }
                    println!("Fetched {} dependenc(y/ies).", fetched.len());
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "fetched": fetched
                                .iter()
                                .map(|o| serde_json::json!({
                                    "name": o.name,
                                    "local_path": o.local_path.display().to_string(),
                                    "resolved_rev": o.resolved_rev,
                                    "already_present": o.already_present,
                                }))
                                .collect::<Vec<_>>()
                        }))
                        .unwrap()
                    );
                }
            }
            PackageCommand::Update {
                format,
                check,
                dry_run,
            } => {
                let fmt = parse_format_str(&format);
                let config = load_config();
                let base = Path::new(".");

                // Build the update plan (read-only: resolves available commits via
                // git ls-remote; never fetches, never writes the lock).
                let plan = match sdkt_core::sync::plan_updates(base, &config) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        std::process::exit(1);
                    }
                };

                if fmt == OutputFormat::Json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "checked": plan.checked,
                            "updated": plan.updated,
                            "unchanged": plan.unchanged,
                            "changes": plan.changes.iter().map(|c| serde_json::json!({
                                "name": c.name,
                                "source": c.source,
                                "status": format!("{:?}", c.status),
                                "old_commit": c.old_commit,
                                "new_commit": c.new_commit,
                                "detail": c.detail,
                            })).collect::<Vec<_>>(),
                        }))
                        .unwrap()
                    );
                } else if check {
                    // --check: report available updates only; exit 0 (errors are
                    // listed, but a non-zero exit is reserved for hard failures
                    // which already surfaced above via process::exit).
                    println!("Checking dependencies...");
                    let mut available = 0;
                    for c in &plan.changes {
                        match c.status {
                            sdkt_core::sync::UpdateStatus::Updated => {
                                available += 1;
                                println!("↑ {} has an update", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Pinned => {
                                println!("✓ {} pinned (rev)", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Constraint => {
                                println!("⚠ {} constraint unsatisfied", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Error => {
                                println!("✗ {} error: {}", c.name, c.detail);
                            }
                            _ => {
                                println!("✓ {} unchanged", c.name);
                            }
                        }
                    }
                    if available > 0 {
                        println!(
                            "{} update(s) available. Run `sdkt package update`.",
                            available
                        );
                    } else {
                        println!("All dependencies up to date.");
                    }
                } else if dry_run {
                    // --dry-run: preview the changes without modifying anything.
                    let mut would = 0;
                    println!("Would update:");
                    for c in &plan.changes {
                        if c.status == sdkt_core::sync::UpdateStatus::Updated {
                            would += 1;
                            let old = &c.old_commit;
                            let new = &c.new_commit;
                            println!("  {}", c.name);
                            println!(
                                "    old commit: {}",
                                if old.is_empty() {
                                    "(none)".to_string()
                                } else {
                                    old.chars().take(12).collect()
                                }
                            );
                            println!(
                                "    new commit: {}",
                                if new.is_empty() {
                                    "(none)".to_string()
                                } else {
                                    new.chars().take(12).collect()
                                }
                            );
                        } else if c.status == sdkt_core::sync::UpdateStatus::Pinned {
                            println!("  {} (pinned, skip)", c.name);
                        } else if c.status == sdkt_core::sync::UpdateStatus::Constraint {
                            println!("  {} (constraint unsatisfied, skip)", c.name);
                        } else if c.status == sdkt_core::sync::UpdateStatus::Error {
                            println!("  {} (error: {})", c.name, c.detail);
                        } else {
                            println!("  {} (unchanged)", c.name);
                        }
                    }
                    if would > 0 {
                        println!("Lock would change.");
                    } else {
                        println!("Nothing to change.");
                    }
                } else {
                    // Real apply: refresh cache + rewrite lock (the plan is already
                    // computed; apply_updates performs the actual fetch).
                    let report = match sdkt_core::sync::apply_updates(base, &config) {
                        Ok((r, _lock)) => r,
                        Err(e) => {
                            eprintln!("Error updating dependencies: {}", e);
                            std::process::exit(1);
                        }
                    };
                    println!("Checking dependencies...");
                    for c in &report.changes {
                        match c.status {
                            sdkt_core::sync::UpdateStatus::Updated => {
                                println!("↑ {} updated", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Pinned => {
                                println!("✓ {} pinned (rev)", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Constraint => {
                                println!("⚠ {} constraint unsatisfied", c.name);
                            }
                            sdkt_core::sync::UpdateStatus::Error => {
                                println!("✗ {} error: {}", c.name, c.detail);
                            }
                            _ => {
                                println!("✓ {} unchanged", c.name);
                            }
                        }
                    }
                    if report.updated > 0 {
                        println!("Updated:");
                        println!("{} dependency", report.updated);
                        println!("Lock refreshed.");
                    } else {
                        println!("Nothing to update.");
                    }
                }
            }
            PackageCommand::Pack { out, format } => {
                let base = Path::new(".");
                let out_dir = Path::new(&out);
                std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
                    eprintln!("Error creating output dir {}: {}", out_dir.display(), e);
                    std::process::exit(1);
                });
                match sdkt_core::package::pack(base, out_dir, &format) {
                    Ok(bundle) => {
                        println!("Packed {} v{}", bundle.name, bundle.version);
                        println!("  format:  {}", bundle.format);
                        println!("  artifact: {}", bundle.out_path);
                        println!("  lock sha256: {}", bundle.lock_sha256);
                        println!("  dependencies bundled: {}", bundle.entries.len());
                    }
                    Err(e) => {
                        eprintln!("Error packing package: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            PackageCommand::Publish { dry_run, broadcast } => {
                let base = Path::new(".");
                let config = load_config();
                // `--broadcast` is opt-in; defines no registry source, so it is
                // rejected (stays offline / dry-run only). No network is ever used.
                if broadcast {
                    eprintln!(
                        "Error: `--broadcast` requires a configured registry source; none is defined in packaging."
                    );
                    std::process::exit(1);
                }
                let _ = dry_run; // default true; readiness is always read-only here.
                match sdkt_core::package::publish_plan(base, &config) {
                    Ok(readiness) => {
                        println!("Publish readiness check:");
                        for (name, ok, detail) in &readiness.checks {
                            let mark = if *ok { "✓" } else { "✗" };
                            println!("  {} {} — {}", mark, name, detail);
                        }
                        if readiness.ready {
                            println!("Package is ready to publish (dry-run).");
                        } else {
                            eprintln!("Package is NOT ready to publish. Fix the issues above.");
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error evaluating publish readiness: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        },
        Commands::Project { action, net } => match action {
            ProjectCommand::Status { format } => {
                let fmt = parse_format_str(&format);
                let network_config = match resolve_network_config(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                ) {
                    Ok(config) => config,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                };
                let network_key = sdkt_core::deployment::network_key(
                    if net.network_passphrase.is_some() {
                        None
                    } else {
                        net.network_profile.as_deref()
                    },
                    &network_config.passphrase,
                );
                let record_path = Path::new(sdkt_core::deployment::DEPLOYMENT_RECORD_FILE);
                let record_file =
                    match sdkt_core::deployment::DeploymentRecordFile::read(record_path) {
                        Ok(file) => file,
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            process::exit(1);
                        }
                    };
                let mut deployments: Vec<_> = record_file
                    .records_for(&network_key)
                    .into_iter()
                    .flat_map(|records| records.iter())
                    .collect();
                deployments.sort_by_key(|(alias, _)| *alias);

                if fmt == OutputFormat::Json {
                    let deployments: Vec<_> = deployments
                        .into_iter()
                        .map(|(alias, record)| {
                            serde_json::json!({
                                "alias": alias,
                                "contract_id": record.contract_id,
                                "wasm_hash": record.wasm_hash,
                                "network": record.network,
                                "timestamp": record.timestamp,
                                "salt": record.salt,
                            })
                        })
                        .collect();
                    let status = if deployments.is_empty() {
                        "no_deployments"
                    } else {
                        "deployed"
                    };
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "status": status,
                            "network": network_key,
                            "deployments": deployments,
                        }))
                        .unwrap()
                    );
                } else if deployments.is_empty() {
                    println!("No deployments recorded for network '{}'.", network_key);
                } else {
                    println!("Project deployment status (network: {}):", network_key);
                    for (alias, record) in deployments {
                        println!("  {}", alias);
                        println!("    Contract ID: {}", record.contract_id);
                        println!("    WASM hash:   {}", record.wasm_hash);
                        println!("    Recorded network: {}", record.network);
                        println!("    Timestamp:  {}", record.timestamp);
                        if let Some(salt) = &record.salt {
                            println!("    Salt:       {}", salt);
                        }
                    }
                }
            }
            ProjectCommand::Deploy {
                salt,
                skip_deployed,
                identity,
                format,
            } => {
                let fmt = parse_format_str(&format);
                // Validate the salt before any network or identity work (fail fast).
                let salt_bytes = salt.as_deref().map(parse_salt_hex).transpose()?;
                let config = load_config();

                // 4.1 — advisory lock check. If an `sdkt.lock` exists, warn
                // (non-fatally) when it has drifted from the current artifacts.
                // This never blocks deployment; it simply surfaces a stale-lock
                // signal so operators can re-run `sdkt build` if needed.
                let lock_report = sdkt_core::lock::verify_lock(Path::new("."), &config);
                if lock_report.present && !lock_report.consistent && fmt != OutputFormat::Json {
                    eprintln!("⚠ Warning: sdkt.lock is stale — run `sdkt build` to refresh it.");
                }

                let client = resolve_rpc_client_mutating(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                // Resolve the effective network once so the record-file scope
                // and the signed network match the same passphrase.
                let network_config = resolve_network_config(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                )?;

                // Determine network from passphrase
                let network = match network_config.passphrase.as_str() {
                    "Test SDF Network ; September 2015" => sdkt_xdr::sign::Network::Testnet,
                    "Public Global Stellar Network ; September 2015" => {
                        sdkt_xdr::sign::Network::Mainnet
                    }
                    "Test SDF Future Network ; October 2022" => sdkt_xdr::sign::Network::Futurenet,
                    other => sdkt_xdr::sign::Network::Custom(other.to_string()),
                };

                // Per-network scope for the deployment record. When
                // --network-passphrase is explicitly set it overrides whatever
                // profile was resolved, so the record key must be derived from
                // the resolved passphrase rather than the profile name.
                // Otherwise use the profile name (if any) for a stable,
                // human-readable key.
                let network_key = sdkt_core::deployment::network_key(
                    if net.network_passphrase.is_some() {
                        None
                    } else {
                        net.network_profile.as_deref()
                    },
                    &network_config.passphrase,
                );
                let record_path = Path::new(sdkt_core::deployment::DEPLOYMENT_RECORD_FILE);
                let mut record_file =
                    match sdkt_core::deployment::DeploymentRecordFile::read(record_path) {
                        Ok(file) => file,
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            process::exit(1);
                        }
                    };

                // Resolve and validate the selected identity before entering
                // the deployment loop. This ensures an unknown --identity fails
                // immediately — even when --skip-deployed would skip every
                // contract — and avoids a redundant store lookup per iteration.
                let identity_store = sdkt_storage::IdentityStore::new()
                    .map_err(|e| format!("Failed to access identity store: {}", e))?;
                let identity_obj = identity_store
                    .resolve_signing_identity(&identity)
                    .map_err(|e| format!("Failed to resolve signing identity: {}", e))?;
                let signing_key = identity_store
                    .load_signing_key(&identity_obj.name)
                    .map_err(|e| format!("Failed to load signing key: {}", e))?;
                let signer = sdkt_xdr::sign::Ed25519Signer::from_seed(&signing_key.to_bytes());
                let source_account = identity_obj.public_key.clone();

                // Write the record file and return whether it succeeded.
                // On failure, print the contract_id so the operator can
                // recover without repeating the deployment.
                let persist_records =
                    |record_file: &sdkt_core::deployment::DeploymentRecordFile,
                     contract_id: Option<&str>|
                     -> bool {
                        match record_file.write(record_path) {
                            Ok(()) => true,
                            Err(e) => {
                                if let Some(id) = contract_id {
                                    eprintln!(
                                    "✗ Failed to write deployment record: {e}\n  \
                                     Contract ID: {id} — save this value to recover without re-deploying."
                                );
                                } else {
                                    eprintln!("✗ Failed to write deployment record: {e}");
                                }
                                false
                            }
                        }
                    };

                match sdkt_core::project::resolve_project(&config) {
                    Ok(resolved) => {
                        // Contracts skipped via --skip-deployed (existing on-chain
                        // record) count toward the resolved total for reporting.
                        let total_planned = resolved.len();

                        if fmt != OutputFormat::Json {
                            println!(
                                "✓ Project dependency graph resolved. Deploying {} contract(s).",
                                total_planned
                            );
                        }

                        let mut results = std::collections::HashMap::new();
                        let mut failure: Option<String> = None;

                        for contract in resolved {
                            // --skip-deployed: honor the record only when the
                            // recorded contract still exists on-chain. A record
                            // file entry alone does not skip — the ledger is the
                            // source of truth.
                            if skip_deployed {
                                if let Some(record) =
                                    record_file.record_for(&network_key, &contract.alias)
                                {
                                    match sdkt_rpc::storage::contract_exists(
                                        &client,
                                        &record.contract_id,
                                    )
                                    .await
                                    {
                                        Ok(true) => {
                                            if fmt != OutputFormat::Json {
                                                println!(
                                                    "  ✓ '{}' already deployed at {} (--skip-deployed)",
                                                    contract.alias, record.contract_id
                                                );
                                            }
                                            results.insert(
                                                contract.alias.clone(),
                                                record.contract_id.clone(),
                                            );
                                            continue;
                                        }
                                        Ok(false) => {
                                            if fmt != OutputFormat::Json {
                                                eprintln!(
                                                    "    ⚠ Recorded contract for '{}' is no longer on-chain; re-deploying.",
                                                    contract.alias
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            failure = Some(format!(
                                                "Failed to verify recorded contract for '{}': {}",
                                                contract.alias, e
                                            ));
                                            eprintln!("    ✗ {}", failure.as_ref().unwrap());
                                            break;
                                        }
                                    }
                                }
                            }

                            if fmt != OutputFormat::Json {
                                println!(
                                    "  Deploying alias '{}' from '{}'...",
                                    contract.alias,
                                    contract.wasm_artifact.display()
                                );
                            }

                            let wasm_bytes = match fs::read(&contract.wasm_artifact) {
                                Ok(bytes) => bytes,
                                Err(e) => {
                                    failure = Some(format!(
                                        "Failed to read WASM for '{}': {}",
                                        contract.alias, e
                                    ));
                                    eprintln!("    ✗ {}", failure.as_ref().unwrap());
                                    break;
                                }
                            };

                            match sdkt_rpc::deploy_contract(
                                &client,
                                &wasm_bytes,
                                &source_account,
                                &signer,
                                network.clone(),
                                salt_bytes,
                            )
                            .await
                            {
                                Ok(sdkt_rpc::DeployOutcome::Success(res)) => {
                                    if fmt != OutputFormat::Json {
                                        println!("    ✓ Contract ID: {}", res.contract_id);
                                    }
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    // Persist the record immediately so a crash,
                                    // SIGINT, or early ? return after this point
                                    // cannot lose this contract's ID. Exit
                                    // nonzero if the write fails — the contract
                                    // is on-chain but the record is not durable.
                                    record_file.set_record(
                                        &network_key,
                                        &contract.alias,
                                        sdkt_core::deployment::DeploymentRecord {
                                            contract_id: res.contract_id.clone(),
                                            wasm_hash: res.wasm_hash.clone(),
                                            network: network_key.clone(),
                                            timestamp,
                                            salt: Some(res.salt.clone()),
                                        },
                                    );
                                    if !persist_records(&record_file, Some(&res.contract_id)) {
                                        process::exit(1);
                                    }
                                    results.insert(contract.alias, res.contract_id);
                                }
                                Ok(sdkt_rpc::DeployOutcome::Partial(p)) => {
                                    failure = Some(p.error.clone());
                                    eprintln!(
                                        "    ⚠ Partial: upload succeeded, create failed: {}",
                                        p.error
                                    );
                                    break;
                                }
                                Ok(sdkt_rpc::DeployOutcome::Failure(e)) => {
                                    failure = Some(e.clone());
                                    eprintln!("    ✗ Failed: {}", e);
                                    break;
                                }
                                Err(e) => {
                                    failure = Some(e.to_string());
                                    eprintln!("Deployment failed for '{}': {}", contract.alias, e);
                                    break;
                                }
                            }
                        }

                        if let Some(err) = failure {
                            eprintln!(
                                "⚠ Deployment record written to {} ({} of {} deployed)",
                                sdkt_core::deployment::DEPLOYMENT_RECORD_FILE,
                                results.len(),
                                total_planned
                            );
                            eprintln!("Deployment failed: {}", err);
                            process::exit(1);
                        }

                        if fmt == OutputFormat::Json {
                            let json = serde_json::json!({
                                "status": "success",
                                "contracts_deployed": results,
                            });
                            println!("{}", serde_json::to_string(&json).unwrap());
                        } else {
                            println!("✓ Project deployment complete.");
                        }
                    }
                    Err(e) => {
                        eprintln!("Error resolving project: {}", e);
                        process::exit(1);
                    }
                }
            }
        },
        Commands::Plugin { action } => match action {
            // In JSON mode stdout carries only the JSON document; warnings and
            // notes stay on stderr exactly as in pretty mode, so the output can
            // be piped straight into a parser.
            PluginAction::Init {
                name,
                force,
                format,
            } => {
                use sdkt_core::scaffold::{generate_plugin_project, PluginScaffoldConfig};

                let fmt = parse_format_str(&format);
                let scaffold_cfg = PluginScaffoldConfig {
                    name: name.clone(),
                    force,
                };

                match generate_plugin_project(&scaffold_cfg) {
                    Ok(result) => {
                        if fmt == OutputFormat::Json {
                            let json = serde_json::json!({
                                "status": "created",
                                "plugin": name,
                                "files": result.files_created,
                            });
                            println!("{}", serde_json::to_string_pretty(&json)?);
                        } else {
                            println!("✓ Created plugin rule project '{}'", name);
                            for f in &result.files_created {
                                println!("  ✓ {}", f);
                            }
                            println!(
                                "✓ Ready — in the new project directory, run: cargo build --release --features plugins"
                            );
                        }
                    }
                    Err(e) => {
                        if fmt == OutputFormat::Json {
                            let json = serde_json::json!({
                                "status": "error",
                                "message": e.to_string(),
                            });
                            println!("{}", serde_json::to_string_pretty(&json)?);
                        } else {
                            eprintln!("Error: {}", e);
                        }
                        process::exit(1);
                    }
                }
            }
            PluginAction::List { format } => {
                let fmt = parse_format_str(&format);
                let plugins = sdkt_audit::plugin_store::list();
                if fmt == OutputFormat::Json {
                    println!("{}", serde_json::to_string_pretty(&plugins)?);
                } else if plugins.is_empty() {
                    println!("No plugins installed.");
                } else {
                    for p in plugins {
                        println!("{}  v{}  ({})  {}", p.id, p.version, p.kind, p.description);
                    }
                }
            }
            PluginAction::Show { id, format } => {
                let fmt = parse_format_str(&format);
                match sdkt_audit::plugin_store::show(&id) {
                    Some(p) if fmt == OutputFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&p)?);
                    }
                    Some(p) => {
                        println!("id: {}", p.id);
                        println!("name: {}", p.name);
                        println!("version: {}", p.version);
                        println!("author: {}", p.author);
                        println!("kind: {}", p.kind);
                        println!("artifact: {}", p.artifact);
                        println!("abi: {}.{}", p.abi_major, p.abi_minor);
                        println!("description: {}", p.description);
                    }
                    None => {
                        eprintln!("Error: plugin '{}' is not installed", id);
                        process::exit(1);
                    }
                }
            }
            PluginAction::Install {
                source,
                id,
                force,
                public_key,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let opts = sdkt_audit::plugin_store::InstallOpts { id, force };
                let path = std::path::Path::new(&source);
                let is_bundle = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("sdktplugin"));
                if public_key.is_some() && !is_bundle {
                    eprintln!("Error: --public-key only applies to .sdktplugin bundles");
                    process::exit(1);
                }
                // `signed` is only known (and reported) for bundle installs.
                let result = if is_bundle {
                    let pubkey = public_key.as_deref().map(read_public_key_or_exit);
                    sdkt_audit::plugin_store::install_bundle_with_key(path, &opts, pubkey.as_ref())
                        .map(|v| (v.metadata, Some(v.signed)))
                } else {
                    sdkt_audit::plugin_store::install(path, &opts).map(|meta| (meta, None))
                };
                match result {
                    Ok((meta, signed)) => {
                        if meta.kind == "native" {
                            eprintln!(
                                    "Warning: native plugins run UNSANDBOXED. Only install from trusted sources."
                                );
                        }
                        if fmt == OutputFormat::Json {
                            let mut json =
                                serde_json::json!({ "status": "installed", "plugin": meta });
                            if let Some(signed) = signed {
                                json["signed"] = serde_json::Value::Bool(signed);
                            }
                            println!("{}", serde_json::to_string_pretty(&json)?);
                        } else {
                            println!(
                                "Installed plugin '{}' ({} v{})",
                                meta.id, meta.kind, meta.version
                            );
                            match signed {
                                Some(true) => println!("  signature: VERIFIED"),
                                Some(false) => println!("  signature: UNSIGNED"),
                                None => {}
                            }
                        }
                        if signed == Some(false) {
                            eprintln!("Note: bundle was NOT signed");
                        }
                    }
                    Err(e) => {
                        eprintln!("Error installing plugin: {}", e);
                        process::exit(1);
                    }
                }
            }
            PluginAction::Remove { id, format } => {
                let fmt = parse_format_str(&format);
                if let Err(e) = sdkt_audit::plugin_store::remove(&id) {
                    eprintln!("Error removing plugin: {}", e);
                    process::exit(1);
                }
                if fmt == OutputFormat::Json {
                    let json = serde_json::json!({ "status": "removed", "id": id });
                    println!("{}", serde_json::to_string_pretty(&json)?);
                } else {
                    println!("Removed plugin '{}' (if it was installed).", id);
                }
            }
            PluginAction::Update { id, source, format } => {
                let fmt = parse_format_str(&format);
                match sdkt_audit::plugin_store::update(&id, std::path::Path::new(&source)) {
                    Ok(meta) => {
                        if meta.kind == "native" {
                            eprintln!(
                                    "Warning: native plugins run UNSANDBOXED. Only install from trusted sources."
                                );
                        }
                        if fmt == OutputFormat::Json {
                            let json = serde_json::json!({ "status": "updated", "plugin": meta });
                            println!("{}", serde_json::to_string_pretty(&json)?);
                        } else {
                            println!("Updated plugin '{}' to v{}", meta.id, meta.version);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error updating plugin: {}", e);
                        process::exit(1);
                    }
                }
            }
            PluginAction::Pack {
                source,
                output,
                secret_key,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let src = std::path::Path::new(&source);
                let meta_path = src.join("plugin.toml");
                if !meta_path.exists() {
                    eprintln!(
                        "Error: plugin.toml not found in plugin directory '{}'",
                        source
                    );
                    process::exit(1);
                }
                let raw_meta = std::fs::read_to_string(&meta_path)
                    .map_err(|e| {
                        eprintln!("Error reading plugin.toml: {}", e);
                        process::exit(1);
                    })
                    .unwrap();
                let meta: sdkt_audit::plugin_store::PluginMeta =
                    match sdkt_audit::plugin_store::parse_meta(&raw_meta) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Error parsing plugin.toml: {}", e);
                            process::exit(1);
                        }
                    };
                let artifact = src.join(&meta.artifact);
                if !artifact.exists() {
                    eprintln!(
                        "Error: artifact '{}' not found in plugin directory",
                        meta.artifact
                    );
                    process::exit(1);
                }
                let out =
                    output.unwrap_or_else(|| format!("{}-{}.sdktplugin", meta.id, meta.version));
                let signing_key = if let Some(key_path) = secret_key {
                    let bytes = std::fs::read(key_path)
                        .map_err(|e| {
                            eprintln!("Error reading secret key: {}", e);
                            process::exit(1);
                        })
                        .unwrap();
                    let arr: [u8; 32] = bytes
                        .try_into()
                        .map_err(|_| {
                            eprintln!("Error: secret key must be exactly 32 bytes");
                            process::exit(1);
                        })
                        .unwrap();
                    Some(sdkt_audit::plugin_store::ed25519_dalek::SigningKey::from_bytes(&arr))
                } else {
                    None
                };
                match sdkt_audit::plugin_store::pack_bundle(
                    std::path::Path::new(&out),
                    &meta,
                    &artifact,
                    signing_key.as_ref(),
                ) {
                    Ok(()) => {
                        if fmt == OutputFormat::Json {
                            let json = serde_json::json!({
                                "status": "packed",
                                "output": out,
                                "id": meta.id,
                                "version": meta.version,
                                "signed": signing_key.is_some(),
                            });
                            println!("{}", serde_json::to_string_pretty(&json)?);
                        } else {
                            println!("Packed plugin to '{}'", out);
                        }
                        if signing_key.is_none() {
                            eprintln!("Note: bundle was NOT signed (pass --secret-key to sign)");
                        }
                    }
                    Err(e) => {
                        eprintln!("Error packing plugin bundle: {}", e);
                        process::exit(1);
                    }
                }
            }
            PluginAction::VerifyBundle {
                bundle,
                public_key,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let pubkey = public_key.as_deref().map(read_public_key_or_exit);
                let staging =
                    std::env::temp_dir().join(format!("sdkt-verify-bundle-{}", std::process::id()));
                match sdkt_audit::plugin_store::verify_bundle(
                    std::path::Path::new(&bundle),
                    &staging,
                    pubkey.as_ref(),
                ) {
                    Ok(result) if fmt == OutputFormat::Json => {
                        let json = serde_json::json!({
                            "valid": true,
                            "signed": result.signed,
                            "plugin": result.metadata,
                        });
                        println!("{}", serde_json::to_string_pretty(&json)?);
                    }
                    Ok(result) => {
                        println!("Bundle is valid.");
                        println!("  id: {}", result.metadata.id);
                        println!("  version: {}", result.metadata.version);
                        println!("  kind: {}", result.metadata.kind);
                        if result.signed {
                            println!("  signature: VERIFIED");
                        } else {
                            println!("  signature: UNSIGNED");
                        }
                    }
                    Err(e) => {
                        eprintln!("Error verifying bundle: {}", e);
                        process::exit(1);
                    }
                }
                let _ = std::fs::remove_dir_all(&staging);
            }
            PluginAction::Doctor {
                target,
                all,
                format,
            } => {
                let fmt = parse_format_str(&format);
                if all {
                    let plugins = sdkt_audit::plugin_store::list();
                    if plugins.is_empty() {
                        if fmt == OutputFormat::Json {
                            println!("{}", serde_json::json!({ "healthy": true, "reports": [] }));
                        } else {
                            println!("No plugins installed.");
                        }
                        process::exit(0);
                    }
                    let mut all_healthy = true;
                    let mut reports = Vec::new();
                    for p in &plugins {
                        let report = sdkt_audit::plugin_doctor::doctor_installed(&p.id);
                        if !report.healthy {
                            all_healthy = false;
                        }
                        reports.push(report);
                    }
                    if fmt == OutputFormat::Json {
                        let json =
                            serde_json::json!({ "healthy": all_healthy, "reports": reports });
                        println!("{}", serde_json::to_string_pretty(&json)?);
                    } else {
                        for (idx, report) in reports.iter().enumerate() {
                            if idx > 0 {
                                println!();
                            }
                            println!("=== Doctor: {} ===", report.target);
                            for (s_idx, stage) in report.stages.iter().enumerate() {
                                let tag = match stage.status {
                                    sdkt_audit::plugin_doctor::DoctorStageStatus::Passed => {
                                        "[PASS]"
                                    }
                                    sdkt_audit::plugin_doctor::DoctorStageStatus::Failed => {
                                        "[FAIL]"
                                    }
                                };
                                println!(
                                    "{} Stage {} ({}): {}",
                                    tag,
                                    s_idx + 1,
                                    stage.name,
                                    stage.detail
                                );
                            }
                            if report.healthy {
                                println!("Plugin '{}' is healthy.", report.target);
                            } else if let Some(failed) = report.failed_stage() {
                                eprintln!("Error: plugin doctor failed at stage '{}'", failed.name);
                            }
                        }
                    }
                    if !all_healthy {
                        process::exit(1);
                    }
                } else {
                    let target = target.expect("target is required when --all is not specified");
                    let report = sdkt_audit::plugin_doctor::doctor(&target);
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        for (s_idx, stage) in report.stages.iter().enumerate() {
                            let tag = match stage.status {
                                sdkt_audit::plugin_doctor::DoctorStageStatus::Passed => "[PASS]",
                                sdkt_audit::plugin_doctor::DoctorStageStatus::Failed => "[FAIL]",
                            };
                            println!(
                                "{} Stage {} ({}): {}",
                                tag,
                                s_idx + 1,
                                stage.name,
                                stage.detail
                            );
                        }
                        if report.healthy {
                            println!("Plugin '{}' is healthy.", report.target);
                        } else if let Some(failed) = report.failed_stage() {
                            eprintln!("Error: plugin doctor failed at stage '{}'", failed.name);
                        }
                    }
                    if !report.healthy {
                        process::exit(report.exit_code());
                    }
                }
            }
        },
        Commands::ReleaseAssurance {
            wasm,
            previous_wasm,
            fuzz_evidence,
            audit,
            disable,
            contract,
            network,
            format,
            max_size_bytes,
            max_growth_pct,
            net,
        } => {
            let fmt = parse_format_str(&format);
            // Validate policy inputs before doing any work: a requested check
            // that cannot be evaluated must fail loudly, never be skipped.
            if let Err(e) = validate_size_threshold_bytes(max_size_bytes, "--max-size-bytes") {
                eprintln!("Error: {e}");
                process::exit(1);
            }
            if let Err(e) = validate_size_threshold_pct(max_growth_pct, "--max-growth-pct") {
                eprintln!("Error: {e}");
                process::exit(1);
            }
            if max_growth_pct.is_some() && previous_wasm.is_none() {
                eprintln!(
                    "Error: --max-growth-pct requires --previous-wasm (growth is measured against \
                     the previous artifact; refusing to silently skip the requested check)"
                );
                process::exit(1);
            }
            // Read-only boundary: same resolution as `verify` / `health`
            // (`resolve_target_network`). The mutating mainnet guard does not
            // apply — release-assurance never signs, submits, or mutates.
            let target = match resolve_target_network(network.as_deref(), &net) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };
            let network_name = target.network_name;
            let report = match run_release_assurance(
                &target.client,
                &wasm,
                previous_wasm.as_deref(),
                &audit,
                &disable,
                contract.as_deref(),
                &network_name,
                fuzz_evidence.as_deref(),
            )
            .await
            {
                Ok(report) => report,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };
            // Opt-in size policy, folded into the artifact section of the
            // finished report: a violation flips artifact to FAIL, which the
            // existing aggregation turns into release_status=FAIL (exit 1).
            // With no policy flags this is a no-op.
            let mut report = report;
            apply_size_policy_to_assurance(
                &mut report,
                previous_wasm.as_deref(),
                max_size_bytes,
                max_growth_pct,
            )?;
            match fmt {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_else(|e| {
                            eprintln!("Error serializing report: {}", e);
                            process::exit(1);
                        })
                    );
                }
                OutputFormat::Pretty => print_release_assurance_pretty(&report),
            }
            // Deterministic exit status: blocking failure → 1, else 0
            // (same convention as `sdkt doctor`).
            if report.release_status == "FAIL" {
                process::exit(1);
            }
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            // Wrap stdout so a consumer that closes the pipe early
            // (`sdkt completions bash | head`) yields EPIPE, which we treat as
            // success instead of letting clap_complete panic on it.
            let mut out = BrokenPipeOk(std::io::stdout());
            clap_complete::generate(shell, &mut cmd, "sdkt", &mut out);
        }
        Commands::Doctor { format, json } => {
            let fmt = if json {
                OutputFormat::Json
            } else {
                parse_format_str(&format)
            };
            run_doctor(fmt);
        }
        Commands::Fuzz(action) => match action {
            FuzzAction::Campaign {
                wasm,
                seed,
                cases,
                function,
                artifact_dir,
                auth,
                expect_success,
                expect_error,
                replay,
                format,
            } => {
                match commands::fuzz::run(
                    &wasm,
                    seed,
                    cases,
                    function,
                    artifact_dir,
                    &auth,
                    expect_success,
                    expect_error,
                    replay,
                    &format,
                ) {
                    // Findings exist → exit 1 (the only exit code that means
                    // "a declared rule was violated").
                    Ok(true) => process::exit(1),
                    Ok(false) => {}
                    // Usage/input error → exit 2, distinct from findings.
                    Err(e) => {
                        eprintln!("Error: {e}");
                        process::exit(2);
                    }
                }
            }
            FuzzAction::Replay { artifact, wasm } => {
                match commands::fuzz::run_replay(&artifact, &wasm, "pretty") {
                    Ok(true) => {}
                    Ok(false) => process::exit(1),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        process::exit(2);
                    }
                }
            }
        },
        Commands::Generate(action) => match action {
            GenerateAction::Client {
                wasm,
                output,
                skip_unsupported,
            } => {
                if let Err(e) = run_generate_client(&wasm, output.as_deref(), skip_unsupported) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
        },
    }

    Ok(())
}

/// Execute `sdkt generate client`: parse the ContractSpec from a local WASM
/// and emit a deterministic typed Rust client (offline, no network).
fn run_generate_client(
    wasm_path: &str,
    output: Option<&str>,
    skip_unsupported: bool,
) -> Result<(), String> {
    let bytes = fs::read(wasm_path).map_err(|e| format!("cannot read WASM '{wasm_path}': {e}"))?;
    let spec = parse_contract_spec(&bytes).map_err(|e| format!("{wasm_path}: {e}"))?;
    let options = sdkt_wasm::GenerateOptions { skip_unsupported };
    let code =
        sdkt_wasm::generate_client_with_options(&spec, &options).map_err(|e| e.to_string())?;
    match output {
        Some(path) => {
            fs::write(path, &code).map_err(|e| format!("cannot write '{path}': {e}"))?;
            println!("✓ Generated client ({} bytes) -> {}", code.len(), path);
        }
        None => print!("{}", code),
    }
    Ok(())
}

#[cfg(test)]
mod events_topic_unit_tests {
    use super::*;
    use sdkt_wasm::ContractSpec;

    fn spec_with_events(events: &[(&str, &[&str])]) -> ContractSpec {
        ContractSpec {
            env_meta: None,
            functions: vec![],
            custom_types: vec![],
            events: events
                .iter()
                .map(|(name, prefix)| sdkt_wasm::spec::ContractEvent {
                    name: name.to_string(),
                    doc: String::new(),
                    prefix_topics: prefix.iter().map(|p| p.to_string()).collect(),
                    params: vec![],
                    data_format: "single_value".to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn wire_topic_produces_no_warning() {
        // Prefix topic is the wire topic; an event without prefix falls back to its name.
        let spec = spec_with_events(&[("Transfer", &["transfer"]), ("Approval", &[])]);
        assert_eq!(unknown_event_topic_warning(&spec, "transfer"), None);
        assert_eq!(unknown_event_topic_warning(&spec, "Approval"), None);
    }

    #[test]
    fn abi_name_differing_from_prefix_suggests_prefix() {
        let spec = spec_with_events(&[("Transfer", &["transfer"])]);
        let w = unknown_event_topic_warning(&spec, "Transfer").expect("warning");
        assert!(w.contains("'Transfer' is the ABI name of an event"), "{w}");
        assert!(
            w.contains("did you mean 'transfer'? (the wire topic is the prefix topic)"),
            "{w}"
        );
        assert!(w.ends_with("Querying anyway."), "{w}");
    }

    #[test]
    fn abi_name_equal_to_prefix_produces_no_warning() {
        let spec = spec_with_events(&[("transfer", &["transfer"])]);
        assert_eq!(unknown_event_topic_warning(&spec, "transfer"), None);
    }

    #[test]
    fn unknown_symbol_warns_and_lists_wire_topics() {
        let spec = spec_with_events(&[("Transfer", &["transfer"]), ("Approval", &[])]);
        let w = unknown_event_topic_warning(&spec, "Mint").expect("warning");
        assert!(w.contains("'Mint' is not declared"), "{w}");
        assert!(w.contains("declared: transfer, Approval."), "{w}");
        assert!(!w.contains("did you mean"), "{w}");
    }

    #[test]
    fn case_mismatch_suggests_exact_symbol() {
        let spec = spec_with_events(&[("Approval", &["approval"])]);
        let w = unknown_event_topic_warning(&spec, "APPROVAL").expect("warning");
        assert!(w.contains("did you mean 'approval'?"), "{w}");
    }

    #[test]
    fn spec_without_events_warns() {
        let spec = spec_with_events(&[]);
        let w = unknown_event_topic_warning(&spec, "Transfer").expect("warning");
        assert!(w.contains("the ABI declares no events"), "{w}");
    }
}

#[cfg(test)]
mod doctor_unit_tests {
    use super::*;
    use std::ffi::OsString;

    // ── wasm_target_present: project contract-build target oracle ──

    #[test]
    fn wasm_target_accepts_unknown_unknown() {
        assert!(wasm_target_present("wasm32-unknown-unknown\n"));
        assert!(wasm_target_present(
            "x86_64-unknown-linux-gnu\nwasm32-unknown-unknown\n"
        ));
    }

    #[test]
    fn wasm_target_accepts_v1_none() {
        assert!(wasm_target_present("wasm32v1-none\n"));
    }

    #[test]
    fn wasm_target_rejects_only_wasip1() {
        // CI installs only wasm32-wasip1 (plugin/playground target). It is
        // NOT a contract build target — sdkt build hardcodes
        // wasm32-unknown-unknown — so the check must not be satisfied by it.
        assert!(!wasm_target_present("wasm32-wasip1\n"));
    }

    #[test]
    fn wasm_target_rejects_empty_and_unrelated() {
        assert!(!wasm_target_present(""));
        assert!(!wasm_target_present(
            "x86_64-unknown-linux-gnu\naarch64-apple-darwin\n"
        ));
    }

    #[test]
    fn wasm_target_handles_missing_trailing_newline() {
        assert!(wasm_target_present("wasm32-unknown-unknown"));
        assert!(wasm_target_present("  wasm32v1-none  \n"));
    }

    // ── path_contains_command: Windows-safe executable discovery ──

    fn with_path(dir: &std::path::Path) -> OsString {
        std::env::join_paths([dir.to_path_buf()]).unwrap()
    }

    #[test]
    fn path_finds_bare_executable() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("cargo"), b"").unwrap();
        assert!(path_contains_command(&with_path(tmp.path()), "cargo"));
    }

    #[test]
    fn path_finds_exe_suffixed_executable() {
        // Windows exposes cargo.exe / rustc.exe on PATH. Discovery must try
        // the .exe form so doctor does not falsely report the toolchain as
        // missing on Windows.
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("cargo.exe"), b"").unwrap();
        assert!(path_contains_command(&with_path(tmp.path()), "cargo"));
    }

    #[test]
    fn path_finds_rustc_exe() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("rustc.exe"), b"").unwrap();
        assert!(path_contains_command(&with_path(tmp.path()), "rustc"));
    }

    #[test]
    fn path_rejects_missing_command() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!path_contains_command(&with_path(tmp.path()), "cargo"));
        assert!(!path_contains_command(&with_path(tmp.path()), "rustc"));
    }

    #[test]
    fn path_rejects_directory_named_like_command() {
        // A directory named "cargo" on PATH is not an executable.
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("cargo")).unwrap();
        assert!(!path_contains_command(&with_path(tmp.path()), "cargo"));
    }

    #[test]
    fn path_rejects_unrelated_executable() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("notcargo.exe"), b"").unwrap();
        fs::write(tmp.path().join("cargo.txt"), b"").unwrap();
        assert!(!path_contains_command(&with_path(tmp.path()), "cargo"));
    }

    // ── DoctorReport healthy aggregation ──

    #[test]
    fn report_healthy_when_only_warnings() {
        let report = DoctorReport::from_checks(vec![
            DoctorCheck::ok("a", "fine"),
            DoctorCheck::warn("b", "meh", "fix it"),
        ]);
        assert!(report.healthy);
    }

    #[test]
    fn report_unhealthy_with_any_error() {
        let report = DoctorReport::from_checks(vec![
            DoctorCheck::ok("a", "fine"),
            DoctorCheck::err("b", "broken", "fix it"),
        ]);
        assert!(!report.healthy);
    }

    #[test]
    fn report_json_shape_is_stable() {
        let report = DoctorReport::from_checks(vec![DoctorCheck::warn("x", "note", "hint")]);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["healthy"], false_or_true(&report));
        assert_eq!(json["checks"][0]["id"], "x");
        assert_eq!(json["checks"][0]["status"], "warning");
        assert_eq!(json["checks"][0]["message"], "note");
        assert_eq!(json["checks"][0]["remediation"], "hint");
    }

    fn false_or_true(r: &DoctorReport) -> serde_json::Value {
        serde_json::json!(r.healthy)
    }

    #[test]
    fn ok_check_omits_remediation_in_json() {
        let report = DoctorReport::from_checks(vec![DoctorCheck::ok("x", "fine")]);
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["checks"][0].get("remediation").is_none());
    }
}

#[cfg(test)]
mod m22_tests {
    use super::*;

    #[test]
    fn verification_outcome_verified() {
        let (m, status, exp) = verification_outcome("abc123", Some(("abc123".to_string(), 4096)));
        assert_eq!(m, Some(true));
        assert_eq!(status, "Verified");
        assert!(exp.is_empty());
    }

    #[test]
    fn verification_outcome_mismatch() {
        let (m, status, exp) = verification_outcome("abc123", Some(("def456".to_string(), 4096)));
        assert_eq!(m, Some(false));
        assert_eq!(status, "Mismatch");
        assert!(exp.contains("abc123"));
        assert!(exp.contains("def456"));
    }

    #[test]
    fn verification_outcome_onchain_only() {
        let (m, status, exp) = verification_outcome("abc123", None);
        assert_eq!(m, None);
        assert_eq!(status, "OnChainOnly");
        assert!(exp.contains("No local WASM"));
    }

    #[test]
    fn verification_report_json_schema() {
        // Verified case
        let r = VerificationReport {
            contract_id: "CABCDEFG".to_string(),
            network: "testnet".to_string(),
            on_chain_wasm_hash: "abc123".to_string(),
            local_wasm_hash: Some("abc123".to_string()),
            local_wasm_size_bytes: Some(4096),
            matches: Some(true),
            verification_status: "Verified".to_string(),
            explanation: String::new(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"contract_id\":\"CABCDEFG\""));
        assert!(json.contains("\"on_chain_wasm_hash\":\"abc123\""));
        assert!(json.contains("\"local_wasm_hash\":\"abc123\""));
        assert!(json.contains("\"local_wasm_size_bytes\":4096"));
        assert!(json.contains("\"match\":true"));
        assert!(json.contains("\"verification_status\":\"Verified\""));

        // OnChainOnly case — local fields must be absent (null/omitted)
        let r2 = VerificationReport {
            contract_id: "CABCDEFG".to_string(),
            network: "testnet".to_string(),
            on_chain_wasm_hash: "abc123".to_string(),
            local_wasm_hash: None,
            local_wasm_size_bytes: None,
            matches: None,
            verification_status: "OnChainOnly".to_string(),
            explanation: "No local WASM provided; reporting on-chain hash only.".to_string(),
        };
        let json2 = serde_json::to_string(&r2).unwrap();
        assert!(json2.contains("\"match\":null") || !json2.contains("\"match\""));
        assert!(!json2.contains("\"local_wasm_hash\":\"abc123\""));
    }
}

#[cfg(test)]
mod m23_tests {
    use super::*;

    #[test]
    fn derive_verdict_healthy() {
        let (h, reasons) = derive_verdict(Some(true), 0, 12);
        assert_eq!(h, "healthy");
        assert!(reasons.is_empty());
    }

    #[test]
    fn derive_verdict_at_risk_expiring() {
        let (h, reasons) = derive_verdict(Some(true), 2, 12);
        assert_eq!(h, "at_risk");
        let reason = reasons
            .iter()
            .find(|r| r.contains("2 storage entries"))
            .unwrap();
        let threshold_days = (u64::from(EXPIRING_SOON_LEDGERS) * 5).div_ceil(86_400);
        assert_eq!(threshold_days, 1);
        assert!(reason.contains(&format!("within ~{threshold_days} day)")));
    }

    #[test]
    fn derive_verdict_critical_mismatch() {
        // Mismatch wins over TTL, regardless of expiring count.
        let (h, reasons) = derive_verdict(Some(false), 5, 12);
        assert_eq!(h, "critical");
        assert!(reasons.iter().any(|r| r.contains("does NOT match")));
    }

    #[test]
    fn derive_verdict_at_risk_empty() {
        let (h, reasons) = derive_verdict(None, 0, 0);
        assert_eq!(h, "at_risk");
        assert!(reasons.iter().any(|r| r.contains("no storage entries")));
    }

    #[test]
    fn derive_verdict_onchain_only_healthy() {
        // No --wasm supplied (verified == None), nothing expiring → healthy.
        let (h, reasons) = derive_verdict(None, 0, 7);
        assert_eq!(h, "healthy");
        assert!(reasons.is_empty());
    }

    #[test]
    fn health_report_json_schema() {
        // Healthy with --wasm verified
        let r = ContractHealthReport {
            contract_id: "CABCDEFG".to_string(),
            network: "testnet".to_string(),
            health: "healthy".to_string(),
            verified: Some(true),
            on_chain_wasm_hash: "abc123".to_string(),
            local_wasm_hash: Some("abc123".to_string()),
            local_wasm_size_bytes: Some(4096),
            storage: HealthStorage {
                total_entries: 12,
                instance_entries: 1,
                persistent_entries: 9,
                temporary_entries: 2,
                other_entries: 0,
                ttl: Some(HealthTtl {
                    minimum_ttl: 518400,
                    maximum_ttl: 518400,
                    average_ttl: 518400,
                    expiring_entries_count: 0,
                    estimated_rent_cost: Some(240000),
                }),
            },
            reasons: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"health\":\"healthy\""));
        assert!(json.contains("\"verified\":true"));
        assert!(json.contains("\"on_chain_wasm_hash\":\"abc123\""));
        assert!(json.contains("\"storage\""));
        assert!(json.contains("\"total_entries\":12"));
        assert!(json.contains("\"ttl\""));
        assert!(json.contains("\"expiring_entries_count\":0"));

        // OnChainOnly (no --wasm) → verified/local fields null/omitted
        let r2 = ContractHealthReport {
            contract_id: "CABCDEFG".to_string(),
            network: "testnet".to_string(),
            health: "healthy".to_string(),
            verified: None,
            on_chain_wasm_hash: "abc123".to_string(),
            local_wasm_hash: None,
            local_wasm_size_bytes: None,
            storage: HealthStorage {
                total_entries: 7,
                instance_entries: 1,
                persistent_entries: 5,
                temporary_entries: 1,
                other_entries: 0,
                ttl: None,
            },
            reasons: vec![],
        };
        let json2 = serde_json::to_string(&r2).unwrap();
        assert!(json2.contains("\"verified\":null") || !json2.contains("\"verified\""));
    }
}

#[cfg(test)]
mod wasm_size_policy_tests {
    use super::*;

    // ---- threshold validation ----

    #[test]
    fn validates_byte_threshold_rejects_zero_and_accepts_none() {
        assert!(validate_size_threshold_bytes(None, "--max-size-bytes").is_ok());
        assert!(validate_size_threshold_bytes(Some(1), "--max-size-bytes").is_ok());
        let err = validate_size_threshold_bytes(Some(0), "--max-size-bytes").unwrap_err();
        assert!(err.contains("greater than 0"), "{err}");
    }

    #[test]
    fn validates_pct_threshold_rejects_negative_and_non_finite() {
        assert!(validate_size_threshold_pct(None, "--max-growth-pct").is_ok());
        assert!(validate_size_threshold_pct(Some(0.0), "--max-growth-pct").is_ok());
        assert!(validate_size_threshold_pct(Some(10.0), "--max-growth-pct").is_ok());
        let neg = validate_size_threshold_pct(Some(-1.0), "--max-growth-pct").unwrap_err();
        assert!(neg.contains("negative"), "{neg}");
        let nan = validate_size_threshold_pct(Some(f64::NAN), "--max-growth-pct").unwrap_err();
        assert!(nan.contains("finite"), "{nan}");
        let inf = validate_size_threshold_pct(Some(f64::INFINITY), "--max-growth-pct").unwrap_err();
        assert!(inf.contains("finite"), "{inf}");
    }

    // ---- policy evaluation: boundaries are inclusive (violation needs >) ----

    #[test]
    fn abs_policy_boundary_is_inclusive() {
        // size == max -> pass; size == max+1 -> violation.
        assert!(evaluate_size_policy(Some(1), 10542, Some(10542), None).is_empty());
        let v = evaluate_size_policy(Some(1), 10543, Some(10542), None);
        assert_eq!(v.len(), 1);
        assert!(
            v[0].contains("10543 bytes exceeds --max-size-bytes 10542"),
            "{v:?}"
        );
    }

    #[test]
    fn growth_policy_boundary_is_inclusive() {
        // 198 -> 530 is +167.7%; exactly 167.7 passes, 167.6 violates.
        assert!(evaluate_size_policy(Some(198), 530, None, Some(167.7)).is_empty());
        let v = evaluate_size_policy(Some(198), 530, None, Some(167.6));
        assert_eq!(v.len(), 1);
        assert!(
            v[0].contains("167.7% exceeds --max-growth-pct 167.6"),
            "{v:?}"
        );
    }

    #[test]
    fn negative_growth_never_violates_a_growth_cap() {
        // Shrinking is not a regression.
        assert!(evaluate_size_policy(Some(10543), 467, None, Some(0.0)).is_empty());
        assert!(evaluate_size_policy(Some(10543), 467, None, Some(10.0)).is_empty());
    }

    #[test]
    fn growth_policy_requires_a_baseline() {
        let v = evaluate_size_policy(None, 100, None, Some(10.0));
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("requires a previous artifact"), "{v:?}");
    }

    #[test]
    fn growth_policy_reports_zero_baseline_as_undefined_violation() {
        // 0-byte baseline with a non-empty candidate: undefined growth must be
        // an explicit violation, never a silent pass.
        let v = evaluate_size_policy(Some(0), 100, None, Some(10.0));
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("undefined (zero-byte baseline)"), "{v:?}");
        // Both artifacts empty is defined (0.0) and passes.
        assert!(evaluate_size_policy(Some(0), 0, None, Some(10.0)).is_empty());
    }

    #[test]
    fn no_policy_flags_means_no_violations() {
        // 22x growth must be fine when no policy is configured.
        assert!(evaluate_size_policy(Some(467), 10543, None, None).is_empty());
    }

    #[test]
    fn both_policies_can_violate_together() {
        let v = evaluate_size_policy(Some(467), 10543, Some(1000), Some(10.0));
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("--max-size-bytes"), "{v:?}");
        assert!(v[1].contains("--max-growth-pct"), "{v:?}");
    }

    #[test]
    fn growth_value_renders_undefined_case() {
        assert_eq!(format_growth_pct(Some(12.3)), "12.3%");
        assert_eq!(format_growth_pct(None), "undefined (zero-byte baseline)");
    }
}

#[cfg(test)]
mod storage_read_key_tests {
    use super::*;
    use stellar_xdr::{ContractDataDurability, LedgerKey, ScVal};

    const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn args(vals: &[&str]) -> Vec<String> {
        vals.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_no_key_source() {
        let err =
            resolve_storage_read_key(CONTRACT, None, None, &[], false, "persistent").unwrap_err();
        assert!(err.contains("provide a key"));
    }

    #[test]
    fn rejects_multiple_key_sources() {
        let err = resolve_storage_read_key(
            CONTRACT,
            Some("AAAA"),
            Some("bal"),
            &[],
            false,
            "persistent",
        )
        .unwrap_err();
        assert!(err.contains("only one of"));
    }

    #[test]
    fn rejects_key_arg_without_map_key() {
        let err =
            resolve_storage_read_key(CONTRACT, None, None, &args(&["u32:1"]), true, "persistent")
                .unwrap_err();
        assert!(err.contains("--key-arg requires --map-key"));
    }

    #[test]
    fn escape_hatch_passes_raw_key_through() {
        let raw = "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let out =
            resolve_storage_read_key(CONTRACT, Some(raw), None, &[], false, "persistent").unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn rejects_empty_key_xdr() {
        let err = resolve_storage_read_key(CONTRACT, Some("   "), None, &[], false, "persistent")
            .unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn instance_builds_persistent_instance_key() {
        let out = resolve_storage_read_key(CONTRACT, None, None, &[], true, "persistent").unwrap();
        match sdkt_xdr::decode_ledger_key(&out).unwrap() {
            LedgerKey::ContractData(d) => {
                assert_eq!(d.key, ScVal::LedgerKeyContractInstance);
                assert_eq!(d.durability, ContractDataDurability::Persistent);
            }
            other => panic!("expected ContractData, got {other:?}"),
        }
    }

    #[test]
    fn typed_map_key_persistent_and_temporary_differ() {
        let persistent = resolve_storage_read_key(
            CONTRACT,
            None,
            Some("balances"),
            &args(&["u32:100"]),
            false,
            "persistent",
        )
        .unwrap();
        let temporary = resolve_storage_read_key(
            CONTRACT,
            None,
            Some("balances"),
            &args(&["u32:100"]),
            false,
            "temporary",
        )
        .unwrap();
        assert_ne!(persistent, temporary);

        match sdkt_xdr::decode_ledger_key(&persistent).unwrap() {
            LedgerKey::ContractData(d) => {
                assert_eq!(d.durability, ContractDataDurability::Persistent);
                match d.key {
                    ScVal::Vec(Some(v)) => {
                        assert_eq!(v.len(), 2);
                        assert_eq!(v[0], ScVal::Symbol("balances".try_into().unwrap()));
                        assert_eq!(v[1], ScVal::U32(100));
                    }
                    other => panic!("expected ScVec key, got {other:?}"),
                }
            }
            other => panic!("expected ContractData, got {other:?}"),
        }
    }

    #[test]
    fn typed_output_equals_raw_key_xdr_path() {
        // The typed path and the raw escape hatch must resolve to the same key:
        // feeding the typed output back as --key-xdr is a no-op.
        let typed = resolve_storage_read_key(
            CONTRACT,
            None,
            Some("bal"),
            &args(&["u32:7"]),
            false,
            "persistent",
        )
        .unwrap();
        let via_raw =
            resolve_storage_read_key(CONTRACT, Some(&typed), None, &[], false, "persistent")
                .unwrap();
        assert_eq!(typed, via_raw);
    }

    #[test]
    fn rejects_unknown_key_arg_type() {
        let err = resolve_storage_read_key(
            CONTRACT,
            None,
            Some("bal"),
            &args(&["weird:1"]),
            false,
            "persistent",
        )
        .unwrap_err();
        assert!(err.contains("unknown arg type"));
    }

    #[test]
    fn rejects_invalid_durability() {
        let err = resolve_storage_read_key(CONTRACT, None, Some("bal"), &[], false, "forever")
            .unwrap_err();
        assert!(err.contains("invalid durability"));
    }
}

#[cfg(test)]
mod storage_analyze_key_resolution_tests {
    use super::*;

    const CONTRACT: &str = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";

    fn args(vals: &[&str]) -> Vec<String> {
        vals.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn analyze_keys_empty_when_no_flags() {
        let keys = resolve_storage_analyze_keys(CONTRACT, &[], None, &[], "persistent").unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn analyze_keys_rejects_key_arg_without_map_key() {
        let err =
            resolve_storage_analyze_keys(CONTRACT, &[], None, &args(&["u32:1"]), "persistent")
                .unwrap_err();
        assert!(err.contains("--key-arg requires --map-key"));
    }

    #[test]
    fn analyze_keys_rejects_empty_key_xdr() {
        let err = resolve_storage_analyze_keys(CONTRACT, &args(&["   "]), None, &[], "persistent")
            .unwrap_err();
        assert!(err.contains("--key-xdr must not be empty"));
    }

    #[test]
    fn analyze_keys_rejects_invalid_key_xdr() {
        let err = resolve_storage_analyze_keys(
            CONTRACT,
            &args(&["not-a-valid-base64-or-hex"]),
            None,
            &[],
            "persistent",
        )
        .unwrap_err();
        assert!(err.contains("invalid LedgerKey"));
    }

    #[test]
    fn analyze_keys_accepts_valid_raw_keys() {
        let raw = "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let keys =
            resolve_storage_analyze_keys(CONTRACT, &args(&[raw]), None, &[], "persistent").unwrap();
        assert_eq!(keys, vec![raw]);
    }

    #[test]
    fn analyze_keys_accepts_typed_key() {
        let keys = resolve_storage_analyze_keys(
            CONTRACT,
            &[],
            Some("balances"),
            &args(&["u32:100"]),
            "temporary",
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
        let decoded = sdkt_xdr::decode_ledger_key(&keys[0]).unwrap();
        match decoded {
            stellar_xdr::LedgerKey::ContractData(d) => {
                assert_eq!(d.durability, stellar_xdr::ContractDataDurability::Temporary);
            }
            other => panic!("expected ContractData, got {other:?}"),
        }
    }

    #[test]
    fn analyze_keys_merges_raw_and_typed_keys() {
        let raw = "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let keys = resolve_storage_analyze_keys(
            CONTRACT,
            &args(&[raw]),
            Some("balances"),
            &args(&["u32:100"]),
            "persistent",
        )
        .unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], raw);
    }

    #[test]
    fn analyze_keys_rejects_invalid_durability() {
        let err = resolve_storage_analyze_keys(CONTRACT, &[], Some("balances"), &[], "forever")
            .unwrap_err();
        assert!(err.contains("invalid durability"));
    }

    #[test]
    fn analyze_keys_rejects_invalid_durability_without_map_key() {
        let err = resolve_storage_analyze_keys(CONTRACT, &[], None, &[], "forever").unwrap_err();
        assert!(err.contains("invalid durability"));
    }
}

#[cfg(test)]
mod project_deploy_salt_tests {
    use super::*;

    const SALT: &str = "00112233445566778899aabbccddeeff00112233";

    fn parse_project_deploy_salt(args: &[&str]) -> Option<String> {
        let cli = Cli::try_parse_from(args).expect("args should parse");
        match cli.command {
            Commands::Project {
                action: ProjectCommand::Deploy { salt, .. },
                ..
            } => salt,
            _ => panic!("expected `project deploy`"),
        }
    }

    #[test]
    fn explicit_salt_is_captured_and_parsed() {
        let salt = parse_project_deploy_salt(&["sdkt", "project", "deploy", "--salt", SALT]);
        assert_eq!(salt.as_deref(), Some(SALT));

        let bytes = parse_salt_hex(salt.as_deref().unwrap()).unwrap();
        assert_eq!(hex::encode(bytes), SALT);
    }

    #[test]
    fn omitted_salt_means_auto_generate() {
        assert_eq!(
            parse_project_deploy_salt(&["sdkt", "project", "deploy"]),
            None
        );
    }

    #[test]
    fn invalid_salt_is_rejected() {
        assert!(parse_salt_hex("deploy")
            .unwrap_err()
            .contains("Invalid --salt"));
        assert!(parse_salt_hex(&"z".repeat(40))
            .unwrap_err()
            .contains("not a hex digit"));
    }
}
