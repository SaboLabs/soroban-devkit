use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use sdkt_core::fee::{FeeConfig, FeeEstimator, LedgerFeeSample, NetworkKind};
use sdkt_core::fetch::DependencyFetcher;
use sdkt_core::{DevKitConfig, NetworkConfig, OutputFormat};
use sdkt_rpc::{
    estimate_dynamic_fee, get_contract_events, get_ttl_info, get_wasm_metadata, inspect_account,
    inspect_contract, inspect_transaction, simulate_transaction, SorobanRpcClient,
};
use sdkt_storage::WasmCache;
use sdkt_storage::{NetworkProfile, NetworkStore};
use sdkt_wasm::spec::parse_contract_spec;
use sdkt_xdr::abi_decode::decode_event_topics;
use sdkt_xdr::decode;
use sdkt_xdr::{
    build_invoke_transaction, sign_transaction, Ed25519Signer, InvokeTransactionParams, Network,
    SigningError, SigningOptions,
};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process;

/// Reusable network-resolution flags shared by every command that talks to a
/// Soroban/Stellar RPC endpoint.
///
/// Flattened into those commands via `#[command(flatten)]` so the resolution
/// semantics stay identical everywhere (no copy/paste).
#[derive(Args, Clone, Debug, Default)]
struct NetworkArgs {
    /// Use a saved network profile (see `sdkt network add`) for the RPC URL and
    /// network passphrase. Overrides .sdkt.toml defaults.
    #[arg(long, value_name = "NAME")]
    network_profile: Option<String>,
    /// Explicit RPC endpoint URL. Overrides any profile and .sdkt.toml value.
    #[arg(long, value_name = "URL")]
    rpc_url: Option<String>,
    /// Explicit network passphrase. Overrides any profile and .sdkt.toml value.
    #[arg(long, value_name = "PASSPHRASE")]
    network_passphrase: Option<String>,
}
/// Apply resolution precedence onto a base [`NetworkConfig`].
///
/// Pure function (no I/O, no network) — this is the single source of truth for
/// M29 precedence and is unit-tested directly.
///
/// Priority (highest wins):
/// 1. explicit `rpc_url` / `network_passphrase`,
/// 2. a resolved `profile` (loaded from `--network-profile`),
/// 3. the `base` config (`.sdkt.toml`, then `NetworkConfig::default()`).
fn apply_profile_overrides(
    base: NetworkConfig,
    profile: Option<NetworkProfile>,
    rpc_url: Option<String>,
    network_passphrase: Option<String>,
) -> NetworkConfig {
    let mut cfg = base;

    if let Some(p) = profile {
        cfg.rpc_url = p.rpc_url;
        cfg.passphrase = p.network_passphrase;
    }

    if let Some(url) = rpc_url {
        cfg.rpc_url = url;
    }
    if let Some(p) = network_passphrase {
        cfg.passphrase = p;
    }

    cfg
}

/// Resolve the effective [`NetworkConfig`] from explicit CLI overrides, an
/// optional named profile, and built-in defaults.
///
/// Resolution priority (highest wins):
/// 1. explicit `--rpc-url` / `--network-passphrase` CLI flags,
/// 2. `--network-profile <NAME>` (loaded from `sdkt_storage::NetworkStore`),
/// 3. built-in defaults: `.sdkt.toml` `[network]`, then `NetworkConfig::default()`.
///
/// Explicit flags always override values loaded from a profile, and a profile
/// always overrides the built-in defaults.
fn resolve_network_config(
    rpc_url: Option<String>,
    network_passphrase: Option<String>,
    network_profile: Option<String>,
) -> Result<NetworkConfig, String> {
    let base = DevKitConfig::from_file(".sdkt.toml")
        .ok()
        .map(|c| c.network)
        .unwrap_or_default();

    let profile = if let Some(name) = network_profile {
        let store = NetworkStore::new().map_err(|e| format!("cannot open network store: {}", e))?;
        let profile = store
            .get(&name)
            .map_err(|e| format!("network profile '{}' not found: {}", name, e))?;
        Some(profile)
    } else {
        None
    };

    Ok(apply_profile_overrides(
        base,
        profile,
        rpc_url,
        network_passphrase,
    ))
}

/// Build a [`SorobanRpcClient`] from the resolved network configuration,
/// exiting with a clear error message if resolution fails.
fn resolve_rpc_client(
    rpc_url: Option<String>,
    network_passphrase: Option<String>,
    network_profile: Option<String>,
) -> SorobanRpcClient {
    match resolve_network_config(rpc_url, network_passphrase, network_profile) {
        Ok(cfg) => SorobanRpcClient::from_config(&cfg),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
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

#[cfg(test)]
mod resolver_tests {
    use super::*;

    fn base_testnet() -> NetworkConfig {
        NetworkConfig {
            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
            passphrase: "Test SDF Network ; September 2015".to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        }
    }

    fn profile(name: &str, url: &str, pass: &str) -> NetworkProfile {
        NetworkProfile::new(name, url, pass)
    }

    #[test]
    fn built_in_default_when_nothing_set() {
        let cfg = apply_profile_overrides(base_testnet(), None, None, None);
        assert_eq!(cfg.rpc_url, "https://soroban-testnet.stellar.org");
        assert_eq!(cfg.passphrase, "Test SDF Network ; September 2015");
    }

    #[test]
    fn profile_overrides_built_in_default() {
        let p = profile("local", "http://127.0.0.1:8000", "Standalone");
        let cfg = apply_profile_overrides(base_testnet(), Some(p), None, None);
        assert_eq!(cfg.rpc_url, "http://127.0.0.1:8000");
        assert_eq!(cfg.passphrase, "Standalone");
    }

    #[test]
    fn rpc_url_flag_overrides_profile() {
        let p = profile("local", "http://127.0.0.1:8000", "Standalone");
        let cfg = apply_profile_overrides(
            base_testnet(),
            Some(p),
            Some("http://override.example".to_string()),
            None,
        );
        assert_eq!(cfg.rpc_url, "http://override.example");
        // passphrase comes from the profile when no passphrase flag is given
        assert_eq!(cfg.passphrase, "Standalone");
    }

    #[test]
    fn passphrase_flag_overrides_profile() {
        let p = profile("local", "http://127.0.0.1:8000", "Standalone");
        let cfg = apply_profile_overrides(
            base_testnet(),
            Some(p),
            None,
            Some("Override Passphrase".to_string()),
        );
        assert_eq!(cfg.rpc_url, "http://127.0.0.1:8000");
        assert_eq!(cfg.passphrase, "Override Passphrase");
    }

    #[test]
    fn explicit_flags_win_over_profile_both() {
        let p = profile("local", "http://127.0.0.1:8000", "Standalone");
        let cfg = apply_profile_overrides(
            base_testnet(),
            Some(p),
            Some("http://rpc.example".to_string()),
            Some("RPC Passphrase".to_string()),
        );
        assert_eq!(cfg.rpc_url, "http://rpc.example");
        assert_eq!(cfg.passphrase, "RPC Passphrase");
    }

    #[test]
    fn rpc_url_flag_without_profile_overrides_built_in() {
        let cfg = apply_profile_overrides(
            base_testnet(),
            None,
            Some("http://flag.example".to_string()),
            None,
        );
        assert_eq!(cfg.rpc_url, "http://flag.example");
        assert_eq!(cfg.passphrase, "Test SDF Network ; September 2015");
    }
}

/// Soroban DevKit — unified toolkit for Stellar/Soroban development.
#[derive(Parser)]
#[command(name = "sdkt")]
#[command(about = "Soroban DevKit — unified toolkit for Stellar/Soroban development")]
#[command(version = env!("CARGO_PKG_VERSION"))]
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
    /// Inspect storage TTL for a contract
    Storage {
        #[command(subcommand)]
        action: StorageAction,
        /// Path to contract WASM for ABI-aware storage decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Inspect a contract's ABI and storage
    Inspect {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Path to contract WASM for ABI-aware storage inspection
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Verify a deployed contract matches a local WASM binary (M22)
    Verify {
        /// Stellar contract ID (C...)
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: String,
        /// Path to a local WASM file to compare against the on-chain code
        #[arg(long, value_name = "WASM")]
        wasm: Option<String>,
        /// Network to fetch the on-chain contract from
        #[arg(short, long, default_value = "testnet")]
        network: String,
        /// Output format
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Unified read-only contract posture report (M23)
    Health {
        /// Stellar contract ID (C...)
        #[arg(short, long, value_name = "CONTRACT_ID")]
        contract: String,
        /// Optional local WASM to verify against the on-chain hash
        #[arg(long, value_name = "WASM")]
        wasm: Option<String>,
        /// Network label for the report
        #[arg(short, long, default_value = "testnet")]
        network: String,
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
        /// Path to contract WASM for ABI-aware decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
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
    },
    /// Static security analysis of a Soroban contract source file (Gap C)
    Audit {
        /// Path to the Rust source file (.rs) to analyze
        path: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Disable a rule by id (repeatable), e.g. --disable MOVE-001
        #[arg(long, value_name = "RULE_ID", action = clap::ArgAction::Append)]
        disable: Vec<String>,
        /// Path to an external rule crate or local rule source directory to load.
        /// Repeatable. (Phase A: the rule must be compiled into the binary; this
        /// flag validates the path and runs the registered rules.)
        #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
        rules: Vec<String>,
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
        #[arg(short, long)]
        wasm: String,
        #[arg(short, long)]
        salt: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
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
    Build,
    /// Generate or inspect the project lock file (`sdkt.lock`)
    Lock {
        #[command(subcommand)]
        action: LockCommand,
    },
    /// Validate and inspect local package manifests (M35.0)
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
    /// Generate shell completion scripts for your shell
    Completions {
        /// Shell to generate completions for (bash, zsh, fish, powershell, elvish)
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum IdentityAction {
    Generate { name: String },
    Import { name: String, secret: String },
    List,
    Show { name: String },
    Delete { name: String },
    Default { name: String },
}

#[derive(Subcommand)]
enum NetworkAction {
    /// Add or update a named network profile
    Add {
        /// Profile name (referenced by other commands)
        name: String,
        /// RPC endpoint URL (e.g. https://soroban-testnet.stellar.org)
        #[arg(short, long, value_name = "URL")]
        rpc_url: String,
        /// Network passphrase (e.g. "Test SDF Network ; September 2015")
        #[arg(short, long, value_name = "PASSPHRASE")]
        passphrase: String,
        /// Optional friendbot URL for test networks
        #[arg(long, value_name = "URL")]
        friendbot: Option<String>,
        /// Optional human-readable description
        #[arg(short, long)]
        description: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// List all saved network profiles
    List {
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Show a single network profile by name
    Show {
        /// Profile name
        name: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Remove a network profile by name
    Remove {
        /// Profile name
        name: String,
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
        #[arg(short, long, default_value = "testnet")]
        network: String,
        /// Force bypass the cache and fetch fresh from RPC
        #[arg(long, default_value_t = false)]
        refresh: bool,
        #[arg(short, long, default_value = "pretty")]
        format: String,
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
    /// Simulate a transaction envelope without submitting it
    Simulate {
        /// Base64 XDR transaction envelope or path to a file containing it
        #[arg(short, long)]
        envelope: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
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
    Build {
        #[arg(long)]
        source: String,
        #[arg(long)]
        sequence: i64,
        #[arg(long, default_value = "100")]
        fee: u32,
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
    },
    /// Sign a transaction envelope using a local identity (M27 / PR2)
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
    /// Deploy all contracts defined in the workspace
    Deploy {
        /// Optional deployment salt base
        #[arg(short, long, default_value = "deploy")]
        salt: String,
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
}

#[derive(Subcommand)]
enum StorageAction {
    Check {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    Estimate {
        wasm: String,
    },
    /// Analyze a contract's storage layout (Instance/Persistent/Temporary
    /// categorization, TTL summary, and per-entry detail).
    Analyze {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
}

/// M22 — Contract verification report.
///
/// Serializes directly to the JSON schema defined in M22_PLAN.md §10.
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

/// M23 — Contract health / posture report.
///
/// Aggregates the existing read-only surfaces (`inspect_contract` +
/// `StorageAnalyzer`) plus optional M22 verification into one report.
/// Serializes to the JSON schema in M23_PLAN.md §12.
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
/// field names; omits `total_size_bytes`/`entries` per M23_PLAN.md §12).
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

/// M23 — Pure health-verdict derivation (no I/O, fully testable).
///
/// Rules (transparent, per M23_PLAN.md §10):
/// - `verified == Some(false)` → Critical (deployed != built).
/// - else `expiring_soon > 0`   → AtRisk (entries near TTL expiry).
/// - else `total_entries == 0`  → AtRisk (empty contract).
/// - else                        → Healthy.
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
            "{} storage entr{} expiring soon (< 30 days).",
            expiring_soon,
            if expiring_soon == 1 {
                "y is"
            } else {
                "ies are"
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

/// Orchestrates the M23 contract health report (read-only).
///
/// - Offline-hashes `--wasm` first (fail-fast) when supplied.
/// - Fetches on-chain WASM hash via `sdkt-rpc::inspect_contract` (no bytecode download).
/// - Fetches storage posture via `sdkt-storage::StorageAnalyzer` (no new RPC).
/// - Reuses M22 `verification_outcome` for the local-vs-onchain comparison.
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

    // Optional M22-style verification (reuse existing helper, no duplicate logic).
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

/// Pure comparison logic for M22 (no I/O, fully testable).
///
/// Given the on-chain hash and an optional local `(hash, size)` pair, returns
/// the `(match, status, explanation)` triple per M22_PLAN.md §10/§11.
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

/// Orchestrates contract verification (M22).
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
    // before touching the network), per M22_PLAN.md "Offline hashing".
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

/// Load `.sdkt.toml` from the current directory.
///
/// A missing file yields the default config (so commands like `sdkt lock
/// verify` still run meaningfully). A present-but-unparseable file (e.g. a
/// duplicate contract name or malformed TOML) is a hard error surfaced with a
/// clear message, rather than silently falling back to an empty config.
fn load_config() -> DevKitConfig {
    match DevKitConfig::from_file(".sdkt.toml") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading .sdkt.toml: {}", e);
            process::exit(1);
        }
    }
}

/// Resolve a `--input` value to envelope text.
///
/// Filesystem rules (matching the rest of the `tx` subcommands):
/// - If the path exists, read it as a file.
/// - If it does not exist but looks like a (missing) path, report a clear
///   "invalid file" error instead of silently mis-parsing it as base64.
/// - Otherwise treat the value as an inline base64 string.
fn resolve_tx_input(input: &str) -> Result<String, String> {
    if fs::metadata(input).is_ok() {
        return fs::read_to_string(input)
            .map_err(|e| format!("invalid file '{}': cannot read ({})", input, e));
    }
    let looks_like_path = input.contains('/')
        || input.contains('\\')
        || input.ends_with(".xdr")
        || input.ends_with(".txt");
    if looks_like_path {
        return Err(format!(
            "invalid file '{}': no such file or directory",
            input
        ));
    }
    Ok(input.to_string())
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
    if v.breaking_changes.is_empty() {
        println!("  (none)");
    } else {
        for c in &v.breaking_changes {
            println!("  - {}", c.label());
        }
    }
    println!();
    println!("Non-breaking:");
    if v.non_breaking_changes.is_empty() {
        println!("  (none)");
    } else {
        for c in &v.non_breaking_changes {
            println!("  - {}", c.label());
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        Commands::Storage { action, abi, net } => match action {
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
                let contract_spec = if let Some(wasm_path) = abi.as_ref() {
                    let wasm_bytes =
                        fs::read(wasm_path).map_err(|e| format!("Failed to read WASM: {}", e))?;
                    Some(
                        parse_contract_spec(&wasm_bytes)
                            .map_err(|e| format!("Failed to parse ABI: {}", e))?,
                    )
                } else {
                    None
                };

                match get_ttl_info(&client, &contract_id).await {
                    Ok(ttl_info) => {
                        if fmt == OutputFormat::Json {
                            if let Some(spec) = contract_spec {
                                let json_str = serde_json::to_string(&serde_json::json!({
                                    "contract_id": contract_id,
                                    "entries": ttl_info.entries.len(),
                                    "abi": serde_json::json!({
                                        "functions": spec.functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                                        "events": spec.events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                                        "custom_types": spec.custom_types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
                                    })
                                }))?;
                                println!("{}", json_str);
                            } else {
                                let json_str = serde_json::to_string(&ttl_info)?;
                                println!("{}", json_str);
                            }
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
            StorageAction::Estimate { wasm } => {
                println!("Storage Estimate for {} (Not yet implemented)", wasm);
            }
            StorageAction::Analyze {
                contract_id,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );
                let analyzer = sdkt_storage::StorageAnalyzer::new(client);

                match analyzer.inspect_contract_storage(&contract_id).await {
                    Ok(report) => {
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
                                println!("  Expiring Soon:  {}", summary.expiring_entries_count);
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
        },
        Commands::Inspect {
            contract_id,
            format,
            abi,
            net,
        } => {
            let fmt = parse_format_str(&format);
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
            } else {
                None
            };

            match inspect_contract(&client, &contract_id).await {
                Ok(inspection) => {
                    if fmt == OutputFormat::Json {
                        if let Some(spec) = contract_spec {
                            let json_str = serde_json::to_string(&serde_json::json!({
                                "contract_id": inspection.contract_id,
                                "wasm_hash": inspection.wasm_hash,
                                "storage_keys": inspection.storage_keys.len(),
                                "abi": serde_json::json!({
                                    "functions": spec.functions.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                                    "events": spec.events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                                    "custom_types": spec.custom_types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
                                })
                            }))?;
                            println!("{}", json_str);
                        } else {
                            let json_str = serde_json::to_string(&inspection)?;
                            println!("{}", json_str);
                        }
                    } else {
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
            net,
        } => {
            let fmt = parse_format_str(&format);
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

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
                }
                Err(e) => {
                    // Surface actionable messages per M22_PLAN.md §9.
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
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

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
                }
                Err(e) => {
                    // Surface actionable messages per M23_PLAN.md §11.
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
                                    .map_or("N/A".to_string(), |v| v.to_string())
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
                let env_data = if fs::metadata(&envelope).is_ok() {
                    fs::read_to_string(&envelope)?
                } else {
                    envelope.clone()
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
            TxAction::Simulate { envelope, format } => {
                let fmt = parse_format_str(&format);
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                let env_data = if fs::metadata(&envelope).is_ok() {
                    fs::read_to_string(&envelope)?
                } else {
                    envelope.clone()
                };

                match simulate_transaction(&client, &env_data).await {
                    Ok(sim) => {
                        if fmt == OutputFormat::Json {
                            let json_str = serde_json::to_string(&sim)?;
                            println!("{}", json_str);
                        } else {
                            println!("Simulation Result:");
                            if let Some(err) = &sim.error {
                                println!("  Status: FAILED");
                                println!("  Error: {}", err);
                            } else {
                                println!("  Status: SUCCESS");
                            }
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
                    Err(e) => {
                        eprintln!("Error simulating transaction: {}", e);
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
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                use sdkt_rpc::{submit_and_wait, PollConfig};
                use std::time::Duration;

                let env_data = if fs::metadata(&envelope).is_ok() {
                    fs::read_to_string(&envelope)?
                } else {
                    envelope.clone()
                };

                let poll_cfg = PollConfig {
                    timeout: Duration::from_secs(timeout),
                    interval: Duration::from_secs(interval),
                };

                match submit_and_wait(&client, &env_data, wait, &poll_cfg).await {
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
            } => {
                let fmt = parse_format_str(&format);

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

                let mut parsed_args = Vec::new();
                for a in arg.iter() {
                    if let Some((t, v)) = a.split_once(':') {
                        use sdkt_xdr::{scval_to_base64, Address, IntoScVal};
                        let b64 = match t.to_lowercase().as_str() {
                            "u32" => {
                                let n: u32 =
                                    v.parse().map_err(|_| format!("invalid u32 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "i32" => {
                                let n: i32 =
                                    v.parse().map_err(|_| format!("invalid i32 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "u64" => {
                                let n: u64 =
                                    v.parse().map_err(|_| format!("invalid u64 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "i64" => {
                                let n: i64 =
                                    v.parse().map_err(|_| format!("invalid i64 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "u128" => {
                                let n: u128 =
                                    v.parse().map_err(|_| format!("invalid u128 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "i128" => {
                                let n: i128 =
                                    v.parse().map_err(|_| format!("invalid i128 value: {v}"))?;
                                scval_to_base64(&n.into_scval()?)?
                            }
                            "bool" => {
                                let b: bool =
                                    v.parse().map_err(|_| format!("invalid bool value: {v}"))?;
                                scval_to_base64(&b.into_scval()?)?
                            }
                            "string" => scval_to_base64(&v.into_scval()?)?,
                            "bytes" => {
                                let mut b = Vec::new();
                                let s = v.trim();
                                for i in (0..s.len()).step_by(2) {
                                    let byte = u8::from_str_radix(&s[i..i + 2], 16)
                                        .map_err(|_| format!("invalid hex byte in: {v}"))?;
                                    b.push(byte);
                                }
                                scval_to_base64(&b.into_scval()?)?
                            }
                            "address" => {
                                let addr = Address::from_strkey(v)
                                    .map_err(|_| format!("invalid Stellar address: {v}"))?;
                                scval_to_base64(&addr.into_scval()?)?
                            }
                            _ => a.clone(), // Unknown type fallback to direct base64
                        };
                        parsed_args.push(b64);
                    } else {
                        parsed_args.push(a.clone());
                    }
                }

                let params = InvokeTransactionParams {
                    source_account,
                    sequence,
                    fee,
                    contract_id: contract.clone(),
                    function: function.clone(),
                    args: parsed_args,
                };

                match build_invoke_transaction(&params) {
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
                let signing_key = match store.load_signing_key(&identity) {
                    Ok(k) => k,
                    Err(_) => {
                        eprintln!("Error: unknown identity '{}'", identity);
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
            abi,
            net,
        } => {
            let fmt = parse_format_str(&format);
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );

            // Load ABI spec if provided
            let contract_spec = if let Some(wasm_path) = abi.as_ref() {
                let wasm_bytes =
                    fs::read(wasm_path).map_err(|e| format!("Failed to read WASM: {}", e))?;
                Some(
                    parse_contract_spec(&wasm_bytes)
                        .map_err(|e| format!("Failed to parse ABI: {}", e))?,
                )
            } else {
                None
            };

            match get_contract_events(&client, &contract_id).await {
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
                                println!("  Asset: {}", b.asset_type);
                                println!("  Balance: {}", b.balance);
                            }
                        }
                        println!("\nSigners:");
                        if account.signers.is_empty() {
                            println!("  (none)");
                        } else {
                            for s in account.signers {
                                println!("  Public Key: {}", s.public_key);
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
        } => {
            let fmt = parse_format_str(&format);
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
                        }
                        println!();
                        println!("Total changes: {}", report.total_changes());
                    }
                }
                Err(e) => {
                    eprintln!("Error diffing WASM: {}", e);
                    process::exit(1);
                }
            }
        }
        Commands::Audit {
            path,
            format,
            disable,
            rules,
        } => {
            let fmt = parse_format_str(&format);

            // Validate any --rules paths up front (Phase A: rule code must be
            // compiled into the binary; this flag validates the provided paths
            // and runs all registered rules, built-ins plus any linked plugins).
            for r in &rules {
                if !std::path::Path::new(r).exists() {
                    eprintln!("Error: rule path '{}' does not exist", r);
                    process::exit(1);
                }
            }

            let src = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read source '{}': {}", path, e))?;

            for r in &rules {
                let path_r = std::path::Path::new(r);

                // Directories pass through as M17 no-ops (validated for existence above).
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
                            if let Err(e) = sdkt_audit::load_and_register(path_r, &src) {
                                eprintln!("Error loading native plugin '{}': {}", r, e);
                                process::exit(1);
                            }
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
                            if let Err(e) = sdkt_audit::load_and_register_wasm(path_r, &src) {
                                eprintln!("Error loading WASM plugin '{}': {}", r, e);
                                process::exit(1);
                            }
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
                    "rs" => {
                        // M17 semantic: source files passed in --rules are existence-validated
                        // above but not loaded at runtime (built-in rules register themselves).
                    }
                    _ => {
                        eprintln!(
                            "Error: Unsupported plugin format: {}\n\nSupported plugin formats:\n  .so\n  .dll\n  .dylib\n  .wasm",
                            r
                        );
                        process::exit(1);
                    }
                }
            }

            // When the `plugins` feature is enabled, link the reference example
            // rule into the registry. Off by default → M16-identical behavior.
            #[cfg(feature = "plugins")]
            sdkt_audit_example_rule::register();

            let disabled_refs: Vec<&str> = disable.iter().map(String::as_str).collect();
            match sdkt_audit::audit_source_with(&src, &disabled_refs) {
                Ok(report) => {
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string(&report)?);
                    } else {
                        println!("Static Analysis Report: {}", path);
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
                    }
                }
                Err(e) => {
                    eprintln!("Error auditing source: {}", e);
                    process::exit(1);
                }
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
                    let json = serde_json::json!({
                        "file": file,
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
            } => {
                let fmt = parse_format_str(&format);
                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

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

                if fmt == OutputFormat::Json {
                    let json_str = serde_json::to_string(&meta)?;
                    println!("{}", json_str);
                } else {
                    println!("WASM Metadata:");
                    println!("Contract ID: {}", contract);
                    println!("Network: {}", network);
                    println!("WASM Hash: {}", meta.hash);
                    println!("Cache Status: {}", cache_status);
                    println!("Size: {} bytes", meta.size_bytes);
                    println!("Exports: {}", meta.exports.len());
                    println!("Imports: {}", meta.imports.len());
                    println!("Custom Sections: {}", meta.custom_sections.len());
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
                IdentityAction::Generate { name } => {
                    let identity = store.generate(&name)?;
                    println!("Identity '{}' generated successfully.", identity.name);
                    println!("Public Key: {}", identity.public_key);
                }
                IdentityAction::Import { name, secret } => {
                    let identity = store.import(&name, &secret)?;
                    println!("Identity '{}' imported successfully.", identity.name);
                    println!("Public Key: {}", identity.public_key);
                }
                IdentityAction::List => {
                    let mut list = store.list()?;
                    list.sort_by(|a, b| a.name.cmp(&b.name));
                    let default_id = store.get_default().ok();

                    if list.is_empty() {
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
                IdentityAction::Show { name } => {
                    let identity = store.get(&name)?;
                    println!("Identity: {}", identity.name);
                    println!("Public Key: {}", identity.public_key);
                }
                IdentityAction::Delete { name } => {
                    store.remove(&name)?;
                    println!("Identity '{}' removed.", name);
                }
                IdentityAction::Default { name } => {
                    store.set_default(&name)?;
                    println!("Identity '{}' set as default.", name);
                }
            }
        }
        Commands::Network { action } => {
            use sdkt_storage::{NetworkProfile, NetworkStore};
            let store = NetworkStore::new()?;
            match action {
                NetworkAction::Add {
                    name,
                    rpc_url,
                    passphrase,
                    friendbot,
                    description,
                    format,
                } => {
                    let fmt = parse_format_str(&format);
                    let mut profile = NetworkProfile::new(name.clone(), rpc_url, passphrase);
                    if let Some(url) = friendbot {
                        profile = profile.with_friendbot(url);
                    }
                    if let Some(desc) = description {
                        profile = profile.with_description(desc);
                    }
                    store.add(profile)?;
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string(&store.get(&name)?)?);
                    } else {
                        println!("Network profile '{}' saved.", name);
                    }
                }
                NetworkAction::List { format } => {
                    let fmt = parse_format_str(&format);
                    let profiles = store.list()?;
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string(&profiles)?);
                    } else if profiles.is_empty() {
                        println!("No network profiles found.");
                    } else {
                        println!("Network profiles:");
                        for p in profiles {
                            println!("  {} ({})", p.name, p.rpc_url);
                        }
                    }
                }
                NetworkAction::Show { name, format } => {
                    let fmt = parse_format_str(&format);
                    let profile = store.get(&name)?;
                    if fmt == OutputFormat::Json {
                        println!("{}", serde_json::to_string(&profile)?);
                    } else {
                        println!("Network profile: {}", profile.name);
                        println!("  RPC URL:         {}", profile.rpc_url);
                        println!("  Passphrase:      {}", profile.network_passphrase);
                        if let Some(url) = &profile.friendbot_url {
                            println!("  Friendbot URL:   {}", url);
                        }
                        if let Some(desc) = &profile.description {
                            println!("  Description:     {}", desc);
                        }
                    }
                }
                NetworkAction::Remove { name, format } => {
                    let fmt = parse_format_str(&format);
                    store.remove(&name)?;
                    if fmt == OutputFormat::Json {
                        let json = serde_json::json!({
                            "status": "removed",
                            "name": name,
                        });
                        println!("{}", serde_json::to_string(&json)?);
                    } else {
                        println!("Network profile '{}' removed.", name);
                    }
                }
            }
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
                        println!("✓ Ready to build");
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
            salt,
            format,
            deny_breaking,
            old_wasm,
            net,
        } => {
            let fmt = parse_format_str(&format);

            // Optional deploy guard: abort on a backwards-incompatible upgrade.
            if deny_breaking {
                let baseline = old_wasm.ok_or_else(|| {
                    "The --deny-breaking flag requires --old-wasm <deployed.wasm> (the currently deployed contract)".to_string()
                })?;
                let old_bytes = fs::read(&baseline)
                    .map_err(|e| format!("Failed to read OLD WASM '{}': {}", baseline, e))?;
                let new_bytes = fs::read(&wasm)
                    .map_err(|e| format!("Failed to read NEW WASM '{}': {}", wasm, e))?;
                match sdkt_wasm::upgrade_safety_wasm(&old_bytes, &new_bytes) {
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

            use sdkt_rpc::deploy_contract;
            let client = resolve_rpc_client(
                net.rpc_url.clone(),
                net.network_passphrase.clone(),
                net.network_profile.clone(),
            );
            // For CLI demo, read wasm file; if file missing, use empty bytes
            let wasm_bytes = fs::read(&wasm).unwrap_or_default();
            match deploy_contract(&client, &wasm_bytes, &salt).await {
                Ok(res) => {
                    if fmt == OutputFormat::Json {
                        println!("{}", sdkt_rpc::format_json(&res));
                    } else {
                        println!("{}", sdkt_rpc::format_pretty(&res));
                    }
                }
                Err(e) => {
                    eprintln!("Deployment error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Build => {
            let config = load_config();
            match sdkt_core::build::build_workspace(&config) {
                Ok(results) => {
                    println!("✓ Workspace built successfully");
                    for res in results {
                        println!("  ✓ {} -> {}", res.alias, res.wasm_artifact.display());
                    }
                }
                Err(e) => {
                    eprintln!("Error building workspace: {}", e);
                    std::process::exit(1);
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

                    // --- Package dependency verification (M35.2) ---
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

                // M35.2 — record resolved dependency state into sdkt.lock so
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
        },
        Commands::Project { action, net } => match action {
            ProjectCommand::Deploy { salt, format } => {
                let fmt = parse_format_str(&format);
                let config = load_config();

                // M34.1 — advisory lock check. If an `sdkt.lock` exists, warn
                // (non-fatally) when it has drifted from the current artifacts.
                // This never blocks deployment; it simply surfaces a stale-lock
                // signal so operators can re-run `sdkt build` if needed.
                let lock_report = sdkt_core::lock::verify_lock(Path::new("."), &config);
                if lock_report.present && !lock_report.consistent && fmt != OutputFormat::Json {
                    eprintln!("⚠ Warning: sdkt.lock is stale — run `sdkt build` to refresh it.");
                }

                let client = resolve_rpc_client(
                    net.rpc_url.clone(),
                    net.network_passphrase.clone(),
                    net.network_profile.clone(),
                );

                match sdkt_core::project::resolve_project(&config) {
                    Ok(resolved) => {
                        if fmt != OutputFormat::Json {
                            println!(
                                "✓ Project dependency graph resolved. Deploying {} contract(s).",
                                resolved.len()
                            );
                        }

                        let mut results = std::collections::HashMap::new();

                        for contract in resolved {
                            if fmt != OutputFormat::Json {
                                println!(
                                    "  Deploying alias '{}' from '{}'...",
                                    contract.alias,
                                    contract.wasm_artifact.display()
                                );
                            }

                            // Use alias + base salt to keep deployments unique per contract
                            let contract_salt = format!("{}_{}", salt, contract.alias);
                            let wasm_bytes =
                                fs::read(&contract.wasm_artifact).unwrap_or_else(|e| {
                                    eprintln!(
                                        "Failed to read WASM for '{}': {}",
                                        contract.alias, e
                                    );
                                    std::process::exit(1);
                                });

                            match sdkt_rpc::deploy_contract(&client, &wasm_bytes, &contract_salt)
                                .await
                            {
                                Ok(res) => {
                                    if fmt != OutputFormat::Json {
                                        println!("    ✓ Contract ID: {}", res.contract_id);
                                    }
                                    results.insert(contract.alias, res.contract_id);
                                }
                                Err(e) => {
                                    eprintln!("Deployment failed for '{}': {}", contract.alias, e);
                                    std::process::exit(1);
                                }
                            }
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
                        std::process::exit(1);
                    }
                }
            }
        },
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            // Wrap stdout so a consumer that closes the pipe early
            // (`sdkt completions bash | head`) yields EPIPE, which we treat as
            // success instead of letting clap_complete panic on it.
            let mut out = BrokenPipeOk(std::io::stdout());
            clap_complete::generate(shell, &mut cmd, "sdkt", &mut out);
        }
    }

    Ok(())
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
        assert!(reasons.iter().any(|r| r.contains("2 storage entries")));
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
