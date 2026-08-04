use clap::{Parser, Subcommand};
use sdkt_core::fee::{FeeConfig, FeeEstimator, LedgerFeeSample, NetworkKind};
use sdkt_core::{DevKitConfig, OutputFormat};
use sdkt_rpc::{
    estimate_dynamic_fee, get_contract_events, get_ttl_info, get_wasm_metadata, inspect_account,
    inspect_contract, inspect_transaction, simulate_transaction, SorobanRpcClient,
};
use sdkt_storage::WasmCache;
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
    },
    /// Inspect a contract's ABI and storage
    Inspect {
        contract_id: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
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
    /// Manage Soroban identities (keys)
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
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
        Commands::Storage { action } => match action {
            StorageAction::Check {
                contract_id,
                format,
            } => {
                let fmt = parse_format_str(&format);
                let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
                let client = SorobanRpcClient::from_config(&config.network);

                match get_ttl_info(&client, &contract_id).await {
                    Ok(ttl_info) => {
                        if fmt == OutputFormat::Json {
                            let json_str = serde_json::to_string(&ttl_info)?;
                            println!("{}", json_str);
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
        },
        Commands::Inspect {
            contract_id,
            format,
        } => {
            let fmt = parse_format_str(&format);
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);

            match inspect_contract(&client, &contract_id).await {
                Ok(inspection) => {
                    if fmt == OutputFormat::Json {
                        let json_str = serde_json::to_string(&inspection)?;
                        println!("{}", json_str);
                    } else {
                        println!("Contract Inspection");
                        println!("Contract ID: {}", inspection.contract_id);
                        println!("WASM Hash: {}", inspection.wasm_hash);
                        println!("Storage Keys: {}", inspection.storage_keys.len());
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
                        use sdkt_xdr::{scval_to_base64, IntoScVal};
                        let b64 = match t.to_lowercase().as_str() {
                            "u32" => {
                                let n: u32 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "i32" => {
                                let n: i32 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "u64" => {
                                let n: u64 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "i64" => {
                                let n: i64 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "u128" => {
                                let n: u128 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "i128" => {
                                let n: i128 = v.parse().unwrap();
                                scval_to_base64(&n.into_scval().unwrap()).unwrap()
                            }
                            "bool" => {
                                let b: bool = v.parse().unwrap();
                                scval_to_base64(&b.into_scval().unwrap()).unwrap()
                            }
                            "string" => scval_to_base64(&v.into_scval().unwrap()).unwrap(),
                            "bytes" => {
                                let mut b = Vec::new();
                                let s = v.trim();
                                for i in (0..s.len()).step_by(2) {
                                    let byte = u8::from_str_radix(&s[i..i + 2], 16).unwrap();
                                    b.push(byte);
                                }
                                scval_to_base64(&b.into_scval().unwrap()).unwrap()
                            }
                            "address" => {
                                use sdkt_xdr::Address;
                                let addr = Address::from_strkey(v).unwrap();
                                scval_to_base64(&addr.into_scval().unwrap()).unwrap()
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
        } => {
            let fmt = parse_format_str(&format);
            let config = DevKitConfig::from_file(".sdkt.toml").unwrap_or_default();
            let client = SorobanRpcClient::from_config(&config.network);

            match get_contract_events(&client, &contract_id).await {
                Ok(events) => {
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
                let cache = WasmCache::new().unwrap_or_else(|_| {
                    eprintln!("Could not initialize cache");
                    process::exit(1);
                });

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
    }

    Ok(())
}
