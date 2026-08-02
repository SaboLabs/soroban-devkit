//! CLI entry point for the Soroban DevKit (sdkt).
//!
//! This binary provides subcommands for decoding XDR:
//! - `decode`: Convert base64/hex XDR to human-readable JSON

use clap::{Parser, Subcommand};
use sdkt_xdr::{decode, OutputFormat};
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
        /// Base64-encoded XDR string (e.g., "AAAAA...")
        #[arg(value_name = "XDR")]
        payload: String,

        /// XDR type to decode (auto-detected if omitted)
        #[arg(short, long, value_name = "TYPE")]
        r#type: Option<String>,

        /// Output format: "json" (compact) or "pretty" (default)
        #[arg(short, long, value_name = "FORMAT", default_value = "pretty")]
        format: String,

        /// Read input from file instead of argument
        #[arg(short = 'i', long, value_name = "FILE")]
        file: Option<String>,
    },
}

fn parse_format(s: &str) -> OutputFormat {
    match s.to_lowercase().as_str() {
        "json" => OutputFormat::Json,
        "pretty" => OutputFormat::Pretty,
        other => {
            eprintln!("Invalid format '{}'. Use 'json' or 'pretty'.", other);
            process::exit(1);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

            let fmt = parse_format(&format);
            let json = decode(&input, r#type.as_deref(), fmt)?;
            println!("{}", json);
        }
    }

    Ok(())
}
