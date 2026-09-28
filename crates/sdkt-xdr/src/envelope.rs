//! Human-readable view of a `TransactionEnvelope`, for reviewing a transaction
//! before it is signed or submitted.
//!
//! [`decode_envelope`] turns base64 envelope XDR into an [`EnvelopeView`]: the
//! source, sequence, fee, memo, each operation (with contract calls and their
//! arguments rendered in the same `type:value` form `tx build --arg` accepts),
//! the Soroban resource footprint and the attached signatures. Everything is
//! computed offline from the envelope bytes alone.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use stellar_xdr::{
    DecoratedSignature, HostFunction, Limits, Memo, MuxedAccount, Operation, OperationBody,
    ReadXdr, ScVal, SorobanTransactionData, Transaction, TransactionEnvelope, TransactionExt,
    TransactionV0Ext,
};

use crate::DecodeError;

/// Nesting cap for untrusted envelopes. Matches the host's XDR depth limit, so
/// any envelope the network would accept still decodes, while a hostile one
/// cannot recurse deep enough to exhaust the stack.
const MAX_XDR_DEPTH: u32 = 500;

/// Structured, stable breakdown of a transaction envelope.
///
/// For a fee-bump envelope the top-level fields describe the inner
/// transaction and [`EnvelopeView::fee_bump`] carries the outer wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvelopeView {
    /// `tx_v0`, `tx` or `tx_fee_bump`.
    pub envelope_type: String,
    pub source: String,
    pub sequence: i64,
    /// Fee in stroops. `i64` so a fee-bump fee is never truncated.
    pub fee: i64,
    pub memo: Option<String>,
    pub operations: Vec<OperationView>,
    pub soroban: Option<SorobanView>,
    pub signatures: Vec<SignatureView>,
    pub fee_bump: Option<FeeBumpView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationView {
    /// Operation type, or the host function kind for `InvokeHostFunction`.
    pub kind: String,
    /// Per-operation source account, when it differs from the transaction's.
    pub source: Option<String>,
    pub contract: Option<String>,
    pub function: Option<String>,
    pub args: Vec<String>,
    /// Number of Soroban authorization entries attached to the operation.
    pub auth_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SorobanView {
    pub read_only: usize,
    pub read_write: usize,
    pub instructions: u32,
    pub disk_read_bytes: u32,
    pub write_bytes: u32,
    pub resource_fee: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignatureView {
    /// Last four bytes of the signer's public key, hex encoded.
    pub hint: String,
    /// Signer account, when the hint matches exactly one account in the envelope.
    pub signer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeeBumpView {
    pub fee_source: String,
    /// Total fee the fee source pays, in stroops.
    pub fee: i64,
    pub signatures: Vec<SignatureView>,
}

/// Decode a base64 `TransactionEnvelope` into an [`EnvelopeView`].
///
/// Whitespace in the input is ignored. Trailing bytes after the envelope are
/// rejected, so this accepts exactly the payloads `tx validate` can parse.
pub fn decode_envelope(b64: &str) -> Result<EnvelopeView, DecodeError> {
    let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return Err(DecodeError::EmptyPayload);
    }
    let raw = STANDARD.decode(compact)?;
    let limits = Limits {
        depth: MAX_XDR_DEPTH,
        len: raw.len(),
    };
    let envelope = TransactionEnvelope::from_xdr(&raw, limits)
        .map_err(|e| DecodeError::XdrParse("TransactionEnvelope".into(), e))?;
    Ok(view_envelope(&envelope))
}

/// Build the view for an already-parsed envelope.
pub fn view_envelope(envelope: &TransactionEnvelope) -> EnvelopeView {
    match envelope {
        TransactionEnvelope::TxV0(env) => {
            let tx = &env.tx;
            let source = MuxedAccount::Ed25519(tx.source_account_ed25519.clone());
            let keys = signer_keys(&source, &tx.operations, None);
            EnvelopeView {
                envelope_type: "tx_v0".into(),
                source: source.to_string(),
                sequence: tx.seq_num.0,
                fee: i64::from(tx.fee),
                memo: render_memo(&tx.memo),
                operations: tx.operations.iter().map(view_operation).collect(),
                soroban: match tx.ext {
                    TransactionV0Ext::V0 => None,
                },
                signatures: view_signatures(&env.signatures, &keys),
                fee_bump: None,
            }
        }
        TransactionEnvelope::Tx(env) => view_v1(&env.tx, &env.signatures, "tx"),
        TransactionEnvelope::TxFeeBump(env) => {
            let stellar_xdr::FeeBumpTransactionInnerTx::Tx(inner) = &env.tx.inner_tx;
            let mut view = view_v1(&inner.tx, &inner.signatures, "tx_fee_bump");
            let outer_keys = signer_keys(
                &inner.tx.source_account,
                &inner.tx.operations,
                Some(&env.tx.fee_source),
            );
            view.fee_bump = Some(FeeBumpView {
                fee_source: env.tx.fee_source.to_string(),
                fee: env.tx.fee,
                signatures: view_signatures(&env.signatures, &outer_keys),
            });
            view
        }
    }
}

fn view_v1(tx: &Transaction, signatures: &[DecoratedSignature], kind: &str) -> EnvelopeView {
    let keys = signer_keys(&tx.source_account, &tx.operations, None);
    EnvelopeView {
        envelope_type: kind.into(),
        source: tx.source_account.to_string(),
        sequence: tx.seq_num.0,
        fee: i64::from(tx.fee),
        memo: render_memo(&tx.memo),
        operations: tx.operations.iter().map(view_operation).collect(),
        soroban: match &tx.ext {
            TransactionExt::V0 => None,
            TransactionExt::V1(data) => Some(view_soroban(data)),
        },
        signatures: view_signatures(signatures, &keys),
        fee_bump: None,
    }
}

fn view_operation(op: &Operation) -> OperationView {
    let mut view = OperationView {
        kind: op.body.name().to_string(),
        source: op.source_account.as_ref().map(ToString::to_string),
        contract: None,
        function: None,
        args: Vec::new(),
        auth_entries: 0,
    };
    let OperationBody::InvokeHostFunction(invoke) = &op.body else {
        return view;
    };
    view.kind = invoke.host_function.name().to_string();
    view.auth_entries = invoke.auth.len();
    match &invoke.host_function {
        HostFunction::InvokeContract(call) => {
            view.contract = Some(call.contract_address.to_string());
            view.function = Some(call.function_name.0.to_utf8_string_lossy());
            view.args = call.args.iter().map(render_scval).collect();
        }
        HostFunction::CreateContractV2(create) => {
            view.args = create.constructor_args.iter().map(render_scval).collect();
        }
        HostFunction::CreateContract(_) | HostFunction::UploadContractWasm(_) => {}
    }
    view
}

fn view_soroban(data: &SorobanTransactionData) -> SorobanView {
    let resources = &data.resources;
    SorobanView {
        read_only: resources.footprint.read_only.len(),
        read_write: resources.footprint.read_write.len(),
        instructions: resources.instructions,
        disk_read_bytes: resources.disk_read_bytes,
        write_bytes: resources.write_bytes,
        resource_fee: data.resource_fee,
    }
}

/// Every account in the envelope that could have produced a signature: the
/// transaction source, per-operation sources and, for the outer fee-bump
/// signatures, the fee source.
fn signer_keys(
    source: &MuxedAccount,
    operations: &[Operation],
    fee_source: Option<&MuxedAccount>,
) -> Vec<[u8; 32]> {
    let mut keys = vec![ed25519_key(source)];
    keys.extend(
        operations
            .iter()
            .filter_map(|op| op.source_account.as_ref().map(ed25519_key)),
    );
    keys.extend(fee_source.map(ed25519_key));
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn ed25519_key(account: &MuxedAccount) -> [u8; 32] {
    match account {
        MuxedAccount::Ed25519(key) => key.0,
        MuxedAccount::MuxedEd25519(muxed) => muxed.ed25519.0,
    }
}

fn view_signatures(signatures: &[DecoratedSignature], keys: &[[u8; 32]]) -> Vec<SignatureView> {
    signatures
        .iter()
        .map(|sig| {
            let mut matches = keys.iter().filter(|key| key[28..] == sig.hint.0);
            let signer = match (matches.next(), matches.next()) {
                (Some(key), None) => Some(
                    stellar_strkey::ed25519::PublicKey(*key)
                        .to_string()
                        .as_str()
                        .to_owned(),
                ),
                _ => None,
            };
            SignatureView {
                hint: hex::encode(sig.hint.0),
                signer,
            }
        })
        .collect()
}

fn render_memo(memo: &Memo) -> Option<String> {
    match memo {
        Memo::None => None,
        Memo::Text(text) => Some(format!("text:{:?}", text.to_utf8_string_lossy())),
        Memo::Id(id) => Some(format!("id:{id}")),
        Memo::Hash(hash) => Some(format!("hash:{}", hex::encode(hash.0))),
        Memo::Return(hash) => Some(format!("return:{}", hex::encode(hash.0))),
    }
}

/// Render an `ScVal` in the `type:value` form `tx build --arg` accepts, so a
/// decoded argument reads the same way it was written. Strings are quoted so
/// commas and brackets inside them stay unambiguous in vec/map output.
pub fn render_scval(val: &ScVal) -> String {
    match val {
        ScVal::Bool(b) => format!("bool:{b}"),
        ScVal::Void => "void".into(),
        ScVal::U32(n) => format!("u32:{n}"),
        ScVal::I32(n) => format!("i32:{n}"),
        ScVal::U64(n) => format!("u64:{n}"),
        ScVal::I64(n) => format!("i64:{n}"),
        ScVal::Timepoint(t) => format!("timepoint:{}", t.0),
        ScVal::Duration(d) => format!("duration:{}", d.0),
        ScVal::U128(p) => format!("u128:{p}"),
        ScVal::I128(p) => format!("i128:{p}"),
        ScVal::U256(p) => format!("u256:{p}"),
        ScVal::I256(p) => format!("i256:{p}"),
        ScVal::String(s) => format!("string:{:?}", s.to_utf8_string_lossy()),
        ScVal::Symbol(s) => format!("symbol:{}", s.to_utf8_string_lossy()),
        ScVal::Bytes(b) => format!("bytes:{}", hex::encode(b.as_slice())),
        ScVal::Address(a) => format!("address:{a}"),
        ScVal::Vec(items) => {
            let items = items.as_ref().map(|v| v.as_slice()).unwrap_or_default();
            let rendered: Vec<String> = items.iter().map(render_scval).collect();
            format!("vec:[{}]", rendered.join(", "))
        }
        ScVal::Map(entries) => {
            let entries = entries.as_ref().map(|m| m.as_slice()).unwrap_or_default();
            let rendered: Vec<String> = entries
                .iter()
                .map(|e| format!("{} => {}", render_scval(&e.key), render_scval(&e.val)))
                .collect();
            format!("map:{{{}}}", rendered.join(", "))
        }
        other => other.name().to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_invoke_transaction, IntoScVal, InvokeTransactionParams};
    use stellar_xdr::{
        FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
        FeeBumpTransactionInnerTx, LedgerFootprint, SignatureHint, SorobanResources,
        SorobanTransactionDataExt, TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };

    const SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

    fn invoke_envelope(args: Vec<ScVal>) -> TransactionEnvelope {
        let args = args
            .iter()
            .map(|v| crate::scval_to_base64(v).unwrap())
            .collect();
        let b64 = build_invoke_transaction(&InvokeTransactionParams {
            source_account: SOURCE.into(),
            sequence: 43,
            fee: 250,
            contract_id: CONTRACT.into(),
            function: "increment".into(),
            args,
            memo: None,
        })
        .unwrap();
        TransactionEnvelope::from_xdr_base64(b64, Limits::none()).unwrap()
    }

    fn to_b64(env: &TransactionEnvelope) -> String {
        env.to_xdr_base64(Limits::none()).unwrap()
    }

    fn v1(env: TransactionEnvelope) -> TransactionV1Envelope {
        match env {
            TransactionEnvelope::Tx(v1) => v1,
            other => panic!("expected a v1 envelope, got {}", other.name()),
        }
    }

    #[test]
    fn renders_invoke_contract_envelope() {
        let env = invoke_envelope(vec![ScVal::U32(42)]);
        let view = decode_envelope(&to_b64(&env)).unwrap();

        assert_eq!(view.envelope_type, "tx");
        assert_eq!(view.source, SOURCE);
        assert_eq!(view.sequence, 43);
        assert_eq!(view.fee, 250);
        assert_eq!(view.memo, None);
        assert_eq!(view.operations.len(), 1);
        let op = &view.operations[0];
        assert_eq!(op.kind, "InvokeContract");
        assert_eq!(op.contract.as_deref(), Some(CONTRACT));
        assert_eq!(op.function.as_deref(), Some("increment"));
        assert_eq!(op.args, vec!["u32:42"]);
        assert!(view.soroban.is_none());
        assert!(view.signatures.is_empty());
        assert!(view.fee_bump.is_none());
    }

    #[test]
    fn renders_every_operation_in_a_multi_op_envelope() {
        let mut env = v1(invoke_envelope(vec![ScVal::Void]));
        let op_source = MuxedAccount::Ed25519(Uint256([3; 32]));
        let bump = Operation {
            source_account: Some(op_source.clone()),
            body: OperationBody::BumpSequence(stellar_xdr::BumpSequenceOp {
                bump_to: stellar_xdr::SequenceNumber(99),
            }),
        };
        let mut ops = env.tx.operations.to_vec();
        ops.push(bump);
        env.tx.operations = ops.try_into().unwrap();

        let view = decode_envelope(&to_b64(&TransactionEnvelope::Tx(env))).unwrap();
        assert_eq!(view.operations.len(), 2);
        assert_eq!(view.operations[0].kind, "InvokeContract");
        assert_eq!(view.operations[0].args, vec!["void"]);
        assert_eq!(view.operations[1].kind, "BumpSequence");
        assert_eq!(
            view.operations[1].source.as_deref(),
            Some(op_source.to_string().as_str())
        );
        assert!(view.operations[1].contract.is_none());
    }

    #[test]
    fn renders_soroban_footprint_and_resources() {
        let mut env = v1(invoke_envelope(vec![]));
        let key = stellar_xdr::LedgerKey::ContractCode(stellar_xdr::LedgerKeyContractCode {
            hash: stellar_xdr::Hash([7; 32]),
        });
        env.tx.ext = TransactionExt::V1(SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint {
                    read_only: VecM::try_from(vec![key.clone(), key.clone()]).unwrap(),
                    read_write: VecM::try_from(vec![key]).unwrap(),
                },
                instructions: 5000,
                disk_read_bytes: 100,
                write_bytes: 50,
            },
            resource_fee: 1234,
        });
        let view = decode_envelope(&to_b64(&TransactionEnvelope::Tx(env))).unwrap();
        let soroban = view.soroban.unwrap();
        assert_eq!((soroban.read_only, soroban.read_write), (2, 1));
        assert_eq!(soroban.instructions, 5000);
        assert_eq!(soroban.resource_fee, 1234);
    }

    #[test]
    fn names_signer_when_hint_matches_source() {
        let mut env = v1(invoke_envelope(vec![]));
        let source = ed25519_key(&env.tx.source_account);
        env.signatures = VecM::try_from(vec![DecoratedSignature {
            hint: SignatureHint([source[28], source[29], source[30], source[31]]),
            signature: stellar_xdr::Signature(vec![0; 64].try_into().unwrap()),
        }])
        .unwrap();
        let view = decode_envelope(&to_b64(&TransactionEnvelope::Tx(env))).unwrap();
        assert_eq!(view.signatures.len(), 1);
        assert_eq!(view.signatures[0].signer.as_deref(), Some(SOURCE));
    }

    #[test]
    fn fee_bump_fee_above_u32_is_not_truncated() {
        let inner = v1(invoke_envelope(vec![]));
        let fee = i64::from(u32::MAX) + 10;
        let env = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: FeeBumpTransaction {
                fee_source: MuxedAccount::Ed25519(Uint256([9; 32])),
                fee,
                inner_tx: FeeBumpTransactionInnerTx::Tx(inner),
                ext: FeeBumpTransactionExt::V0,
            },
            signatures: VecM::try_from(vec![DecoratedSignature {
                hint: SignatureHint([9; 4]),
                signature: stellar_xdr::Signature(vec![0; 64].try_into().unwrap()),
            }])
            .unwrap(),
        });
        let view = decode_envelope(&to_b64(&env)).unwrap();
        assert_eq!(view.envelope_type, "tx_fee_bump");
        assert_eq!(view.fee, 250);
        let bump = view.fee_bump.unwrap();
        assert_eq!(bump.fee, fee);
        assert_eq!(
            bump.signatures[0].signer.as_deref(),
            Some(bump.fee_source.as_str())
        );
        let json = serde_json::to_value(&bump).unwrap();
        assert_eq!(json["fee"], serde_json::json!(fee));
    }

    #[test]
    fn rejects_trailing_bytes_after_envelope() {
        let env = invoke_envelope(vec![]);
        let mut raw = env.to_xdr(Limits::none()).unwrap();
        raw.extend_from_slice(&[0, 0, 0, 0]);
        let err = decode_envelope(&STANDARD.encode(raw)).unwrap_err();
        assert!(matches!(err, DecodeError::XdrParse(..)), "{err}");
    }

    #[test]
    fn rejects_invalid_base64_and_empty_input() {
        assert!(matches!(
            decode_envelope("not base64!"),
            Err(DecodeError::Base64(_))
        ));
        assert!(matches!(
            decode_envelope("  \n"),
            Err(DecodeError::EmptyPayload)
        ));
    }

    #[test]
    fn renders_nested_scvals_in_arg_syntax() {
        let vec = ScVal::Vec(Some(
            vec!["a, b".into_scval().unwrap(), ScVal::Bool(true)]
                .try_into()
                .unwrap(),
        ));
        assert_eq!(render_scval(&vec), r#"vec:[string:"a, b", bool:true]"#);
        assert_eq!(
            render_scval(&ScVal::Symbol(stellar_xdr::ScSymbol(
                "transfer".try_into().unwrap()
            ))),
            "symbol:transfer"
        );
        assert_eq!(
            render_scval(&ScVal::I128(stellar_xdr::Int128Parts {
                hi: -1,
                lo: u64::MAX
            })),
            "i128:-1"
        );
    }
}
