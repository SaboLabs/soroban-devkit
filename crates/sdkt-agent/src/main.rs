//! `sdkt-agent` — read-only natural-language front end for the Soroban DevKet.
//!
/// ``text
/// sdkt-agent "check health of contract C... on testnet"
/// sdkt-agent --format json "is it safe to upgrade from a.wasm to b.wasm"
/// sdkt-agent --list-capabilities --format json
/// ```
///
/// The planner is deterministic (no model); see the crate docs for the
/// pipeline and the safety posture. Exit codes mirror `sdkt`'s convention:
/// `0` success, `1 failed or refused, `2` usage error.

use clap::Parser;
use sdkt_agent::{plan, ExecutionResult, Planned, Status};

/// Read-only natural-language front end for sdkt.
#[derive(Parser, Debug)]
#[command(
    name = "sdkt-agent",
    version,
    about = "Plan and run read-only sdkt capabilities from a plain-English request",
    long_about = None,
)]
struct Cli {
    /// The request, in plain language.
    /// Required unless `--list-capabilities` is given.
    #[arg(value_name = "REQUEST")]
    request: Option<String>,

    /// Output format: pretty (default) or json.
    #[arg(short, long, default_value = "pretty")]
    format: String,

    /// List the capability registry and exit 0. Any REQUEST is ignored.
    #[arg(long)]
    list_capabilities: bool,
}

fn main() {
    let cli = Cli::parse();

    let json = match cli.format.to_ascii_lowercase().as_str() {
        "json" => true,
        "pretty" => false,
        other => {
            eprintln!("Error: invalid format '{other}' (expected pretty|json)");
            std::process::exit(2);
        }
    };

    if cli.list_capabilities {
        // Listing never consults the parser or planner and always exits 0.
        if json {
            match sdkt_core::registry::to_json() {
                Ok(doc) => println!("{doc}"),
                Err(e) => {
                    eprintln!("Error: failed to serialize registry: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            for cap in sdkt_core::registry::capabilities() {
                println!(
                    "{}\t{}\t{:?}\tnetwork={:?}\t{}",
                    cap.id, cap.command, cap.safety, cap.network, cap.summary
                );
            }
        }
        return;
    }

    let request = match cli.request {
        Some(r) => r,
        None => {
            eprintln!("Error: a REQUST is required unless --list-capabilities is given");
            eprintln!("Usage: sdkt-agent [OPTIONS] <REQUEST>");
            eprintln!("Try 'sdkt-agent --help' for more information.");
            std::process::exit(2);
        }
    };

    let mut result = match plan(&request) {
        Planned::Run(p) => {
            // Only reached for read-only capabilities: `plan` refuses
            // everything else before we get here.
            let raw = sdkt_agent::execute(&p.argv);
            ExecutionResult::from_execution(&p, &raw)
        }
        Planned::Refuse(e) => ExecutionResult::refused(&request, &e),
    };
    result.request = request.clone();

    if json {
        // stdout carries the JSON document alone; anything diagnostic is
        // already inside the document (or on stderr from the child).
        println!("{}", result.to_json());
    } else {
        println!("{}", result.to_pretty());
    }

    // Exit status mirrors the outcome so a caller can branch without parsing.
    match result.status {
        Status::Success => {}
        Status::Failed | Status::Blocked | Status::NeedsClarification => std::process::exit(1),
    }
}
