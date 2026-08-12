//! Browser glue for the Soroban DevKit WASM inspector.
//!
//! This crate contains **no inspection logic of its own**. It is a thin
//! `wasm-bindgen` wrapper that forwards raw contract bytes to the same
//! `sdkt-wasm` functions the `sdkt wasm inspect` CLI command uses:
//!
//! - [`sdkt_wasm::parse_metadata`]      — hash, size, version, exports, imports, custom sections
//! - [`sdkt_wasm::parse_contract_spec`] — functions, custom types, events, env metadata
//!
//! Keeping the wrapper this small is deliberate: the CLI and the Web Playground
//! must not diverge in parsing behaviour.
//!
//! Build (browser only — this crate is excluded from the native workspace):
//! ```text
//! cargo build -p sdkt-playground --release --target wasm32-unknown-unknown
//! wasm-bindgen --target web --no-typescript \
//!   --out-dir website/playground/wasm \
//!   target/wasm32-unknown-unknown/release/sdkt_playground.wasm
//! ```

use sdkt_wasm::{parse_contract_spec, parse_metadata, WasmError};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Result payload handed to JavaScript.
///
/// `spec` mirrors the CLI: the contract spec is **optional**, because a WASM
/// binary without a `contractspecv0` section is still a valid module — the CLI
/// prints "Contract Spec Available: No" in that case rather than failing.
#[derive(Serialize, Debug)]
pub struct InspectionResult {
    pub metadata: sdkt_wasm::WasmMetadata,
    pub spec: Option<sdkt_wasm::ContractSpec>,
    /// Present when metadata parsed but the contract spec did not. Purely
    /// informational; never a stack trace.
    pub spec_error: Option<String>,
}

/// Map a [`WasmError`] to a stable, user-facing message.
///
/// Deliberately does **not** use `Display` on the underlying parser errors for
/// the failure cases where the wasmparser message would leak byte offsets and
/// internal wording; those are summarised instead. No panics, no backtraces.
fn user_message(err: &WasmError) -> String {
    match err {
        WasmError::Empty => "The file is empty. Select a compiled .wasm contract.".to_string(),
        WasmError::Parse(_) => concat!(
            "This file is not a valid WebAssembly module. ",
            "It may be corrupted, truncated, or not a .wasm file at all."
        )
        .to_string(),
        WasmError::NoContractSpec => concat!(
            "No `contractspecv0` section found. ",
            "The module is valid WebAssembly but does not carry a Soroban contract spec."
        )
        .to_string(),
        WasmError::SpecXdr(_) => concat!(
            "The `contractspecv0` section could not be decoded as Stellar XDR. ",
            "The contract spec may be malformed or built with an incompatible SDK."
        )
        .to_string(),
    }
}

/// Inspect raw Soroban contract WASM bytes.
///
/// Returns a plain JS object `{ metadata, spec, spec_error }` on success, or a
/// `string` error message on failure. Everything runs inside the caller's
/// WebAssembly instance — no I/O, no network, no filesystem access.
#[wasm_bindgen]
pub fn inspect_wasm(bytes: &[u8]) -> Result<JsValue, JsValue> {
    // Metadata is mandatory: if the module itself will not parse there is
    // nothing meaningful to show.
    let metadata = parse_metadata(bytes).map_err(|e| JsValue::from_str(&user_message(&e)))?;

    // Spec is optional, exactly as in the CLI (`parse_contract_spec(..).ok()`).
    let (spec, spec_error) = match parse_contract_spec(bytes) {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(user_message(&e))),
    };

    let result = InspectionResult {
        metadata,
        spec,
        spec_error,
    };

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Version of the toolkit this playground build was produced from.
#[wasm_bindgen]
pub fn sdkt_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------------------
// Native test surface (not exported to JS).
//
// #[wasm_bindgen] functions cannot be called from the native test runner, so
// the pure inspection logic lives here and `inspect_wasm` delegates to it.
// The browser wrapper is therefore exactly as thin as intended: the same
// functions, the same error mapping, one hop.
// ---------------------------------------------------------------------------

/// The inspection pipeline, minus wasm-bindgen.
pub fn inspect_parts(bytes: &[u8]) -> Result<InspectionResult, WasmError> {
    let metadata = parse_metadata(bytes)?;
    let (spec, spec_error) = match parse_contract_spec(bytes) {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(user_message(&e))),
    };
    Ok(InspectionResult {
        metadata,
        spec,
        spec_error,
    })
}

/// Expose the error mapping for tests without requiring a wasm target.
pub fn user_message_for_test(err: &WasmError) -> String {
    user_message(err)
}
