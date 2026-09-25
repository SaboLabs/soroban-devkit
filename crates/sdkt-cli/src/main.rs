use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use sdkt_core::fee::{FeeConfig, FeeEstimator, LedgerFeeSample, NetworkKind};
use sdkt_core::fetch::DependencyFetcher;
use sdkt_core::{DevKitConfig, NetworkConfig, OutputFormat};
use sdkt_rpc::inspect::StorageSummary;
use sdkt_rpc::wasm::get_wasm_bytecode;
use sdkt_rpc::{
    estimate_dynamic_fee, extend_footprint, get_contract_events, get_next_sequence, get_ttl_info,
    get_wasm_metadata, inspect_account, inspect_contract, inspect_transaction, read_contract_state,
    simulate_transaction, SorobanRpcClient, StorageKeyInfo, TtlInfoSummary,
};
use sdkt_storage::WasmCache;
use sdkt_storage::{NetworkProfile, NetworkStore, StorageAnalyzer};
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
/// precedence and is unit-tested directly.
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

/// Whether the operator explicitly named the target network (via `--rpc-url`,
/// `--network-passphrase`, or `--network-profile`). When this is `false` the
/// resolved `NetworkConfig` came entirely from built-in defaults (testnet),
/// and mutating operations must therefore refuse mainnet.
fn network_is_explicit(
    rpc_url: &Option<String>,
    network_passphrase: &Option<String>,
    network_profile: &Option<String>,
) -> bool {
    rpc_url.is_some() || network_passphrase.is_some() || network_profile.is_some()
}

/// Build a [`SorobanRpcClient`] for a *mutating* (state-changing) RPC operation.
///
/// This reuses the existing resolution path and then applies the conservative
/// mainnet-safety guard from `sdkt_core::guard_mutating_network`. A mutating
/// command is only allowed to touch mainnet when the operator has explicitly
/// selected the network; an implicit testnet default combined with a mainnet
/// endpoint/passphrase is rejected before any request leaves the process.
///
/// Resolution or guard failures print a clear message and exit non-zero.
fn resolve_rpc_client_mutating(
    rpc_url: Option<String>,
    network_passphrase: Option<String>,
    network_profile: Option<String>,
) -> SorobanRpcClient {
    let explicit = network_is_explicit(&rpc_url, &network_passphrase, &network_profile);
    let cfg = match resolve_network_config(
        rpc_url.clone(),
        network_passphrase.clone(),
        network_profile.clone(),
    ) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };
    if let Err(e) = sdkt_core::guard_mutating_network(&cfg, explicit) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
    SorobanRpcClient::from_config(&cfg)
}

/// Adapter that makes a closed consumer (EPIPE / `BrokenPipe`) look like a
/// successful write.
///
/// `clap_complete::generate` writes the script to the provided `Write` and (in this version) unwraps write errors internally. When the consumer closes the pipe early — e.g. `sdkt completions bash | head` — the underlying write fails with `BrokenPipe`, which would otherwise panic. By mapping that one error to `Ok`, downstream writers never see it and `sdkt` exits cleanly.
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

// The remainder of this file is unchanged. Only the event RPC invocation
// below is adapted to the current sdkt-rpc pagination API.

