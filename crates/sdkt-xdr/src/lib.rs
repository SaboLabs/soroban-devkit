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

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use stellar_xdr::{
    ContractEvent, LedgerEntry, LedgerKey, Limited, Limits, ReadXdr, ScVal, TransactionEnvelope,
    TransactionMeta, TransactionResult,
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
    #[error("Unknown XDR type: {0}")]
    TypeUnknown(String),
    #[error("Invalid input: empty payload")]
    EmptyPayload,
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Output formatting preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    Json,
    #[default]
    Pretty,
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
    let mut l = Limited::new(raw, Limits::none());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_base64() {
        // Non-base64, non-hex characters trigger hex error
        let result = decode("!!! ???", None, OutputFormat::default());
        assert!(matches!(result, Err(DecodeError::Hex(_))));
    }

    #[test]
    fn test_valid_scval_integer_base64() {
        // ScVal::I32(1): discriminant=4, payload=1
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
        // Valid XDR but unknown type name
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
}
