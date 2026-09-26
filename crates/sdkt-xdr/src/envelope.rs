//! Human-readable transaction envelope decoder and formatter.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    AccountId, DecoratedSignature, Hash, HostFunction, Limits, Memo, MuxedAccount, OperationBody,
    PublicKey, ReadXdr, ScAddress, ScVal, SorobanTransactionData, TransactionEnvelope, Uint256,
};

use crate::DecodeError;

/// StrKey representation of a `MuxedAccount`.
pub fn muxed_account_to_strkey(acc: &MuxedAccount) -> String {
    match acc {
        MuxedAccount::Ed25519(Uint256(bytes)) => {
            format!(
                "{}",
                stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(*bytes))
            )
        }
        MuxedAccount::MuxedEd25519(mux) => {
            format!(
                "{}",
                stellar_strkey::Strkey::MuxedAccountEd25519(stellar_strkey::ed25519::MuxedAccount {
                    id: mux.id,
                    ed25519: mux.ed25519.0,
                })
            )
        }
    }
}

/// StrKey representation of an `AccountId`.
pub fn account_id_to_strkey(acc: &AccountId) -> String {
    match &acc.0 {
        PublicKey::PublicKeyTypeEd25519(Uint256(bytes)) => {
            format!(
                "{}",
                stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(*bytes))
            )
        }
    }
}

/// StrKey representation of an `ScAddress`.
pub fn sc_address_to_strkey(addr: &ScAddress) -> String {
    match addr {
        ScAddress::Account(acc) => account_id_to_strkey(acc),
        ScAddress::Contract(stellar_xdr::ContractId(Hash(bytes))) => {
            format!(
                "{}",
                stellar_strkey::Strkey::Contract(stellar_strkey::Contract(*bytes))
            )
        }
        _ => format!("scaddress({:?})", addr),
    }
}

/// StrKey representation of a contract ID `Hash`.
pub fn contract_id_to_strkey(hash: &Hash) -> String {
    format!(
        "{}",
        stellar_strkey::Strkey::Contract(stellar_strkey::Contract(hash.0))
    )
}

/// StrKey representation of a 32-byte Ed25519 public key.
pub fn ed25519_pk_to_strkey(bytes: &[u8; 32]) -> String {
    format!(
        "{}",
        stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(*bytes))
    )
}

/// Convert `ScVal` to human-readable string representation (e.g. `u32:42`, `symbol:increment`, `address:G...`).
pub fn format_scval_human(val: &ScVal) -> String {
    match val {
        ScVal::Bool(b) => format!("bool:{b}"),
        ScVal::U32(n) => format!("u32:{n}"),
        ScVal::I32(n) => format!("i32:{n}"),
        ScVal::U64(n) => format!("u64:{n}"),
        ScVal::I64(n) => format!("i64:{n}"),
        ScVal::U128(p) => {
            let val = ((p.hi as u128) << 64) | (p.lo as u128);
            format!("u128:{val}")
        }
        ScVal::I128(p) => {
            let val = ((p.hi as i128) << 64) | (p.lo as i128);
            format!("i128:{val}")
        }
        ScVal::U256(p) => {
            let bytes = [
                p.hi_hi.to_be_bytes(),
                p.hi_lo.to_be_bytes(),
                p.lo_hi.to_be_bytes(),
                p.lo_lo.to_be_bytes(),
            ]
            .concat();
            format!("u256:0x{}", hex::encode(bytes))
        }
        ScVal::I256(p) => {
            let bytes = [
                p.hi_hi.to_be_bytes(),
                p.hi_lo.to_be_bytes(),
                p.lo_hi.to_be_bytes(),
                p.lo_lo.to_be_bytes(),
            ]
            .concat();
            format!("i256:0x{}", hex::encode(bytes))
        }
        ScVal::String(s) => format!("string:{}", s.to_utf8_string_lossy()),
        ScVal::Symbol(s) => format!("symbol:{}", s.to_utf8_string_lossy()),
        ScVal::Bytes(b) => format!("bytes:{}", hex::encode(b.as_slice())),
        ScVal::Address(addr) => format!("address:{}", sc_address_to_strkey(addr)),
        ScVal::Vec(v) => {
            if let Some(items) = v.as_ref() {
                let formatted: Vec<String> = items.iter().map(format_scval_human).collect();
                format!("[{}]", formatted.join(", "))
            } else {
                "[]".to_string()
            }
        }
        ScVal::Map(m) => {
            if let Some(entries) = m.as_ref() {
                let formatted: Vec<String> = entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{}: {}",
                            format_scval_human(&e.key),
                            format_scval_human(&e.val)
                        )
                    })
                    .collect();
                format!("{{{}}}", formatted.join(", "))
            } else {
                "{}".to_string()
            }
        }
        ScVal::Void => "void".to_string(),
        ScVal::Timepoint(t) => format!("timepoint:{}", t.0),
        ScVal::Duration(d) => format!("duration:{}", d.0),
        ScVal::LedgerKeyContractInstance => "ledger_key_contract_instance".to_string(),
        ScVal::LedgerKeyNonce(n) => format!("nonce:{}", n.nonce),
        ScVal::ContractInstance(_) => "contract_instance".to_string(),
        ScVal::Error(e) => format!("error:{:?}", e),
        _ => format!("scval({:?})", val),
    }
}

/// Summary of Soroban resources and footprint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SorobanDataSummary {
    pub read_only_footprint: usize,
    pub read_write_footprint: usize,
    pub instructions: u32,
    pub read_bytes: u32,
    pub write_bytes: u32,
    pub resource_fee: i64,
}

impl SorobanDataSummary {
    pub fn from_xdr(data: &SorobanTransactionData) -> Self {
        Self {
            read_only_footprint: data.resources.footprint.read_only.len(),
            read_write_footprint: data.resources.footprint.read_write.len(),
            instructions: data.resources.instructions,
            read_bytes: data.resources.disk_read_bytes,
            write_bytes: data.resources.write_bytes,
            resource_fee: data.resource_fee,
        }
    }
}

/// Summary of a single operation within a transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationSummary {
    pub index: usize,
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_balance: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bump_to: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extend_to: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_bytes_len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_hash: Option<String>,
}

/// Summary of a decorated signature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureSummary {
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
}

/// Full human-readable breakdown of a `TransactionEnvelope`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvelopeSummary {
    pub envelope_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    pub operations_count: usize,
    pub operations: Vec<OperationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soroban_data: Option<SorobanDataSummary>,
    pub signatures_count: usize,
    pub signatures: Vec<SignatureSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_tx: Option<Box<EnvelopeSummary>>,
}

impl EnvelopeSummary {
    /// Render as human-readable pretty text.
    pub fn to_pretty_string(&self) -> String {
        let mut out = String::new();

        if self.envelope_type == "fee_bump" {
            out.push_str("Fee Bump Transaction Envelope:\n");
            if let Some(ref source) = self.source_account {
                out.push_str(&format!("  Fee Source: {}\n", source));
            }
            if let Some(fee) = self.fee {
                out.push_str(&format!("  Fee: {} stroops\n", fee));
            }
            out.push_str(&format_signatures_pretty(&self.signatures));
            if let Some(ref inner) = self.inner_tx {
                out.push_str("\nInner ");
                out.push_str(&inner.to_pretty_string());
            }
            return out;
        }

        out.push_str("Transaction Envelope:\n");
        if let Some(ref source) = self.source_account {
            out.push_str(&format!("  Source: {}\n", source));
        }
        if let Some(seq) = self.sequence {
            out.push_str(&format!("  Sequence: {}\n", seq));
        }
        if let Some(fee) = self.fee {
            out.push_str(&format!("  Fee: {} stroops\n", fee));
        }
        if let Some(ref memo) = self.memo {
            if memo != "None" {
                out.push_str(&format!("  Memo: {}\n", memo));
            }
        }

        out.push_str(&format!("  Operations ({}):\n", self.operations_count));
        for op in &self.operations {
            out.push_str(&format!("    [{}] {}\n", op.index, op.type_name));
            if let Some(ref contract) = op.contract_id {
                out.push_str(&format!("        Contract: {}\n", contract));
            }
            if let Some(ref func) = op.function_name {
                out.push_str(&format!("        Function: {}\n", func));
            }
            if let Some(ref args) = op.args {
                out.push_str(&format!("        Args: {}\n", args.join(", ")));
            }
            if let Some(ref dest) = op.destination {
                out.push_str(&format!("        Destination: {}\n", dest));
            }
            if let Some(amt) = op.amount {
                out.push_str(&format!("        Amount: {} stroops\n", amt));
            }
            if let Some(bal) = op.starting_balance {
                out.push_str(&format!("        Starting Balance: {} stroops\n", bal));
            }
            if let Some(seq) = op.bump_to {
                out.push_str(&format!("        Bump To: {}\n", seq));
            }
            if let Some(ext) = op.extend_to {
                out.push_str(&format!("        Extend To: {}\n", ext));
            }
            if let Some(size) = op.wasm_bytes_len {
                out.push_str(&format!("        Wasm Size: {} bytes\n", size));
            }
            if let Some(ref hash) = op.wasm_hash {
                out.push_str(&format!("        Wasm Hash: {}\n", hash));
            }
            if let Some(ref src) = op.source_account {
                out.push_str(&format!("        Source: {}\n", src));
            }
        }

        if let Some(ref soroban) = self.soroban_data {
            out.push_str("  Soroban Data:\n");
            out.push_str(&format!(
                "    Footprint: {} read-only, {} read-write\n",
                soroban.read_only_footprint, soroban.read_write_footprint
            ));
            out.push_str(&format!("    Instructions: {}\n", soroban.instructions));
        }

        out.push_str(&format_signatures_pretty(&self.signatures));

        out
    }
}

fn format_signatures_pretty(signatures: &[SignatureSummary]) -> String {
    let mut out = String::new();
    let count = signatures.len();
    if count == 0 {
        out.push_str("  Signatures: 0\n");
    } else if count == 1 {
        let sig = &signatures[0];
        if let Some(ref signer) = sig.signer {
            out.push_str(&format!("  Signatures: 1 (ed25519, {})\n", signer));
        } else {
            out.push_str(&format!("  Signatures: 1 (hint: {})\n", sig.hint));
        }
    } else {
        out.push_str(&format!("  Signatures: {}\n", count));
        for (i, sig) in signatures.iter().enumerate() {
            if let Some(ref signer) = sig.signer {
                out.push_str(&format!("    [{}] ed25519, {}\n", i, signer));
            } else {
                out.push_str(&format!("    [{}] hint: {}\n", i, sig.hint));
            }
        }
    }
    out
}

/// Decode a base64 XDR string representation of a `TransactionEnvelope`.
pub fn decode_envelope(b64: &str) -> Result<EnvelopeSummary, DecodeError> {
    let mut trimmed = b64.trim();
    if let Some(rest) = trimmed.strip_prefix("Transaction Envelope (Base64):") {
        trimmed = rest.trim();
    }
    if trimmed.is_empty() {
        return Err(DecodeError::EmptyPayload);
    }
    let raw = STANDARD
        .decode(trimmed)
        .map_err(DecodeError::Base64)?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = stellar_xdr::Limited::new(&mut cursor, Limits::none());
    let env = TransactionEnvelope::read_xdr(&mut limited)
        .map_err(|e| DecodeError::XdrParse("TransactionEnvelope".into(), e))?;

    decode_parsed_envelope(&env)
}

/// Decode an already-parsed `TransactionEnvelope` into an `EnvelopeSummary`.
pub fn decode_parsed_envelope(env: &TransactionEnvelope) -> Result<EnvelopeSummary, DecodeError> {
    match env {
        TransactionEnvelope::Tx(v1) => decode_v1_envelope(v1),
        TransactionEnvelope::TxV0(v0) => decode_v0_envelope(v0),
        TransactionEnvelope::TxFeeBump(fb) => decode_fee_bump_envelope(fb),
    }
}

fn extract_muxed_account_bytes(acc: &MuxedAccount) -> Option<[u8; 32]> {
    match acc {
        MuxedAccount::Ed25519(Uint256(b)) => Some(*b),
        MuxedAccount::MuxedEd25519(m) => Some(m.ed25519.0),
    }
}

fn decode_v1_envelope(
    v1: &stellar_xdr::TransactionV1Envelope,
) -> Result<EnvelopeSummary, DecodeError> {
    let tx = &v1.tx;
    let source_account = muxed_account_to_strkey(&tx.source_account);
    let mut candidate_keys = Vec::new();
    if let Some(bytes) = extract_muxed_account_bytes(&tx.source_account) {
        candidate_keys.push(bytes);
    }

    let memo_str = match &tx.memo {
        Memo::None => None,
        Memo::Text(t) => Some(format!("Text(\"{}\")", String::from_utf8_lossy(t.as_slice()))),
        Memo::Id(id) => Some(format!("Id({})", id)),
        Memo::Hash(h) => Some(format!("Hash({})", hex::encode(h.0))),
        Memo::Return(r) => Some(format!("Return({})", hex::encode(r.0))),
    };

    let mut operations = Vec::new();
    for (i, op) in tx.operations.iter().enumerate() {
        let op_summary = decode_operation(i, op, &mut candidate_keys)?;
        operations.push(op_summary);
    }

    let soroban_data = match &tx.ext {
        stellar_xdr::TransactionExt::V0 => None,
        stellar_xdr::TransactionExt::V1(data) => Some(SorobanDataSummary::from_xdr(data)),
    };

    let signatures = decode_signatures(&v1.signatures, &candidate_keys);

    Ok(EnvelopeSummary {
        envelope_type: "tx".to_string(),
        source_account: Some(source_account),
        sequence: Some(tx.seq_num.0),
        fee: Some(tx.fee),
        memo: memo_str,
        operations_count: operations.len(),
        operations,
        soroban_data,
        signatures_count: signatures.len(),
        signatures,
        inner_tx: None,
    })
}

fn decode_v0_envelope(
    v0: &stellar_xdr::TransactionV0Envelope,
) -> Result<EnvelopeSummary, DecodeError> {
    let tx = &v0.tx;
    let pk_bytes = tx.source_account_ed25519.0;
    let source_account = ed25519_pk_to_strkey(&pk_bytes);
    let mut candidate_keys = vec![pk_bytes];

    let memo_str = match &tx.memo {
        Memo::None => None,
        Memo::Text(t) => Some(format!("Text(\"{}\")", String::from_utf8_lossy(t.as_slice()))),
        Memo::Id(id) => Some(format!("Id({})", id)),
        Memo::Hash(h) => Some(format!("Hash({})", hex::encode(h.0))),
        Memo::Return(r) => Some(format!("Return({})", hex::encode(r.0))),
    };

    let mut operations = Vec::new();
    for (i, op) in tx.operations.iter().enumerate() {
        let op_summary = decode_operation(i, op, &mut candidate_keys)?;
        operations.push(op_summary);
    }

    let signatures = decode_signatures(&v0.signatures, &candidate_keys);

    Ok(EnvelopeSummary {
        envelope_type: "tx_v0".to_string(),
        source_account: Some(source_account),
        sequence: Some(tx.seq_num.0),
        fee: Some(tx.fee),
        memo: memo_str,
        operations_count: operations.len(),
        operations,
        soroban_data: None,
        signatures_count: signatures.len(),
        signatures,
        inner_tx: None,
    })
}

fn decode_fee_bump_envelope(
    fb: &stellar_xdr::FeeBumpTransactionEnvelope,
) -> Result<EnvelopeSummary, DecodeError> {
    let fee_source = muxed_account_to_strkey(&fb.tx.fee_source);
    let mut candidate_keys = Vec::new();
    if let Some(bytes) = extract_muxed_account_bytes(&fb.tx.fee_source) {
        candidate_keys.push(bytes);
    }

    let inner_summary = match &fb.tx.inner_tx {
        stellar_xdr::FeeBumpTransactionInnerTx::Tx(v1) => decode_v1_envelope(v1)?,
    };

    let signatures = decode_signatures(&fb.signatures, &candidate_keys);

    Ok(EnvelopeSummary {
        envelope_type: "fee_bump".to_string(),
        source_account: Some(fee_source),
        sequence: inner_summary.sequence,
        fee: Some(fb.tx.fee as u32),
        memo: None,
        operations_count: inner_summary.operations_count,
        operations: inner_summary.operations.clone(),
        soroban_data: inner_summary.soroban_data.clone(),
        signatures_count: signatures.len(),
        signatures,
        inner_tx: Some(Box::new(inner_summary)),
    })
}

fn decode_operation(
    index: usize,
    op: &stellar_xdr::Operation,
    candidate_keys: &mut Vec<[u8; 32]>,
) -> Result<OperationSummary, DecodeError> {
    let source_account = op.source_account.as_ref().map(|acc| {
        if let Some(b) = extract_muxed_account_bytes(acc) {
            candidate_keys.push(b);
        }
        muxed_account_to_strkey(acc)
    });

    let mut summary = OperationSummary {
        index,
        type_name: "".to_string(),
        contract_id: None,
        function_name: None,
        args: None,
        source_account,
        destination: None,
        amount: None,
        starting_balance: None,
        bump_to: None,
        extend_to: None,
        wasm_bytes_len: None,
        wasm_hash: None,
    };

    match &op.body {
        OperationBody::InvokeHostFunction(op_host) => {
            match &op_host.host_function {
                HostFunction::InvokeContract(args) => {
                    summary.type_name = "InvokeContract".to_string();
                    summary.contract_id = Some(sc_address_to_strkey(&args.contract_address));
                    summary.function_name = Some(args.function_name.to_utf8_string_lossy());
                    let arg_strs: Vec<String> = args.args.iter().map(format_scval_human).collect();
                    if !arg_strs.is_empty() {
                        summary.args = Some(arg_strs);
                    }
                }
                HostFunction::CreateContract(_) => {
                    summary.type_name = "CreateContract".to_string();
                }
                HostFunction::CreateContractV2(args) => {
                    summary.type_name = "CreateContractV2".to_string();
                    if let stellar_xdr::ContractExecutable::Wasm(h) = &args.executable {
                        summary.wasm_hash = Some(hex::encode(h.0));
                    }
                }
                HostFunction::UploadContractWasm(wasm) => {
                    summary.type_name = "UploadContractWasm".to_string();
                    summary.wasm_bytes_len = Some(wasm.len());
                }
            }
        }
        OperationBody::Payment(p) => {
            summary.type_name = "Payment".to_string();
            summary.destination = Some(muxed_account_to_strkey(&p.destination));
            summary.amount = Some(p.amount);
            if let Some(b) = extract_muxed_account_bytes(&p.destination) {
                candidate_keys.push(b);
            }
        }
        OperationBody::CreateAccount(ca) => {
            summary.type_name = "CreateAccount".to_string();
            summary.destination = Some(account_id_to_strkey(&ca.destination));
            summary.starting_balance = Some(ca.starting_balance);
        }
        OperationBody::BumpSequence(bs) => {
            summary.type_name = "BumpSequence".to_string();
            summary.bump_to = Some(bs.bump_to.0);
        }
        OperationBody::ExtendFootprintTtl(ef) => {
            summary.type_name = "ExtendFootprintTtl".to_string();
            summary.extend_to = Some(ef.extend_to);
        }
        OperationBody::RestoreFootprint(_) => {
            summary.type_name = "RestoreFootprint".to_string();
        }
        other => {
            summary.type_name = format!("{:?}", other);
        }
    }

    Ok(summary)
}

fn decode_signatures(
    sigs: &[DecoratedSignature],
    candidate_keys: &[[u8; 32]],
) -> Vec<SignatureSummary> {
    sigs.iter()
        .map(|sig| {
            let hint_hex = hex::encode(sig.hint.0);
            let mut signer = None;
            for key in candidate_keys {
                if key[28..32] == sig.hint.0 {
                    signer = Some(ed25519_pk_to_strkey(key));
                    break;
                }
            }
            SignatureSummary {
                hint: hint_hex,
                signer,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{build_invoke_transaction, InvokeTransactionParams};
    use crate::sign::{sign_envelope_with, Ed25519Signer, Network, SigningOptions};
    use stellar_xdr::{
        ContractDataDurability, ContractId, FeeBumpTransaction, FeeBumpTransactionEnvelope,
        FeeBumpTransactionInnerTx, LedgerFootprint, LedgerKey, LedgerKeyContractData, Limited,
        MuxedAccount, ScAddress, ScSymbol, ScVal, SorobanResources, SorobanTransactionData,
        SorobanTransactionDataExt, TransactionEnvelope, TransactionExt, Uint256, VecM, WriteXdr,
    };

    const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

    #[test]
    fn test_decode_single_op_invoke() {
        let params = InvokeTransactionParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 43,
            fee: 250,
            contract_id: TEST_CONTRACT.to_string(),
            function: "increment".to_string(),
            args: vec!["AAAAAQAAAAoAAAAA".to_string()], // ScVal::I32(1) encoded as base64
        };
        let b64 = build_invoke_transaction(&params).unwrap();

        let summary = decode_envelope(&b64).unwrap();
        assert_eq!(summary.envelope_type, "tx");
        assert_eq!(summary.source_account.as_deref(), Some(TEST_SOURCE));
        assert_eq!(summary.sequence, Some(43));
        assert_eq!(summary.fee, Some(250));
        assert_eq!(summary.operations_count, 1);
        let op = &summary.operations[0];
        assert_eq!(op.type_name, "InvokeContract");
        assert_eq!(op.contract_id.as_deref(), Some(TEST_CONTRACT));
        assert_eq!(op.function_name.as_deref(), Some("increment"));

        let pretty = summary.to_pretty_string();
        assert!(pretty.contains("Transaction Envelope:"));
        assert!(pretty.contains(&format!("Source: {}", TEST_SOURCE)));
        assert!(pretty.contains("Sequence: 43"));
        assert!(pretty.contains("Fee: 250 stroops"));
        assert!(pretty.contains("Operations (1):"));
        assert!(pretty.contains("[0] InvokeContract"));
        assert!(pretty.contains(&format!("Contract: {}", TEST_CONTRACT)));
        assert!(pretty.contains("Function: increment"));
    }

    #[test]
    fn test_decode_envelope_with_soroban_data_and_signatures() {
        let params = InvokeTransactionParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 100,
            fee: 500,
            contract_id: TEST_CONTRACT.to_string(),
            function: "test_func".to_string(),
            args: vec![],
        };
        let b64_unsigned = build_invoke_transaction(&params).unwrap();

        // Decode unsigned envelope to get struct, then add SorobanData and sign
        let raw = STANDARD.decode(&b64_unsigned).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let limited = Limits::none();
        let mut env = TransactionEnvelope::read_xdr(&mut Limited::new(&mut cursor, limited)).unwrap();

        if let TransactionEnvelope::Tx(ref mut v1) = env {
            v1.tx.ext = TransactionExt::V1(SorobanTransactionData {
                ext: SorobanTransactionDataExt::V0,
                resources: SorobanResources {
                    footprint: LedgerFootprint {
                        read_only: vec![
                            LedgerKey::ContractData(LedgerKeyContractData {
                                contract: ScAddress::Contract(ContractId(Hash([1u8; 32]))),
                                key: ScVal::Symbol(ScSymbol("a".try_into().unwrap())),
                                durability: ContractDataDurability::Persistent,
                            }),
                            LedgerKey::ContractData(LedgerKeyContractData {
                                contract: ScAddress::Contract(ContractId(Hash([2u8; 32]))),
                                key: ScVal::Symbol(ScSymbol("b".try_into().unwrap())),
                                durability: ContractDataDurability::Persistent,
                            }),
                        ]
                        .try_into()
                        .unwrap(),
                        read_write: vec![LedgerKey::ContractData(LedgerKeyContractData {
                            contract: ScAddress::Contract(ContractId(Hash([3u8; 32]))),
                            key: ScVal::Symbol(ScSymbol("c".try_into().unwrap())),
                            durability: ContractDataDurability::Persistent,
                        })]
                        .try_into()
                        .unwrap(),
                    },
                    instructions: 5000,
                    disk_read_bytes: 100,
                    write_bytes: 200,
                },
                resource_fee: 1000,
            });
        }

        let signer = Ed25519Signer::from_seed(&[7u8; 32]);
        let signed_env = sign_envelope_with(
            env,
            &signer,
            &SigningOptions {
                network: Network::Testnet,
            },
        )
        .unwrap();

        let mut buf = Vec::new();
        let limited = Limits::none();
        signed_env.write_xdr(&mut Limited::new(&mut buf, limited)).unwrap();
        let signed_b64 = STANDARD.encode(&buf);

        let summary = decode_envelope(&signed_b64).unwrap();
        assert!(summary.soroban_data.is_some());
        let soroban = summary.soroban_data.as_ref().unwrap();
        assert_eq!(soroban.read_only_footprint, 2);
        assert_eq!(soroban.read_write_footprint, 1);
        assert_eq!(soroban.instructions, 5000);
        assert_eq!(summary.signatures_count, 1);

        let pretty = summary.to_pretty_string();
        assert!(pretty.contains("Footprint: 2 read-only, 1 read-write"));
        assert!(pretty.contains("Instructions: 5000"));
        assert!(pretty.contains("Signatures: 1"));
    }

    #[test]
    fn test_decode_fee_bump_envelope() {
        let params = InvokeTransactionParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 10,
            fee: 100,
            contract_id: TEST_CONTRACT.to_string(),
            function: "ping".to_string(),
            args: vec![],
        };
        let inner_b64 = build_invoke_transaction(&params).unwrap();
        let raw = STANDARD.decode(&inner_b64).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let limited = Limits::none();
        let inner_env = TransactionEnvelope::read_xdr(&mut Limited::new(&mut cursor, limited)).unwrap();

        let inner_v1 = match inner_env {
            TransactionEnvelope::Tx(v1) => v1,
            _ => panic!("Expected Tx v1"),
        };

        let fb_env = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: FeeBumpTransaction {
                fee_source: MuxedAccount::Ed25519(Uint256([9u8; 32])),
                fee: 1000,
                inner_tx: FeeBumpTransactionInnerTx::Tx(inner_v1),
                ext: stellar_xdr::FeeBumpTransactionExt::V0,
            },
            signatures: VecM::default(),
        });

        let summary = decode_parsed_envelope(&fb_env).unwrap();
        assert_eq!(summary.envelope_type, "fee_bump");
        assert_eq!(summary.fee, Some(1000));
        assert!(summary.inner_tx.is_some());

        let pretty = summary.to_pretty_string();
        assert!(pretty.contains("Fee Bump Transaction Envelope:"));
        assert!(pretty.contains("Fee: 1000 stroops"));
        assert!(pretty.contains("Inner Transaction Envelope:"));
    }

    #[test]
    fn test_decode_invalid_envelope_fails() {
        assert!(decode_envelope("invalid base64!!!").is_err());
        assert!(decode_envelope("").is_err());
        assert!(decode_envelope("AAAA").is_err()); // valid base64 but invalid XDR
    }
}
