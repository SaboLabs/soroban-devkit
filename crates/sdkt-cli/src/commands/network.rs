//! Network command orchestration (`sdkt network <action>`).
//!
//! Extracted from `main.rs` as a bounded, behavior-preserving refactor: the
//! network-facing resolution helpers, probe plumbing, profile CRUD handling,
//! and identity printing live here, while the clap definitions (`NetworkArgs`,
//! `Commands::Network`) stay in `main.rs`. Shared resolvers used by other
//! command arms are re-exported `pub(crate)` with unchanged signatures.

use clap::Subcommand;
use sdkt_core::{
    DevKitConfig, NetworkConfig, OutputFormat, MAINNET_PASSPHRASE, TESTNET_PASSPHRASE,
};
use sdkt_rpc::{NetworkInfo, SorobanRpcClient};
use sdkt_storage::{NetworkProfile, NetworkStore};
use std::process;

use crate::NetworkArgs;

#[derive(Subcommand)]
pub(crate) enum NetworkAction {
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
    /// Check that a saved profile's RPC endpoint is reachable
    Check {
        /// Profile name
        name: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Read-only network identity / protocol / resource-limit diagnosis
    ///
    /// Resolves the target exactly like `verify` / `release-assurance`
    /// (explicit `--network`, or `--rpc-url` / `--network-profile` /
    /// `.sdkt.toml` defaults) and reports the passphrase identity, protocol
    /// agreement across RPC and Horizon, and the ledger limits Horizon
    /// actually observed. Never signs, submits, or deploys; when a source
    /// does not answer, the value is reported unavailable — never assumed.
    Diagnose {
        /// Network to diagnose (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
    /// Capture the live network configuration into a fuzz `NetworkProfile`
    /// file (read-only: never signs, submits, or deploys).
    ///
    /// Resolves its target exactly like `diagnose` (explicit `--network`,
    /// or `--rpc-url` / `--network-profile` / `.sdkt.toml` defaults),
    /// fetches the live configuration with `capture_network_config`,
    /// converts it with `profile_from_capture`, validates it, and writes
    /// the profile as JSON to `--out` (default `network-profile.json`).
    /// A partial capture is still written with status `INCOMPLETE` and
    /// exit 0; an unreachable endpoint or RPC error fails with a non-zero
    /// exit and writes no file.
    Capture {
        /// Network to capture (testnet | mainnet | futurenet)
        #[arg(
            short,
            long,
            value_name = "NETWORK",
            conflicts_with = "rpc_url",
            conflicts_with = "network_profile",
            conflicts_with = "network_passphrase"
        )]
        network: Option<String>,
        /// Output file for the captured profile JSON
        #[arg(long, value_name = "FILE", default_value = "network-profile.json")]
        out: String,
        /// Output format (pretty or json)
        #[arg(short, long, default_value = "pretty")]
        format: String,
        #[command(flatten)]
        net: NetworkArgs,
    },
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
pub(crate) fn apply_profile_overrides(
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
pub(crate) fn resolve_network_config(
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
pub(crate) fn resolve_rpc_client(
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

/// Target network resolution result for RPC commands that accept `--network`.
pub(crate) struct TargetNetwork {
    pub client: SorobanRpcClient,
    #[allow(dead_code)]
    pub config: NetworkConfig,
    pub network_name: String,
}

impl std::fmt::Debug for TargetNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetNetwork")
            .field("config", &self.config)
            .field("network_name", &self.network_name)
            .finish()
    }
}

/// Resolve the RPC client and canonical network name for commands that accept `--network`.
///
/// When `--network` is explicitly provided (`testnet`, `mainnet`, `futurenet`), it
/// configures the well-known endpoint/passphrase and conflicts with `--rpc-url`,
/// `--network-profile`, and `--network-passphrase`. When omitted, the network is resolved
/// from `NetworkArgs` (profile, rpc-url, or defaults), and the canonical name is derived
/// from the profile or passphrase.
///
/// `mainnet` has no built-in public RPC endpoint: SDF operates public JSON-RPC only for
/// testnet and futurenet. Explicit `--network mainnet` without `--rpc-url` therefore
/// fails with an actionable error instead of silently targeting a non-existent endpoint.
pub(crate) fn resolve_target_network(
    network: Option<&str>,
    net: &NetworkArgs,
) -> Result<TargetNetwork, String> {
    if let Some(explicit_net) = network {
        let explicit_net = explicit_net.trim();
        if !explicit_net.is_empty() {
            if net.rpc_url.is_some()
                || net.network_profile.is_some()
                || net.network_passphrase.is_some()
            {
                return Err("--network conflicts with --rpc-url, --network-passphrase, and --network-profile".to_string());
            }

            let (rpc_url, passphrase, canonical_name) =
                match explicit_net.to_ascii_lowercase().as_str() {
                    "testnet" => (
                        "https://soroban-testnet.stellar.org",
                        TESTNET_PASSPHRASE,
                        "testnet",
                    ),
                    "mainnet" => {
                        return Err(
                            "mainnet has no built-in public RPC endpoint (SDF operates public \
                             JSON-RPC for testnet and futurenet only). Provide an explicit RPC \
                             provider URL with --rpc-url <URL> and the mainnet passphrase with \
                             --network-passphrase \"Public Global Stellar Network ; September \
                             2015\", or save a profile via `sdkt network add mainnet ...`. See \
                             https://developers.stellar.org/docs/data/apis/rpc/providers"
                                .to_string(),
                        );
                    }
                    "futurenet" => (
                        "https://rpc-futurenet.stellar.org",
                        "Test SDF Future Network ; October 2022",
                        "futurenet",
                    ),
                    other => {
                        return Err(format!(
                            "invalid network '{}' (expected testnet|mainnet|futurenet)",
                            other
                        ));
                    }
                };

            let cfg = NetworkConfig {
                rpc_url: rpc_url.to_string(),
                passphrase: passphrase.to_string(),
                timeout_secs: Some(15),
                pool_max_idle_per_host: Some(100),
            };
            return Ok(TargetNetwork {
                client: SorobanRpcClient::from_config(&cfg),
                config: cfg,
                network_name: canonical_name.to_string(),
            });
        }
    }

    let cfg = resolve_network_config(
        net.rpc_url.clone(),
        net.network_passphrase.clone(),
        net.network_profile.clone(),
    )?;

    let network_name = if let Some(ref profile) = net.network_profile {
        profile.clone()
    } else if cfg.passphrase == MAINNET_PASSPHRASE || is_mainnet_rpc_url(&cfg.rpc_url) {
        "mainnet".to_string()
    } else if cfg.passphrase == "Test SDF Future Network ; October 2022"
        || is_futurenet_rpc_url(&cfg.rpc_url)
    {
        "futurenet".to_string()
    } else if cfg.passphrase == TESTNET_PASSPHRASE || is_testnet_rpc_url(&cfg.rpc_url) {
        "testnet".to_string()
    } else {
        "custom".to_string()
    };

    Ok(TargetNetwork {
        client: SorobanRpcClient::from_config(&cfg),
        config: cfg,
        network_name,
    })
}

fn is_mainnet_rpc_url(rpc_url: &str) -> bool {
    let url = rpc_url.to_ascii_lowercase();
    url.contains("stellar.org") && !url.contains("testnet") && !url.contains("futurenet")
}

fn is_futurenet_rpc_url(rpc_url: &str) -> bool {
    let url = rpc_url.to_ascii_lowercase();
    url.contains("futurenet")
}

fn is_testnet_rpc_url(rpc_url: &str) -> bool {
    let url = rpc_url.to_ascii_lowercase();
    url.contains("testnet")
}

/// Whether the operator explicitly named the target network (via `--rpc-url`,
/// `--network-passphrase`, or `--network-profile`). When this is `false` the
/// resolved [`NetworkConfig`] came entirely from built-in defaults (testnet),
/// and mutating operations must therefore refuse mainnet.
pub(crate) fn network_is_explicit(
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
pub(crate) fn resolve_rpc_client_mutating(
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

/// Outcome of an `sdkt network check <profile>` reachability probe.
///
/// Serializes to the structured JSON schema used by `--format json`:
/// `profile`, `rpc_url`, `reachable`, `status`, `latest_ledger`,
/// `protocol_version`, `configured_passphrase`, `endpoint_passphrase`,
/// `friendbot_url`, `network_info_error`, `error`. `error` describes
/// reachability/health only; unsupported `getNetwork` is reported separately.
#[derive(Debug, serde::Serialize)]
pub(crate) struct NetworkCheckOutcome {
    profile: String,
    rpc_url: String,
    reachable: bool,
    status: Option<String>,
    latest_ledger: Option<u32>,
    protocol_version: Option<u32>,
    configured_passphrase: String,
    endpoint_passphrase: Option<String>,
    friendbot_url: Option<String>,
    network_info_error: Option<String>,
    error: Option<String>,
}

impl NetworkCheckOutcome {
    /// A profile is healthy only when the RPC endpoint answered a ledger
    /// query *and* the subsequent health check reported `healthy`.
    fn is_healthy(&self) -> bool {
        self.reachable && self.error.is_none()
    }
}

/// Probe a resolved network endpoint for reachability without mutating any
/// stored profile.
///
/// `get_ledger()` is issued first because it exercises real connectivity
/// (transport/HTTP + JSON-RPC) and returns the latest ledger sequence and
/// protocol version. `get_health()` then reports the node's health string.
/// A transport/connection failure on the ledger call means the endpoint is
/// unreachable; a healthy ledger response followed by a failed or non-healthy
/// health call means the endpoint is reachable but not usable.
async fn probe_network_profile(profile: &str, cfg: &NetworkConfig) -> NetworkCheckOutcome {
    let client = SorobanRpcClient::from_config(cfg);
    let rpc_url = cfg.rpc_url.clone();

    let mut outcome = NetworkCheckOutcome {
        profile: profile.to_string(),
        rpc_url: rpc_url.clone(),
        reachable: false,
        status: None,
        latest_ledger: None,
        protocol_version: None,
        configured_passphrase: cfg.passphrase.clone(),
        endpoint_passphrase: None,
        friendbot_url: None,
        network_info_error: None,
        error: None,
    };

    match client.get_ledger().await {
        Ok(ledger) => {
            outcome.reachable = true;
            outcome.latest_ledger = Some(ledger.sequence);
            outcome.protocol_version = Some(ledger.protocol_version);

            match client.get_health().await {
                Ok(health) => {
                    outcome.status = Some(health.status.clone());
                    if !health.status.eq_ignore_ascii_case("healthy") {
                        outcome.error = Some(format!(
                            "RPC endpoint '{}' is reachable but reported health status '{}'",
                            rpc_url, health.status
                        ));
                    }
                }
                Err(e) => {
                    outcome.error = Some(format!(
                        "RPC endpoint '{}' is reachable but the health check failed: {}",
                        rpc_url, e
                    ));
                }
            }

            match client.get_network().await {
                Ok(network) => {
                    apply_network_info(&mut outcome, network);
                }
                Err(e) => {
                    outcome.network_info_error = Some(e.to_string());
                }
            }
        }
        Err(e) => {
            outcome.error = Some(format!("RPC endpoint '{}' is unreachable: {}", rpc_url, e));
        }
    }

    outcome
}

fn apply_network_info(outcome: &mut NetworkCheckOutcome, network: NetworkInfo) {
    if network.latest_ledger.is_some() {
        outcome.latest_ledger = network.latest_ledger;
    }
    outcome.protocol_version = Some(network.protocol_version);
    outcome.endpoint_passphrase = Some(network.passphrase);
    outcome.friendbot_url = network.friendbot_url;
}

fn print_network_identity(outcome: &NetworkCheckOutcome) {
    println!("  Profile passphrase: {}", outcome.configured_passphrase);
    match &outcome.endpoint_passphrase {
        Some(passphrase) => {
            println!("  Endpoint passphrase: {}", passphrase);
            if passphrase != &outcome.configured_passphrase {
                println!("  Warning: endpoint passphrase does not match the profile.");
            }
        }
        None => println!("  Endpoint passphrase: not reported by endpoint"),
    }
    if let Some(friendbot) = &outcome.friendbot_url {
        println!("  Friendbot URL:      {}", friendbot);
    }
    if let Some(err) = &outcome.network_info_error {
        println!("  Network identity:   not reported by endpoint ({})", err);
    }
}

/// Handle `sdkt network <action>`: profile CRUD plus the reachability check.
/// Convert a transport-level [`CapturedNetworkConfig`] into a fuzz
/// [`NetworkProfile`] (pure: no I/O, no network).
///
/// Every captured value carries its observation ledger, so a parameter the
/// RPC did not return stays unavailable in the profile — never a silent
/// zero or default. `source_endpoint` is the RPC URL the capture came
/// from; `captured_at_unix` is the capture time in Unix seconds.
pub(crate) fn capture_to_profile(
    captured: &sdkt_rpc::network_capture::CapturedNetworkConfig,
    source_endpoint: &str,
    captured_at_unix: u64,
) -> sdkt_fuzz::network_profile::NetworkProfile {
    use sdkt_fuzz::network_profile::CostParamEntrySnapshot;
    use sdkt_fuzz::profile_bridge::{profile_from_capture, CaptureInput};

    // A captured value is usable only with its observation ledger; fall
    // back to the capture ledger when the entry carries none.
    let pair_u64 = |c: &sdkt_rpc::network_capture::Captured<u64>| {
        c.value
            .map(|v| (v, c.observed_at_ledger.unwrap_or(captured.ledger_sequence)))
    };
    let pair_u32 = |c: &sdkt_rpc::network_capture::Captured<u32>| {
        c.value
            .map(|v| (v, c.observed_at_ledger.unwrap_or(captured.ledger_sequence)))
    };
    let pair_i64 = |c: &sdkt_rpc::network_capture::Captured<i64>| {
        c.value
            .map(|v| (v, c.observed_at_ledger.unwrap_or(captured.ledger_sequence)))
    };

    let to_snapshot =
        |params: &[sdkt_rpc::network_capture::CapturedCostParam]| -> Vec<CostParamEntrySnapshot> {
            params
                .iter()
                .map(|p| CostParamEntrySnapshot {
                    cost_type: p.cost_type.clone(),
                    index: p.index,
                    const_term: p.const_term,
                    linear_term: p.linear_term,
                })
                .collect()
        };
    let (cpu_cost_params, mem_cost_params, cost_params_observed_at) =
        match &captured.cost_params.value {
            Some(tables) => (
                Some(to_snapshot(&tables.cpu)),
                Some(to_snapshot(&tables.mem)),
                captured.cost_params.observed_at_ledger,
            ),
            None => (None, None, None),
        };

    profile_from_capture(CaptureInput {
        passphrase: captured.passphrase.clone(),
        protocol_version: captured.protocol_version,
        ledger_sequence: captured.ledger_sequence,
        source_endpoint: source_endpoint.to_string(),
        captured_at_unix,
        cpu_limit: pair_u64(&captured.cpu_limit),
        mem_limit: pair_u64(&captured.mem_limit),
        ledger_max_instructions: pair_u64(&captured.ledger_max_instructions),
        fee_rate_per_instructions_increment: pair_u64(
            &captured.fee_rate_per_instructions_increment,
        ),
        max_contract_size_bytes: pair_u32(&captured.max_contract_size_bytes),
        tx_max_size_bytes: pair_u32(&captured.tx_max_size_bytes),
        tx_max_contract_events_size_bytes: pair_u32(&captured.tx_max_contract_events_size_bytes),
        fee_contract_events_1kb: pair_i64(&captured.fee_contract_events_1kb),
        cpu_cost_params,
        mem_cost_params,
        cost_params_observed_at,
    })
}

/// Write a captured profile as pretty JSON. The caller decides when a
/// write may happen: only after a successful capture and validation, so
/// a failed capture never leaves a partial file behind.
pub(crate) fn write_captured_profile(
    profile: &sdkt_fuzz::network_profile::NetworkProfile,
    out: &str,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("cannot serialize captured profile: {e}"))?;
    std::fs::write(out, json).map_err(|e| format!("cannot write profile file '{out}': {e}"))
}

pub(crate) async fn run_network_action(
    action: crate::NetworkAction,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = NetworkStore::new()?;
    match action {
        crate::NetworkAction::Add {
            name,
            rpc_url,
            passphrase,
            friendbot,
            description,
            format,
        } => {
            let fmt = crate::parse_format_str(&format);
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
        crate::NetworkAction::List { format } => {
            let fmt = crate::parse_format_str(&format);
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
        crate::NetworkAction::Show { name, format } => {
            let fmt = crate::parse_format_str(&format);
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
        crate::NetworkAction::Remove { name, format } => {
            let fmt = crate::parse_format_str(&format);
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
        crate::NetworkAction::Check { name, format } => {
            let fmt = crate::parse_format_str(&format);

            // Resolve through the same precedence path every other
            // network-aware command uses. `Check` is strictly
            // read-only: the stored profile is never written back.
            let cfg = match resolve_network_config(None, None, Some(name.clone())) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };

            let outcome = probe_network_profile(&name, &cfg).await;

            if fmt == OutputFormat::Json {
                println!("{}", serde_json::to_string(&outcome)?);
            } else if outcome.is_healthy() {
                println!("Network profile '{}' is reachable.", outcome.profile);
                println!("  RPC URL:          {}", outcome.rpc_url);
                println!(
                    "  Status:           {}",
                    outcome.status.as_deref().unwrap_or("unknown")
                );
                if let Some(seq) = outcome.latest_ledger {
                    println!("  Latest ledger:    {}", seq);
                }
                if let Some(protocol) = outcome.protocol_version {
                    println!("  Protocol version: {}", protocol);
                }
                print_network_identity(&outcome);
            } else {
                if outcome.reachable {
                    println!(
                        "Network profile '{}' is reachable but not healthy.",
                        outcome.profile
                    );
                } else {
                    println!("Network profile '{}' is NOT reachable.", outcome.profile);
                }
                println!("  RPC URL:          {}", outcome.rpc_url);
                if let Some(seq) = outcome.latest_ledger {
                    println!("  Latest ledger:    {}", seq);
                }
                if let Some(protocol) = outcome.protocol_version {
                    println!("  Protocol version: {}", protocol);
                }
                print_network_identity(&outcome);
                if let Some(err) = &outcome.error {
                    println!("  Error:            {}", err);
                }
            }

            if !outcome.is_healthy() {
                process::exit(1);
            }
        }
        crate::NetworkAction::Diagnose {
            network,
            format,
            net,
        } => {
            let fmt = crate::parse_format_str(&format);
            crate::commands::diagnostics::run_network_diagnose(network, &net, &fmt).await?;
        }
        crate::NetworkAction::Capture {
            network,
            out,
            format,
            net,
        } => {
            let fmt = crate::parse_format_str(&format);
            let target = match resolve_target_network(network.as_deref(), &net) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            };
            // Read-only capture: fetches live configuration, never signs,
            // submits, or deploys. Any failure exits non-zero with no file.
            let captured =
                match sdkt_rpc::network_capture::capture_network_config(&target.client).await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    }
                };
            let captured_at_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let profile = capture_to_profile(&captured, &target.config.rpc_url, captured_at_unix);
            if let Err(e) = profile.validate(Some(profile.ledger_sequence)) {
                eprintln!("Error: captured profile failed validation: {e}");
                process::exit(1);
            }
            if let Err(e) = write_captured_profile(&profile, &out) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
            let status = profile.status();
            let unavailable = captured.unavailable_params();
            if fmt == OutputFormat::Json {
                let json = serde_json::json!({
                    "out": out,
                    "network": target.network_name,
                    "rpc_url": target.config.rpc_url,
                    "protocol_version": profile.protocol_version,
                    "ledger_sequence": profile.ledger_sequence,
                    "status": status.as_str(),
                    "unavailable_params": unavailable,
                });
                println!("{}", serde_json::to_string(&json)?);
            } else {
                println!("Network profile captured to '{out}'.");
                println!("  Network:          {}", target.network_name);
                println!("  RPC URL:          {}", target.config.rpc_url);
                println!("  Protocol version: {}", profile.protocol_version);
                println!("  Ledger sequence:  {}", profile.ledger_sequence);
                println!("  Status:           {}", status.as_str());
                if !unavailable.is_empty() {
                    println!("  Unavailable parameters: {}", unavailable.join(", "));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn resolve_target_network_explicit_builtins() {
        let net = NetworkArgs::default();

        // testnet
        let target = resolve_target_network(Some("testnet"), &net).unwrap();
        assert_eq!(target.network_name, "testnet");
        assert_eq!(target.config.rpc_url, "https://soroban-testnet.stellar.org");
        assert_eq!(target.config.passphrase, TESTNET_PASSPHRASE);

        // mainnet: no built-in public RPC endpoint — explicit --network mainnet
        // must fail with an actionable error instead of silently targeting a
        // non-existent endpoint.
        let err = resolve_target_network(Some("mainnet"), &net).unwrap_err();
        assert!(
            err.contains("no built-in public RPC endpoint"),
            "unexpected mainnet error: {err}"
        );
        assert!(
            err.contains("--rpc-url"),
            "mainnet error must point at --rpc-url: {err}"
        );

        // futurenet
        let target = resolve_target_network(Some("futurenet"), &net).unwrap();
        assert_eq!(target.network_name, "futurenet");
        assert_eq!(target.config.rpc_url, "https://rpc-futurenet.stellar.org");
        assert_eq!(
            target.config.passphrase,
            "Test SDF Future Network ; October 2022"
        );

        // case insensitivity
        let target = resolve_target_network(Some("FutureNet"), &net).unwrap();
        assert_eq!(target.network_name, "futurenet");

        // case insensitivity also applies to mainnet (error must be reached)
        let err = resolve_target_network(Some("MainNet"), &net).unwrap_err();
        assert!(err.contains("no built-in public RPC endpoint"));
    }

    #[test]
    fn resolve_target_network_invalid_network_error() {
        let net = NetworkArgs::default();
        let err = resolve_target_network(Some("unknown_net"), &net).unwrap_err();
        assert!(err.contains("invalid network 'unknown_net'"));
        assert!(err.contains("expected testnet|mainnet|futurenet"));
    }

    #[test]
    fn resolve_target_network_conflicts() {
        let net_rpc = NetworkArgs {
            rpc_url: Some("http://custom.rpc".to_string()),
            ..Default::default()
        };
        let err = resolve_target_network(Some("mainnet"), &net_rpc).unwrap_err();
        assert_eq!(
            err,
            "--network conflicts with --rpc-url, --network-passphrase, and --network-profile"
        );

        let net_profile = NetworkArgs {
            network_profile: Some("test-profile".to_string()),
            ..Default::default()
        };
        let err = resolve_target_network(Some("testnet"), &net_profile).unwrap_err();
        assert_eq!(
            err,
            "--network conflicts with --rpc-url, --network-passphrase, and --network-profile"
        );

        let net_pass = NetworkArgs {
            network_passphrase: Some("Custom Passphrase".to_string()),
            ..Default::default()
        };
        let err = resolve_target_network(Some("futurenet"), &net_pass).unwrap_err();
        assert_eq!(
            err,
            "--network conflicts with --rpc-url, --network-passphrase, and --network-profile"
        );
    }

    #[test]
    fn resolve_target_network_none_falls_back_to_network_args() {
        let net_default = NetworkArgs::default();
        let target = resolve_target_network(None, &net_default).unwrap();
        assert_eq!(target.network_name, "testnet");
        assert_eq!(target.config.rpc_url, "https://soroban-testnet.stellar.org");

        let net_mainnet_rpc = NetworkArgs {
            rpc_url: Some("https://soroban-rpc.stellar.org".to_string()),
            ..Default::default()
        };
        let target = resolve_target_network(None, &net_mainnet_rpc).unwrap();
        assert_eq!(target.network_name, "mainnet");

        let net_custom = NetworkArgs {
            rpc_url: Some("http://127.0.0.1:8000".to_string()),
            network_passphrase: Some("Standalone Network".to_string()),
            ..Default::default()
        };
        let target = resolve_target_network(None, &net_custom).unwrap();
        assert_eq!(target.network_name, "custom");
        assert_eq!(target.config.rpc_url, "http://127.0.0.1:8000");
    }

    // -----------------------------------------------------------------------
    // Issue #507: `sdkt network capture` — fixture-based conversion tests.
    // The fixture is a recorded CapturedNetworkConfig JSON; no live network.
    // -----------------------------------------------------------------------

    fn capture_fixture() -> sdkt_rpc::network_capture::CapturedNetworkConfig {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/captured-network-config.json");
        let text = std::fs::read_to_string(&path).expect("read capture fixture");
        serde_json::from_str(&text).expect("parse capture fixture")
    }

    #[test]
    fn capture_fixture_converts_to_complete_valid_profile() {
        use sdkt_fuzz::network_profile::ProfileStatus;

        let captured = capture_fixture();
        assert!(captured.unavailable_params().is_empty());
        let profile = capture_to_profile(
            &captured,
            "https://soroban-testnet.stellar.org",
            1_791_518_867,
        );
        assert_eq!(profile.status(), ProfileStatus::Complete);
        assert!(profile.has_complete_configuration());
        profile
            .validate(Some(profile.ledger_sequence))
            .expect("complete profile validates");
        assert_eq!(profile.protocol_version, 30);
        assert_eq!(profile.ledger_sequence, 12_345_678);
        assert_eq!(profile.config.cpu_limit.value, Some(400_000_000));
        assert_eq!(profile.config.mem_limit.value, Some(41_943_040));
        assert_eq!(
            profile.source_endpoint,
            "https://soroban-testnet.stellar.org"
        );
        // No parameter was silently substituted: every captured value kept
        // its observation ledger.
        assert_eq!(
            profile.config.cpu_limit.provenance.observed_at_ledger,
            Some(12_345_600)
        );
    }

    #[test]
    fn capture_fixture_incomplete_is_incomplete_but_validates() {
        use sdkt_fuzz::network_profile::ProfileStatus;
        use sdkt_rpc::network_capture::Captured;

        let mut captured = capture_fixture();
        captured.cpu_limit = Captured::unavailable();
        captured.cost_params = Captured::unavailable();
        assert!(!captured.unavailable_params().is_empty());

        let profile = capture_to_profile(
            &captured,
            "https://soroban-testnet.stellar.org",
            1_791_518_867,
        );
        assert_eq!(profile.status(), ProfileStatus::Incomplete);
        assert!(!profile.has_complete_configuration());
        assert_eq!(profile.config.cpu_limit.value, None);
        assert_eq!(profile.config.cost_params.value, None);
        // An incomplete capture still validates (validation is structural,
        // not completeness); the CLI writes it and exits 0.
        profile
            .validate(Some(profile.ledger_sequence))
            .expect("incomplete profile validates");
    }

    #[test]
    fn write_captured_profile_roundtrips() {
        let captured = capture_fixture();
        let profile = capture_to_profile(
            &captured,
            "https://soroban-testnet.stellar.org",
            1_791_518_867,
        );
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("network-profile.json");
        write_captured_profile(&profile, out.to_str().unwrap()).unwrap();

        let text = std::fs::read_to_string(&out).unwrap();
        let back: sdkt_fuzz::network_profile::NetworkProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(back, profile);
        back.validate(Some(back.ledger_sequence))
            .expect("written file validates");
    }
}
