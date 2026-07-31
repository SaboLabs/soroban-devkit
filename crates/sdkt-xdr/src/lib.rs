//! XDR decoding engine for Stellar and Soroban structures.
//!
//! This module handles the conversion of raw Base64 or Hex encoded XDR payloads
//! into standardized, human-readable JSON formats.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::Value;
use stellar_xdr::{
    LedgerEntry, LedgerKey, Limits, ReadXdr, ScVal, TransactionEnvelope, TransactionMeta,
    TransactionResult,
};

/// Error states encountered during decoding operations.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// Provided string is not valid Base64 format.
    #[error("Failed to decode Base64 content: {0}")]
    Base64(#[from] base64::DecodeError),

    /// Provided string is not valid Hex format.
    #[error("Failed to decode Hex content: {0}")]
    Hex(#[from] hex::FromHexError),

    /// Byte array could not be mapped to any valid Stellar/Soroban XDR schema.
    #[error("Failed to parse XDR bytes for type {0}: {1}")]
    XdrParse(String, stellar_xdr::Error),

    /// Auto-detection failed to find a matching XDR type variant.
    #[error("Could not automatically determine XDR type for the given payload")]
    TypeUnknown,
}

/// Dynamic XDR decoding helper.
pub struct XdrDecoder;

impl XdrDecoder {
    /// Attempts to parse raw input string (detecting Base64 or Hex automatically)
    /// and decodes it into a formatted JSON structure using a specified XDR type.
    pub fn decode_to_json(input: &str, xdr_type: &str) -> Result<Value, DecodeError> {
        let clean_input = input.trim();
        let bytes = if let Ok(decoded) = hex::decode(clean_input) {
            decoded
        } else {
            BASE64_STANDARD.decode(clean_input)?
        };

        Self::decode_bytes_to_json(&bytes, xdr_type)
    }

    /// Decodes raw binary bytes to JSON based on the designated XDR type string.
    pub fn decode_bytes_to_json(bytes: &[u8], xdr_type: &str) -> Result<Value, DecodeError> {
        let mut read_bytes = bytes;

        let json_val = match xdr_type.to_lowercase().replace('_', "").as_str() {
            "transactionenvelope" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = TransactionEnvelope::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            "transactionresult" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = TransactionResult::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            "transactionmeta" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = TransactionMeta::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            "scval" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = ScVal::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            "ledgerkey" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = LedgerKey::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            "ledgerentry" => {
                let mut limited = stellar_xdr::Limited::new(&mut read_bytes, Limits::none());
                let parsed = LedgerEntry::read_xdr(&mut limited)
                    .map_err(|e| DecodeError::XdrParse(xdr_type.to_string(), e))?;
                serde_json::to_value(&parsed).map_err(|_| {
                    DecodeError::XdrParse(xdr_type.to_string(), stellar_xdr::Error::Invalid)
                })?
            }
            _ => return Err(DecodeError::TypeUnknown),
        };

        Ok(json_val)
    }

    /// Auto-detects the XDR type variant by executing quick trial decodes against common structures.
    pub fn auto_decode(input: &str) -> Result<(String, Value), DecodeError> {
        let candidates = [
            "TransactionEnvelope",
            "TransactionResult",
            "TransactionMeta",
            "ScVal",
            "LedgerKey",
            "LedgerEntry",
        ];

        for &candidate in &candidates {
            if let Ok(json_val) = Self::decode_to_json(input, candidate) {
                return Ok((candidate.to_string(), json_val));
            }
        }

        Err(DecodeError::TypeUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_base64() {
        let res = XdrDecoder::decode_to_json("!!!invalid!!!", "ScVal");
        assert!(res.is_err());
    }

    #[test]
    fn test_valid_scval_integer_base64() {
        // ScVal for an i32 containing value 42
        // Base64 envelope representation: AAAABgAAAAoAAAAq
        let base64_payload = "AAAABgAAAAoAAAAq";
        let res = XdrDecoder::decode_to_json(base64_payload, "ScVal").unwrap();

        // Value 42 is represented as i32 under scVal enum
        assert_eq!(res["i32"], 42);
    }

    #[test]
    fn test_auto_decode_scval() {
        let base64_payload = "AAAABgAAAAoAAAAq";
        let (detected_type, json_val) = XdrDecoder::auto_decode(base64_payload).unwrap();
        assert_eq!(detected_type, "ScVal");
        assert_eq!(json_val["i32"], 42);
    }
}
