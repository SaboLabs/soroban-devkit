//! XDR decoding engine for Stellar and Soroban structures.
//!
//! Handles conversion of raw Base64, Hex, or raw-byte XDR payloads into
//! standardized JSON formats (compact or pretty-printed).
//!
//! # Supported Types
//!
//! - `ScVal`
//! - `TransactionEnvelope`
//! - `TransactionResult`
//! - `TransactionMeta`
//! - `LedgerKey`
//! - `LedgerEntry`
//! - `ContractEvent` (auto + explicit)
//!
//! # Example
//!
//! ```rust
//! use sdkt_xdr::decode;
//!
//! let b64 = "AAAAAQAAAAoAAAAA"; // ScVal::I32(1)
//! let json = decode(b64, Some("scval"), sdkt_xdr::OutputFormat::Json).unwrap();
//! println!("{}", json);
//! ```

pub mod builder;
pub mod sign;
pub mod typed;
pub use builder::{
    build_invoke_transaction, decode_account_id, decode_contract_id, InvokeTransactionParams,
};
pub use sign::{
    sign_envelope_with, sign_transaction, verify_signature, Ed25519Signer, Network, Signer,
    SigningError, SigningOptions,
};
pub use typed::{
    decode_scvals, decode_scvals_ref, encode_scvals, scval_from_base64, scval_to_base64, Address,
    FromScVal, IntoScVal, ScValError,
};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;

pub use sdkt_core::OutputFormat;

use stellar_xdr::{
    ContractEvent, ContractExecutable, ContractId, Hash, LedgerEntry, LedgerEntryData, LedgerKey,
    LedgerKeyContractCode, LedgerKeyContractData, Limited, Limits, ReadXdr, ScAddress, ScVal,
    TransactionEnvelope, TransactionMeta, TransactionResult, WriteXdr,
};
use thiserror::Error;

/// Errors returned by the decoder.
#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("Base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Hex decode failed: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("XDR parse failed for type '{0}': {1}")]
    XdrParse(String, stellar_xdr::Error),
    #[error("XDR write failed: {0}")]
    XdrWrite(stellar_xdr::Error),
    #[error("Unknown XDR type: {0}")]
    TypeUnknown(String),
    #[error("Invalid input: empty payload")]
    EmptyPayload,
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Extraction error: {0}")]
    Extraction(String),
}

/// Parameters for constructing a `LedgerKey`.
pub enum LedgerKeyParams {
    /// A contract's instance data key. Takes the contract ID as a hex string.
    ContractData(String),
    /// A contract's WASM code key. Takes the WASM hash as a hex string.
    ContractCode(String),
}

/// Encodes `LedgerKeyParams` into a Base64 XDR `LedgerKey`.
pub fn encode_ledger_key(params: &LedgerKeyParams) -> Result<String, DecodeError> {
    let key = match params {
        LedgerKeyParams::ContractData(contract_id_hex) => {
            let hash_bytes = hex::decode(contract_id_hex).map_err(DecodeError::Hex)?;
            if hash_bytes.len() != 32 {
                return Err(DecodeError::Extraction(
                    "Contract ID must be 32 bytes".to_string(),
                ));
            }
            let mut contract_id = [0u8; 32];
            contract_id.copy_from_slice(&hash_bytes);

            LedgerKey::ContractData(LedgerKeyContractData {
                contract: ScAddress::Contract(ContractId(Hash(contract_id))),
                key: ScVal::LedgerKeyContractInstance,
                durability: stellar_xdr::ContractDataDurability::Persistent,
            })
        }
        LedgerKeyParams::ContractCode(wasm_hash_hex) => {
            let hash_bytes = hex::decode(wasm_hash_hex).map_err(DecodeError::Hex)?;
            if hash_bytes.len() != 32 {
                return Err(DecodeError::Extraction(
                    "WASM hash must be 32 bytes".to_string(),
                ));
            }
            let mut wasm_hash = [0u8; 32];
            wasm_hash.copy_from_slice(&hash_bytes);

            LedgerKey::ContractCode(LedgerKeyContractCode {
                hash: Hash(wasm_hash),
            })
        }
    };

    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    key.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;
    Ok(STANDARD.encode(&buf))
}

/// Extracts the WASM hash from a Base64 encoded `LedgerEntry`.
/// Traverses: LedgerEntry -> LedgerEntryData::ContractData -> ScVal::ContractInstance -> ContractExecutable::Wasm -> Hash
pub fn extract_wasm_hash(base64_ledger_entry: &str) -> Result<String, DecodeError> {
    let raw = detect_and_decode(base64_ledger_entry)?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    let entry = LedgerEntry::read_xdr(&mut l)
        .map_err(|e| DecodeError::XdrParse("LedgerEntry".to_string(), e))?;

    let data = match entry.data {
        LedgerEntryData::ContractData(d) => d,
        _ => return Err(DecodeError::Extraction("Not a ContractData entry".into())),
    };

    let instance = match data.val {
        ScVal::ContractInstance(i) => i,
        _ => return Err(DecodeError::Extraction("Not a ContractInstance".into())),
    };

    let hash = match instance.executable {
        ContractExecutable::Wasm(h) => h,
        _ => return Err(DecodeError::Extraction("Not a Wasm executable".into())),
    };

    Ok(hex::encode(hash.0))
}

/// Extracts the raw WASM bytecode from a Base64 encoded `LedgerEntry` containing a `ContractCode` entry.
pub fn extract_wasm_bytecode(base64_ledger_entry: &str) -> Result<Vec<u8>, DecodeError> {
    let raw = detect_and_decode(base64_ledger_entry)?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    let entry = LedgerEntry::read_xdr(&mut l)
        .map_err(|e| DecodeError::XdrParse("LedgerEntry".to_string(), e))?;

    let data = match entry.data {
        LedgerEntryData::ContractCode(c) => c,
        _ => return Err(DecodeError::Extraction("Not a ContractCode entry".into())),
    };

    Ok(data.code.to_vec())
}

/// Decode a base64- or hex-encoded XDR payload to JSON.
///
/// `payload` is tried as base64 first; if that fails and the string is valid
/// hex, it is decoded as hex. Raw-byte callers should use [`decode_bytes`].
///
/// # Arguments
///
/// * `payload`     – base64 or hex encoded string
/// * `type_hint`   – explicit type (`"scval"`, etc.) or `None` for auto-detection
/// * `format`      – [`OutputFormat::Json`] or [`OutputFormat::Pretty`]
///
/// # Returns
///
/// JSON string representation of the decoded XDR.
///
/// # Errors
///
/// Returns [`DecodeError`] if the input is invalid or the XDR cannot be parsed.
pub fn decode(
    payload: &str,
    type_hint: Option<&str>,
    format: OutputFormat,
) -> Result<String, DecodeError> {
    let raw = detect_and_decode(payload)?;
    let value = decode_bytes(&raw, type_hint)?;
    format_json(&value, format)
}

/// Decode raw bytes (no base64/hex pre-processing).
pub fn decode_bytes(raw: &[u8], type_hint: Option<&str>) -> Result<Value, DecodeError> {
    if raw.is_empty() {
        return Err(DecodeError::EmptyPayload);
    }

    let type_name = type_hint.unwrap_or("auto");

    match type_name.to_lowercase().as_str() {
        "scval" => decode_single::<ScVal>(raw, "ScVal"),
        "transactionenvelope" => decode_single::<TransactionEnvelope>(raw, "TransactionEnvelope"),
        "transactionresult" => decode_single::<TransactionResult>(raw, "TransactionResult"),
        "transactionmeta" => decode_single::<TransactionMeta>(raw, "TransactionMeta"),
        "ledgerkey" => decode_single::<LedgerKey>(raw, "LedgerKey"),
        "ledgerentry" => decode_single::<LedgerEntry>(raw, "LedgerEntry"),
        "contractevent" => decode_single::<ContractEvent>(raw, "ContractEvent"),
        "auto" => auto_detect(raw),
        other => Err(DecodeError::TypeUnknown(other.to_string())),
    }
}

fn decode_single<T: ReadXdr + serde::Serialize>(
    raw: &[u8],
    name: &str,
) -> Result<Value, DecodeError> {
    let mut cursor = std::io::Cursor::new(raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    T::read_xdr(&mut l)
        .map_err(|e| DecodeError::XdrParse(name.to_string(), e))
        .and_then(|v| serde_json::to_value(&v).map_err(DecodeError::Json))
}

fn auto_detect(raw: &[u8]) -> Result<Value, DecodeError> {
    if let Ok(v) = decode_single::<ScVal>(raw, "ScVal") {
        return Ok(v);
    }
    if let Ok(v) = decode_single::<TransactionEnvelope>(raw, "TransactionEnvelope") {
        return Ok(v);
    }
    if let Ok(v) = decode_single::<ContractEvent>(raw, "ContractEvent") {
        return Ok(v);
    }
    Err(DecodeError::TypeUnknown(
        "auto-detection failed for all known types".to_string(),
    ))
}

fn detect_and_decode(payload: &str) -> Result<Vec<u8>, DecodeError> {
    if payload.is_empty() {
        return Err(DecodeError::EmptyPayload);
    }

    let trimmed = payload.trim();

    let base64_likely = !trimmed.is_empty()
        && trimmed.len().is_multiple_of(4)
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');

    if base64_likely {
        if let Ok(bytes) = STANDARD.decode(trimmed) {
            return Ok(bytes);
        }
    }

    if trimmed.len().is_multiple_of(2) && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Ok(bytes) = hex::decode(trimmed) {
            return Ok(bytes);
        }
    }

    if base64_likely {
        Err(DecodeError::Base64(base64::DecodeError::InvalidLength(0)))
    } else {
        Err(DecodeError::Hex(hex::FromHexError::InvalidHexCharacter {
            c: trimmed
                .chars()
                .find(|c| !c.is_ascii_hexdigit())
                .unwrap_or('?'),
            index: 0,
        }))
    }
}

pub fn format_json(value: &Value, format: OutputFormat) -> Result<String, DecodeError> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string(value)?),
        OutputFormat::Pretty => Ok(serde_json::to_string_pretty(value)?),
    }
}

/// Estimate the serialized XDR size (in bytes) of a `WriteXdr` value.
///
/// Reuses the existing `WriteXdr` machinery — no duplicate parser. Returns the
/// exact number of bytes the payload would occupy on the wire.
pub fn estimate_xdr_size<T: WriteXdr>(value: &T) -> usize {
    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    // Best-effort: if serialization fails (e.g. value too large), report the
    // buffer length so far. Callers use this only for pre-flight checks.
    let _ = value.write_xdr(&mut l);
    buf.len()
}

/// Validate that a raw byte payload is a well-formed XDR `TransactionEnvelope`.
///
/// Returns `Ok(size)` when the payload parses, or `Err` with a message when it
/// does not. This is a pure structural check (no RPC).
pub fn validate_xdr(raw: &[u8]) -> Result<usize, String> {
    if raw.is_empty() {
        return Err("empty payload".into());
    }
    let mut cursor = std::io::Cursor::new(raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    TransactionEnvelope::read_xdr(&mut l)
        .map_err(|e| format!("malformed transaction envelope: {e}"))
        .map(|_| raw.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        ContractDataDurability, ContractDataEntry, ContractExecutable, ExtensionPoint, Hash,
        LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerKey, ScAddress, ScContractInstance,
        ScVal, WriteXdr,
    };

    #[test]
    fn test_invalid_base64() {
        let result = decode("!!! ???", None, OutputFormat::default());
        assert!(matches!(result, Err(DecodeError::Hex(_))));
    }

    #[test]
    fn test_valid_base64_but_invalid_xdr() {
        // Base64 for "hello world" which is not valid XDR
        let result = decode("aGVsbG8gd29ybGQ=", None, OutputFormat::default());
        assert!(matches!(result, Err(DecodeError::TypeUnknown(_))));
    }

    #[test]
    fn test_valid_scval_integer_base64() {
        let payload = "AAAABAAAAAE=";
        let json = decode(payload, Some("scval"), OutputFormat::Json).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.is_object());
        assert_eq!(v["i32"], 1);
    }

    #[test]
    fn test_auto_decode_scval() {
        let payload = "AAAABAAAAAE=";
        let json = decode(payload, None, OutputFormat::Json).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.is_object());
        assert_eq!(v["i32"], 1);
    }

    #[test]
    fn test_empty_payload() {
        let result = decode("", None, OutputFormat::default());
        assert!(matches!(result, Err(DecodeError::EmptyPayload)));
    }

    #[test]
    fn test_unknown_type() {
        let payload = "AAAABAAAAAE=";
        let result = decode(payload, Some("nonexistent"), OutputFormat::default());
        assert!(matches!(result, Err(DecodeError::TypeUnknown(_))));
    }

    #[test]
    fn test_json_vs_pretty() {
        let payload = "AAAABAAAAAE=";
        let compact = decode(payload, Some("scval"), OutputFormat::Json).unwrap();
        let pretty = decode(payload, Some("scval"), OutputFormat::Pretty).unwrap();
        assert!(!compact.contains('\n'));
        assert!(pretty.contains('\n'));
    }

    #[test]
    fn test_encode_ledger_key() {
        let contract_id = "0000000000000000000000000000000000000000000000000000000000000000";
        let res =
            encode_ledger_key(&LedgerKeyParams::ContractData(contract_id.to_string())).unwrap();
        // Decode it back to verify it's a LedgerKey::ContractData with ScVal::LedgerKeyContractInstance
        let decoded = detect_and_decode(&res).unwrap();
        let mut cursor = std::io::Cursor::new(&decoded);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let lk = LedgerKey::read_xdr(&mut l).unwrap();
        match lk {
            LedgerKey::ContractData(d) => {
                assert_eq!(d.key, ScVal::LedgerKeyContractInstance);
            }
            _ => panic!("Expected ContractData"),
        }
    }

    fn create_test_ledger_entry(executable: ContractExecutable) -> String {
        let entry = LedgerEntry {
            last_modified_ledger_seq: 1,
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable,
                    storage: None,
                }),
            }),
            ext: LedgerEntryExt::V0,
        };
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        let mut l = Limited::new(&mut cursor, Limits::none());
        entry.write_xdr(&mut l).unwrap();
        STANDARD.encode(&buf)
    }

    #[test]
    fn test_extract_wasm_hash_valid() {
        let hash = [1u8; 32];
        let b64 = create_test_ledger_entry(ContractExecutable::Wasm(Hash(hash)));
        let extracted = extract_wasm_hash(&b64).unwrap();
        assert_eq!(extracted, hex::encode(hash));
    }

    #[test]
    fn test_extract_wasm_hash_non_wasm() {
        let b64 = create_test_ledger_entry(ContractExecutable::StellarAsset);
        let err = extract_wasm_hash(&b64).unwrap_err();
        assert!(matches!(err, DecodeError::Extraction(e) if e.contains("Not a Wasm")));
    }

    #[test]
    fn test_extract_wasm_hash_non_ledger_entry() {
        // Just an ScVal
        let b64 = "AAAABAAAAAE=";
        let err = extract_wasm_hash(b64).unwrap_err();
        assert!(matches!(err, DecodeError::XdrParse(_, _)));
    }

    #[test]
    fn test_extract_wasm_hash_malformed_base64() {
        let err = extract_wasm_hash("invalid base64!!!").unwrap_err();
        assert!(matches!(err, DecodeError::Hex(_))); // detect_and_decode falls back to Hex and fails there
    }

    #[test]
    fn test_estimate_xdr_size() {
        let val = ScVal::U32(42);
        // Tag(4) + U32(4) = 8 bytes
        assert_eq!(estimate_xdr_size(&val), 8);
    }
}

pub mod abi_decode;
