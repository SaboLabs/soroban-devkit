//! Human-readable transaction envelope viewer (`sdkt tx decode`).
//!
//! Decodes a base64 `TransactionEnvelope` offline and renders source, fee,
//! sequence, operations (including InvokeContract details), Soroban footprint
//! summary, and attached signatures — without network calls.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use stellar_strkey::Strkey;
use stellar_xdr::{
    HostFunction, Limited, Limits, Memo, MuxedAccount, Operation, OperationBody, PublicKey,
    ReadXdr, ScAddress, ScVal, Transaction, TransactionEnvelope, TransactionExt, TransactionV0,
};

/// Structured breakdown of a transaction envelope for pretty / JSON output.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvelopeView {
    /// Envelope variant: `"tx"`, `"txV0"`, or `"feeBump"`.
    pub r#type: String,
    /// Fee-bump outer source (only for fee-bump envelopes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_source: Option<String>,
    /// Fee-bump outer fee in stroops (only for fee-bump envelopes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_bump_fee: Option<i64>,
    /// Source account (G… strkey) of the (inner) transaction.
    pub source: String,
    /// Sequence number.
    pub sequence: i64,
    /// Base fee in stroops.
    pub fee: u32,
    /// Human-readable memo summary.
    pub memo: String,
    /// Decoded operations.
    pub operations: Vec<OperationView>,
    /// Soroban resource / footprint summary when `TransactionExt::V1` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soroban: Option<SorobanView>,
    /// Attached signatures.
    pub signatures: Vec<SignatureView>,
}

/// One operation inside the envelope.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub index: usize,
    /// Short type name (`InvokeContract`, `UploadContractWasm`, …).
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Extra detail for non-invoke ops (e.g. wasm byte length).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Soroban footprint / resource summary.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SorobanView {
    pub read_only: u32,
    pub read_write: u32,
    pub instructions: u32,
    pub disk_read_bytes: u32,
    pub write_bytes: u32,
    pub resource_fee: i64,
}

/// One attached signature.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignatureView {
    /// Signature scheme (currently always `"ed25519"`).
    pub r#type: String,
    /// Last 4 bytes of the signer public key, hex-encoded.
    pub hint: String,
    /// Full G… strkey when the hint can be matched to a known key (rare offline).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
}

/// Errors from envelope viewing / decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeViewError {
    Empty,
    InvalidBase64(String),
    MalformedEnvelope(String),
}

impl std::fmt::Display for EnvelopeViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvelopeViewError::Empty => write!(f, "transaction envelope is empty"),
            EnvelopeViewError::InvalidBase64(e) => write!(f, "invalid base64: {e}"),
            EnvelopeViewError::MalformedEnvelope(e) => write!(f, "malformed envelope: {e}"),
        }
    }
}

impl std::error::Error for EnvelopeViewError {}

/// Decode a base64 `TransactionEnvelope` into a structured [`EnvelopeView`].
///
/// Pure / offline — performs no network I/O.
pub fn view_envelope_base64(b64: &str) -> Result<EnvelopeView, EnvelopeViewError> {
    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return Err(EnvelopeViewError::Empty);
    }
    let raw = STANDARD
        .decode(trimmed)
        .map_err(|e| EnvelopeViewError::InvalidBase64(e.to_string()))?;
    if raw.is_empty() {
        return Err(EnvelopeViewError::Empty);
    }
    let mut cursor = std::io::Cursor::new(&raw);
    let mut limited = Limited::new(&mut cursor, Limits::none());
    let envelope = TransactionEnvelope::read_xdr(&mut limited)
        .map_err(|e| EnvelopeViewError::MalformedEnvelope(e.to_string()))?;
    Ok(view_envelope(&envelope))
}

/// Build an [`EnvelopeView`] from an already-parsed envelope.
pub fn view_envelope(envelope: &TransactionEnvelope) -> EnvelopeView {
    match envelope {
        TransactionEnvelope::Tx(env) => {
            let mut view = view_transaction(&env.tx, "tx");
            view.signatures = view_signatures(&env.signatures);
            view
        }
        TransactionEnvelope::TxV0(env) => {
            let mut view = view_transaction_v0(&env.tx, "txV0");
            view.signatures = view_signatures(&env.signatures);
            view
        }
        TransactionEnvelope::TxFeeBump(fb) => {
            let fee_source = format_muxed(&fb.tx.fee_source);
            let fee_bump_fee = fb.tx.fee;
            let stellar_xdr::FeeBumpTransactionInnerTx::Tx(inner) = &fb.tx.inner_tx;
            let mut view = view_transaction(&inner.tx, "feeBump");
            view.fee_source = Some(fee_source);
            view.fee_bump_fee = Some(fee_bump_fee);
            // Prefer outer signatures; fall back to inner if outer is empty.
            let outer = view_signatures(&fb.signatures);
            if outer.is_empty() {
                view.signatures = view_signatures(&inner.signatures);
            } else {
                view.signatures = outer;
            }
            view
        }
    }
}

/// Pretty-print an [`EnvelopeView`] to match the CLI acceptance example.
pub fn format_envelope_pretty(view: &EnvelopeView) -> String {
    let mut out = String::new();
    out.push_str("Transaction Envelope:\n");
    if view.r#type == "feeBump" {
        if let Some(fs) = &view.fee_source {
            out.push_str(&format!("  Fee Source: {}\n", fs));
        }
        if let Some(ff) = view.fee_bump_fee {
            out.push_str(&format!("  Fee-Bump Fee: {} stroops\n", ff));
        }
    }
    out.push_str(&format!("  Source:     {}\n", view.source));
    out.push_str(&format!("  Sequence:   {}\n", view.sequence));
    out.push_str(&format!("  Fee:        {} stroops\n", view.fee));
    if view.memo != "none" {
        out.push_str(&format!("  Memo:       {}\n", view.memo));
    }
    out.push_str(&format!("  Operations ({}):\n", view.operations.len()));
    if view.operations.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for op in &view.operations {
            out.push_str(&format!("    [{}] {}\n", op.index, op.r#type));
            if let Some(c) = &op.contract {
                out.push_str(&format!("        Contract:  {}\n", c));
            }
            if let Some(f) = &op.function {
                out.push_str(&format!("        Function:  {}\n", f));
            }
            if let Some(args) = &op.args {
                if args.is_empty() {
                    out.push_str("        Args:      (none)\n");
                } else {
                    out.push_str(&format!("        Args:      {}\n", args.join(", ")));
                }
            }
            if let Some(d) = &op.detail {
                out.push_str(&format!("        Detail:    {}\n", d));
            }
        }
    }
    if let Some(s) = &view.soroban {
        out.push_str("  Soroban Data:\n");
        out.push_str(&format!(
            "    Footprint:  {} read-only, {} read-write\n",
            s.read_only, s.read_write
        ));
        out.push_str(&format!("    Instructions: {}\n", s.instructions));
        if s.disk_read_bytes > 0 || s.write_bytes > 0 {
            out.push_str(&format!(
                "    I/O:         {} read / {} write bytes\n",
                s.disk_read_bytes, s.write_bytes
            ));
        }
        if s.resource_fee != 0 {
            out.push_str(&format!("    Resource Fee: {} stroops\n", s.resource_fee));
        }
    }
    if view.signatures.is_empty() {
        out.push_str("  Signatures: 0\n");
    } else {
        let parts: Vec<String> = view
            .signatures
            .iter()
            .map(|s| {
                if let Some(signer) = &s.signer {
                    format!("{}, {}", s.r#type, signer)
                } else {
                    format!("{}, hint={}", s.r#type, s.hint)
                }
            })
            .collect();
        out.push_str(&format!(
            "  Signatures: {} ({})\n",
            view.signatures.len(),
            parts.join("; ")
        ));
    }
    out
}

fn view_transaction(tx: &Transaction, kind: &str) -> EnvelopeView {
    EnvelopeView {
        r#type: kind.into(),
        fee_source: None,
        fee_bump_fee: None,
        source: format_muxed(&tx.source_account),
        sequence: tx.seq_num.0,
        fee: tx.fee,
        memo: format_memo(&tx.memo),
        operations: tx
            .operations
            .iter()
            .enumerate()
            .map(|(i, op)| view_operation(i, op))
            .collect(),
        soroban: view_soroban_ext(&tx.ext),
        signatures: Vec::new(),
    }
}

fn view_transaction_v0(tx: &TransactionV0, kind: &str) -> EnvelopeView {
    let source = format_ed25519(&tx.source_account_ed25519.0);
    EnvelopeView {
        r#type: kind.into(),
        fee_source: None,
        fee_bump_fee: None,
        source,
        sequence: tx.seq_num.0,
        fee: tx.fee,
        memo: format_memo(&tx.memo),
        operations: tx
            .operations
            .iter()
            .enumerate()
            .map(|(i, op)| view_operation(i, op))
            .collect(),
        soroban: None,
        signatures: Vec::new(),
    }
}

fn view_soroban_ext(ext: &TransactionExt) -> Option<SorobanView> {
    match ext {
        TransactionExt::V0 => None,
        TransactionExt::V1(data) => Some(SorobanView {
            read_only: data.resources.footprint.read_only.len() as u32,
            read_write: data.resources.footprint.read_write.len() as u32,
            instructions: data.resources.instructions,
            disk_read_bytes: data.resources.disk_read_bytes,
            write_bytes: data.resources.write_bytes,
            resource_fee: data.resource_fee,
        }),
    }
}

fn view_operation(index: usize, op: &Operation) -> OperationView {
    match &op.body {
        OperationBody::InvokeHostFunction(hf) => match &hf.host_function {
            HostFunction::InvokeContract(args) => OperationView {
                index,
                r#type: "InvokeContract".into(),
                contract: Some(format_sc_address(&args.contract_address)),
                function: Some(args.function_name.to_utf8_string_lossy()),
                args: Some(args.args.iter().map(format_scval).collect()),
                detail: None,
            },
            HostFunction::UploadContractWasm(bytes) => OperationView {
                index,
                r#type: "UploadContractWasm".into(),
                contract: None,
                function: None,
                args: None,
                detail: Some(format!("{} wasm bytes", bytes.len())),
            },
            HostFunction::CreateContract(args) => OperationView {
                index,
                r#type: "CreateContract".into(),
                contract: None,
                function: None,
                args: None,
                detail: Some(format!("{:?}", args.executable)),
            },
            HostFunction::CreateContractV2(args) => OperationView {
                index,
                r#type: "CreateContractV2".into(),
                contract: None,
                function: None,
                args: None,
                detail: Some(format!(
                    "{:?}, {} constructor args",
                    args.executable,
                    args.constructor_args.len()
                )),
            },
            other => OperationView {
                index,
                r#type: format!("{:?}", other)
                    .split('(')
                    .next()
                    .unwrap_or("HostFunction")
                    .to_string(),
                contract: None,
                function: None,
                args: None,
                detail: None,
            },
        },
        OperationBody::ExtendFootprintTtl(op) => OperationView {
            index,
            r#type: "ExtendFootprintTtl".into(),
            contract: None,
            function: None,
            args: None,
            detail: Some(format!("extend_to={}", op.extend_to)),
        },
        other => {
            let name = format!("{:?}", other);
            let short = name.split('(').next().unwrap_or("Operation").to_string();
            OperationView {
                index,
                r#type: short,
                contract: None,
                function: None,
                args: None,
                detail: None,
            }
        }
    }
}

fn view_signatures(sigs: &[stellar_xdr::DecoratedSignature]) -> Vec<SignatureView> {
    sigs.iter()
        .map(|s| SignatureView {
            r#type: "ed25519".into(),
            hint: hex::encode(s.hint.0),
            signer: None,
        })
        .collect()
}

fn format_muxed(account: &MuxedAccount) -> String {
    match account {
        MuxedAccount::Ed25519(u) => format_ed25519(&u.0),
        MuxedAccount::MuxedEd25519(m) => {
            // Prefer the underlying ed25519 key; include mux id when present.
            format!("{}#{}", format_ed25519(&m.ed25519.0), m.id)
        }
    }
}

fn format_ed25519(raw: &[u8; 32]) -> String {
    Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(*raw)).to_string()
}

fn format_sc_address(addr: &ScAddress) -> String {
    match addr {
        ScAddress::Account(account_id) => match &account_id.0 {
            PublicKey::PublicKeyTypeEd25519(u) => format_ed25519(&u.0),
        },
        ScAddress::Contract(cid) => {
            Strkey::Contract(stellar_strkey::Contract(cid.0 .0)).to_string()
        }
        other => format!("{:?}", other),
    }
}

fn format_memo(memo: &Memo) -> String {
    match memo {
        Memo::None => "none".into(),
        Memo::Text(t) => format!("text:{}", String::from_utf8_lossy(t.as_slice())),
        Memo::Id(id) => format!("id:{id}"),
        Memo::Hash(h) => format!("hash:{}", hex::encode(h.0)),
        Memo::Return(h) => format!("return:{}", hex::encode(h.0)),
    }
}

/// Format an `ScVal` as a compact `TYPE:VALUE` string (inverse of CLI typed args).
pub fn format_scval(val: &ScVal) -> String {
    match val {
        ScVal::Bool(b) => format!("bool:{b}"),
        ScVal::U32(n) => format!("u32:{n}"),
        ScVal::I32(n) => format!("i32:{n}"),
        ScVal::U64(n) => format!("u64:{n}"),
        ScVal::I64(n) => format!("i64:{n}"),
        ScVal::U128(p) => format!("u128:hi={},lo={}", p.hi, p.lo),
        ScVal::I128(p) => format!("i128:hi={},lo={}", p.hi, p.lo),
        ScVal::U256(_) => "u256:...".into(),
        ScVal::I256(_) => "i256:...".into(),
        ScVal::String(s) => format!("string:{}", s.to_utf8_string_lossy()),
        ScVal::Symbol(s) => format!("symbol:{}", s.to_utf8_string_lossy()),
        ScVal::Bytes(b) => format!("bytes:{}", hex::encode(b.as_slice())),
        ScVal::Address(a) => format!("address:{}", format_sc_address(a)),
        ScVal::Vec(Some(v)) => {
            let inner: Vec<String> = v.iter().map(format_scval).collect();
            format!("vec:[{}]", inner.join(", "))
        }
        ScVal::Vec(None) => "vec:[]".into(),
        ScVal::Map(Some(m)) => {
            let inner: Vec<String> = m
                .iter()
                .map(|e| format!("{}={}", format_scval(&e.key), format_scval(&e.val)))
                .collect();
            format!("map:{{{}}}", inner.join(", "))
        }
        ScVal::Map(None) => "map:{}".into(),
        ScVal::Void => "void".into(),
        other => format!("scval:{:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{build_invoke_transaction, InvokeTransactionParams};
    use stellar_xdr::{
        Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, LedgerFootprint,
        LedgerKey, LedgerKeyContractCode, Memo, MuxedAccount, Operation, OperationBody,
        Preconditions, ScAddress, ScSymbol, SequenceNumber, SorobanResources,
        SorobanTransactionData, SorobanTransactionDataExt, Transaction, TransactionExt,
        TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };

    const SRC: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    // Contract id for Hash([2u8; 32])
    fn contract_strkey() -> String {
        Strkey::Contract(stellar_strkey::Contract([2u8; 32])).to_string()
    }

    fn dummy_source() -> MuxedAccount {
        MuxedAccount::Ed25519(Uint256([1u8; 32]))
    }

    fn make_invoke_op(function: &str, args: Vec<ScVal>) -> Operation {
        let contract_id = stellar_xdr::ContractId(Hash([2u8; 32]));
        Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(contract_id),
                    function_name: ScSymbol(function.as_bytes().try_into().unwrap()),
                    args: VecM::try_from(args).unwrap(),
                }),
                auth: VecM::default(),
            }),
        }
    }

    fn make_envelope(
        fee: u32,
        seq: i64,
        ops: Vec<Operation>,
        ext: TransactionExt,
    ) -> TransactionEnvelope {
        let tx = Transaction {
            source_account: dummy_source(),
            fee,
            seq_num: SequenceNumber(seq),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: ops.try_into().unwrap(),
            ext,
        };
        TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        })
    }

    fn envelope_b64(env: &TransactionEnvelope) -> String {
        let mut buf = Vec::new();
        let mut l = stellar_xdr::Limited::new(&mut buf, Limits::none());
        env.write_xdr(&mut l).unwrap();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf)
    }

    #[test]
    fn views_single_invoke_fields() {
        let env = make_envelope(
            250,
            43,
            vec![make_invoke_op("increment", vec![ScVal::U32(42)])],
            TransactionExt::V0,
        );
        let view = view_envelope(&env);
        assert_eq!(view.sequence, 43);
        assert_eq!(view.fee, 250);
        assert_eq!(view.operations.len(), 1);
        assert_eq!(view.operations[0].r#type, "InvokeContract");
        assert_eq!(view.operations[0].function.as_deref(), Some("increment"));
        assert_eq!(
            view.operations[0].args.as_ref().map(|a| a.as_slice()),
            Some(["u32:42"].as_slice())
        );
        assert_eq!(
            view.operations[0].contract.as_deref(),
            Some(contract_strkey().as_str())
        );
        assert!(view.source.starts_with('G'));
        assert!(view.signatures.is_empty());

        let pretty = format_envelope_pretty(&view);
        assert!(pretty.contains("Sequence:   43"));
        assert!(pretty.contains("Fee:        250 stroops"));
        assert!(pretty.contains("InvokeContract"));
        assert!(pretty.contains("increment"));
        assert!(pretty.contains("u32:42"));
    }

    #[test]
    fn views_multi_op_and_soroban_footprint() {
        let footprint = LedgerFootprint {
            read_only: VecM::try_from(vec![
                LedgerKey::ContractCode(LedgerKeyContractCode {
                    hash: Hash([9u8; 32]),
                }),
                LedgerKey::ContractCode(LedgerKeyContractCode {
                    hash: Hash([8u8; 32]),
                }),
            ])
            .unwrap(),
            read_write: VecM::try_from(vec![LedgerKey::ContractCode(LedgerKeyContractCode {
                hash: Hash([7u8; 32]),
            })])
            .unwrap(),
        };
        let soroban = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint,
                instructions: 5000,
                disk_read_bytes: 0,
                write_bytes: 0,
            },
            resource_fee: 100,
        };
        let env = make_envelope(
            300,
            99,
            vec![
                make_invoke_op("a", vec![]),
                make_invoke_op("b", vec![ScVal::Bool(true)]),
            ],
            TransactionExt::V1(soroban),
        );
        let view = view_envelope(&env);
        assert_eq!(view.operations.len(), 2);
        let s = view.soroban.expect("soroban data");
        assert_eq!(s.read_only, 2);
        assert_eq!(s.read_write, 1);
        assert_eq!(s.instructions, 5000);

        let pretty = format_envelope_pretty(&view);
        assert!(pretty.contains("2 read-only, 1 read-write"));
        assert!(pretty.contains("Instructions: 5000"));
    }

    #[test]
    fn views_signatures_count_and_hint() {
        let mut env = make_envelope(
            200,
            10,
            vec![make_invoke_op("hello", vec![])],
            TransactionExt::V0,
        );
        if let TransactionEnvelope::Tx(ref mut e) = env {
            e.signatures = VecM::try_from(vec![stellar_xdr::DecoratedSignature {
                hint: stellar_xdr::SignatureHint([0xde, 0xad, 0xbe, 0xef]),
                signature: stellar_xdr::Signature(vec![0u8; 64].try_into().unwrap()),
            }])
            .unwrap();
        }
        let view = view_envelope(&env);
        assert_eq!(view.signatures.len(), 1);
        assert_eq!(view.signatures[0].hint, "deadbeef");
        let pretty = format_envelope_pretty(&view);
        assert!(pretty.contains("Signatures: 1"));
        assert!(pretty.contains("deadbeef"));
    }

    #[test]
    fn json_is_stable_structured() {
        let env = make_envelope(
            250,
            43,
            vec![make_invoke_op("increment", vec![ScVal::U32(42)])],
            TransactionExt::V0,
        );
        let view = view_envelope(&env);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["type"], "tx");
        assert_eq!(json["sequence"], 43);
        assert_eq!(json["fee"], 250);
        assert_eq!(json["operations"][0]["type"], "InvokeContract");
        assert_eq!(json["operations"][0]["function"], "increment");
        assert_eq!(json["operations"][0]["args"][0], "u32:42");
        // Must NOT be raw XDR serde nesting
        assert!(json.get("Tx").is_none());
    }

    #[test]
    fn invalid_base64_errors_clearly() {
        let err = view_envelope_base64("not!!!valid!!!base64").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("malformed") || msg.contains("invalid base64") || msg.contains("base64"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn empty_input_errors() {
        assert!(matches!(
            view_envelope_base64("   ").unwrap_err(),
            EnvelopeViewError::Empty
        ));
    }

    #[test]
    fn roundtrip_tx_build_values() {
        // Use a well-formed G-address (all-zero pubkey with valid checksum used elsewhere).
        let params = InvokeTransactionParams {
            source_account: SRC.to_string(),
            sequence: 43,
            fee: 250,
            contract_id: contract_strkey(),
            function: "increment".into(),
            args: vec![
                // ScVal::U32(42) as base64 — encode via WriteXdr
                {
                    let mut buf = Vec::new();
                    let mut l = stellar_xdr::Limited::new(&mut buf, Limits::none());
                    ScVal::U32(42).write_xdr(&mut l).unwrap();
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf)
                },
            ],
        };
        let b64 = build_invoke_transaction(&params).expect("build");
        let view = view_envelope_base64(&b64).expect("view");
        assert_eq!(view.sequence, 43);
        assert_eq!(view.fee, 250);
        assert_eq!(view.source, SRC);
        assert_eq!(view.operations.len(), 1);
        assert_eq!(view.operations[0].function.as_deref(), Some("increment"));
        assert_eq!(view.operations[0].contract.as_deref(), Some(contract_strkey().as_str()));
        assert_eq!(
            view.operations[0].args.as_ref().unwrap()[0],
            "u32:42"
        );
    }

    #[test]
    fn format_scval_primitives() {
        assert_eq!(format_scval(&ScVal::U32(7)), "u32:7");
        assert_eq!(format_scval(&ScVal::Bool(false)), "bool:false");
        assert_eq!(format_scval(&ScVal::Void), "void");
    }
}
