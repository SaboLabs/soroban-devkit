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
    request: String,

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

    let mut result = match plan(&cli.request) {
        Planned::Run(p) => {
            // Only reached for read-only capabilities: `plan` refuses
            // everything else before we get here.
            let raw = sdkt_agent::execute(&p.argv);
            ExecutionResult::from_execution(&p, &raw)
        }
        Planned::Refuse(e) => ExecutionResult::refused(&cli.request, &e),
    };
    result.request = cli.request.clone();

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
