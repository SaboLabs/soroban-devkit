use clap::{Parser, Subcommand};
use sdkt_core::fee::{FeeConfig, FeeEstimator, LedgerFeeSample, NetworkKind};
use sdkt_core::{DevKitConfig, OutputFormat};
use sdkt_rpc::{
    estimate_dynamic_fee, get_contract_events, get_ttl_info, get_wasm_metadata, inspect_account,
    inspect_contract, inspect_transaction, simulate_transaction, SorobanRpcClient,
};
use sdkt_storage::WasmCache;
use sdkt_wasm::spec::parse_contract_spec;
use sdkt_xdr::abi_decode::decode_event_topics;
use sdkt_xdr::decode;
use sdkt_xdr::{build_invoke_transaction, InvokeTransactionParams};
use std::fs;
use std::process;

/// Soroban DevKit — unified toolkit for Stellar/Soroban development.
#[derive(Parser)]
#[command(name = "sdkt")]
#[command(about = "Soroban DevKit — unified toolkit for Stellar/Soroban development")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decode base64-encoded XDR to JSON
    Decode {
        #[arg(value_name = "XDR")]
        payload: String,
        #[arg(short, long, value_name = "TYPE")]
        r#type: Option<String>,
        #[arg(short, long, value_name = "FORMAT", default_value = "pretty")]
        format: String,
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
    },
    /// Inspect a contract's ABI and storage
    Inspect {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Path to contract WASM for ABI-aware storage inspection
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
    },
    /// Inspect a Soroban transaction
    Tx {
        #[command(subcommand)]
        action: TxAction,
    },
    /// Event explorer
    Events {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
        /// Path to contract WASM for ABI-aware decoding
        #[arg(long, value_name = "WASM")]
        abi: Option<String>,
    },
    /// Inspect an account's balances and signers
    Account {
        address: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
    },
    /// Estimate transaction fee from recent ledger base fees
    Fee {
        #[command(subcommand)]
        action: FeeAction,
    },
    /// Manage WASM metadata and caching
    Wasm {
        #[command(subcommand)]
        action: WasmAction,
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
enum WasmAction {
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
    /// Build a transaction envelope XDR
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
            let input = if let Some(path) = file {
                fs::read_to_string(&path)?
            } else {
                payload
            };

            let fmt = parse_format_str(&format);
            let json = decode(&input, r#type.as_deref(), fmt)?;
            println!("{}", json);
        }
        Commands::Storage { action, abi } => match action {
            StorageAction::Check {
                contract_id,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

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
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);
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
        } => {
            let fmt = parse_format_str(&format);
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);

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
        Commands::Tx { action } => match action {
            TxAction::Inspect { hash, format } => {
                let fmt = parse_format_str(&format);
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

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
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

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
                            if let Some(cost) = &sim.cost {
                                println!("  Cost:");
                                println!("    CPU Instructions: {}", cost.cpu_insns);
                                println!("    Memory Bytes: {}", cost.mem_bytes);
                            }
                            if !sim.events.is_empty() {
                                println!("  Events: {} emitted", sim.events.len());
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
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

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
        },
        Commands::Events {
            contract_id,
            format,
            abi,
        } => {
            let fmt = parse_format_str(&format);
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);

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
        Commands::Account { address, format } => {
            let fmt = parse_format_str(&format);
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);

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
        Commands::Fee { action } => match action {
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
                    let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                    let client = SorobanRpcClient::from_config(&config.network);
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

            // Dynamic plugin loading (M18, Phase B). Only `.so`/`.dylib`/`.dll`
            // artifacts are treated as loadable plugins; other paths keep the
            // M17 semantics (validated above, no-op for execution).
            #[cfg(feature = "plugins")]
            {
                let plugin_exts = ["so", "dylib", "dll"];
                for r in &rules {
                    let is_plugin = std::path::Path::new(r)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| plugin_exts.contains(&e.to_ascii_lowercase().as_str()))
                        .unwrap_or(false);
                    if is_plugin {
                        if let Err(e) = sdkt_audit::plugin_loader::load_and_register(
                            std::path::Path::new(r),
                            &src,
                        ) {
                            eprintln!("Error loading plugin '{}': {}", r, e);
                            process::exit(1);
                        }
                    }
                }
            }
            #[cfg(not(feature = "plugins"))]
            {
                let plugin_exts = ["so", "dylib", "dll"];
                for r in &rules {
                    let is_plugin = std::path::Path::new(r)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| plugin_exts.contains(&e.to_ascii_lowercase().as_str()))
                        .unwrap_or(false);
                    if is_plugin {
                        eprintln!(
                            "Error: '{}' is a plugin artifact but this build was compiled \
                             without the `plugins` feature. Rebuild with --features plugins.",
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
        Commands::Wasm { action } => match action {
            WasmAction::Metadata {
                contract,
                network,
                refresh,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

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
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);
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
    }

    Ok(())
}
