//! Inspect a committed Soroban contract without an RPC connection.
//! Run with: cargo run -p sdkt-wasm --example inspect_wasm

use sdkt_wasm::{parse_contract_spec, parse_metadata};

const WASM: &[u8] = include_bytes!("../../sdkt-cli/tests/fixtures/us_new.wasm");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let metadata = parse_metadata(WASM)?;
    let spec = parse_contract_spec(WASM)?;

    println!("WASM SHA-256: {}", metadata.hash);
    println!("Size: {} bytes", metadata.size_bytes);
    println!("Custom sections: {}", metadata.custom_sections.join(", "));
    println!("Contract functions:");
    for function in &spec.functions {
        println!("  {} ({} parameters)", function.name, function.parameters.len());
    }

    Ok(())
}
