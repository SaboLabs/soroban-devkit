//! `sdkt-agent` — read-only natural-language front end for the Soroban DevKit.
//!
//! ```text
//! sdkt-agent "check health of contract C... on testnet"
//! sdkt-agent --format json "is it safe to upgrade from a.wasm to b.wasm"
//! ```
//!
//! The planner is deterministic (no model); see the crate docs for the
//! pipeline and the safety posture. Exit codes mirror `sdkt`'s convention:
//! `0` success, `1` failed or refused, `2` usage error.

use clap::Parser;
use sdkt_agent::{plan, ExecutionResult, Planned, Status};
use sdkt_core::registry;

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
    #[arg(value_name = "REQUEST")]
    request: Option<String>,

    /// List the installed capability registry without planning or executing a request.
    #[arg(long)]
    list_capabilities: bool,

    /// Output format: pretty (default) or json.
    #[arg(short, long, default_value = "pretty")]
    format: String,
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
        if json {
            match registry::to_json() {
                Ok(document) => println!("{document}"),
                Err(error) => {
                    eprintln!("Error serializing capability registry: {error}");
                    std::process::exit(1);
                }
            }
        } else {
            for capability in registry::capabilities() {
                println!(
                    "{} | {} | {:?} | {:?} | {}",
                    capability.id,
                    capability.argv(),
                    capability.safety,
                    capability.network,
                    capability.summary
                );
            }
        }
        return;
    }

    let Some(request) = cli.request.as_deref() else {
        eprintln!("error: the following required argument was not provided: REQUEST");
        std::process::exit(2);
    };

    let mut result = match plan(request) {
        Planned::Run(p) => {
            // Only reached for read-only capabilities: `plan` refuses
            // everything else before we get here.
            let raw = sdkt_agent::execute(&p.argv);
            ExecutionResult::from_execution(&p, &raw)
        }
        Planned::Refuse(e) => ExecutionResult::refused(request, &e),
    };
    result.request = request.to_string();

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
