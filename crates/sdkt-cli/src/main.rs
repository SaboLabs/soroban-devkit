use clap::{Parser, Subcommand};
use sdkt_core::{DevKitConfig, OutputFormat};
use sdkt_rpc::{
    get_contract_events, get_ttl_info, inspect_contract, inspect_transaction, SorobanRpcClient,
};
use sdkt_xdr::decode;
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
}

#[derive(Subcommand)]
enum TxAction {
    Inspect {
        hash: String,
        #[arg(short, long, default_value = "pretty")]
        format: String,
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
    }

    Ok(())
}
