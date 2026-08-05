//! Offline, pure transaction validation (no RPC).
//!
//! [`validate`] inspects a decoded [`stellar_xdr::TransactionEnvelope`] and
//! produces a [`TransactionValidationReport`] containing hard errors and soft
//! warnings. Validation never touches the network: it only inspects the XDR
//! structure the caller already holds. Use it as a pre-flight check before
//! simulation / submission.

use serde::{Deserialize, Serialize};
use stellar_xdr::{Memo, Operation, OperationBody, Transaction, TransactionEnvelope};

/// Default minimum base fee (stroops) used for hard validation.
pub const MIN_FEE_STROOPS: u32 = 100;
/// Conservative hard cap on serialized envelope size in bytes. Stellar's
/// current network limit is 100 KiB; this allows checking oversize locally.
pub const MAX_XDR_SIZE: usize = 100 * 1024;

/// The result of running [`validate`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionValidationReport {
    /// True when there are no errors (warnings are allowed).
    pub valid: bool,
    /// Hard failures that must be fixed before submission.
    pub errors: Vec<ValidationError>,
    /// Non-blocking observations.
    pub warnings: Vec<ValidationWarning>,
}

/// A hard validation failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidationError {
    /// An empty base64/raw payload was supplied.
    EmptyTransaction,
    /// The payload could not be decoded as a `TransactionEnvelope`.
    MalformedEnvelope(String),
    /// The envelope has zero operations.
    EmptyOperations,
    /// The base fee is below the network minimum.
    FeeTooLow { fee: u32, min: u32 },
    /// The sequence number is not a positive value.
    InvalidSequence(i64),
    /// The transaction has no source account (cannot be expressed).
    MissingSourceAccount,
    /// The serialized envelope exceeds [`MAX_XDR_SIZE`].
    XdrTooLarge { size: usize, max: usize },
    /// An `InvokeHostFunction` references a contract but the function name is empty.
    MissingFunctionName,
}

impl ValidationError {
    /// Human-readable message (used by CLI pretty output).
    pub fn message(&self) -> String {
        match self {
            ValidationError::EmptyTransaction => "transaction is empty".into(),
            ValidationError::MalformedEnvelope(e) => format!("malformed envelope: {e}"),
            ValidationError::EmptyOperations => "transaction has no operations".into(),
            ValidationError::FeeTooLow { fee, min } => {
                format!("fee {fee} stroops below minimum {min}")
            }
            ValidationError::InvalidSequence(s) => format!("invalid sequence number {s}"),
            ValidationError::MissingSourceAccount => "missing source account".into(),
            ValidationError::XdrTooLarge { size, max } => {
                format!("XDR size {size} bytes exceeds limit {max}")
            }
            ValidationError::MissingFunctionName => {
                "contract invocation missing function name".into()
            }
        }
    }
}

/// A soft, non-blocking observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ValidationWarning {
    /// Memo is of kind `Hash` or `Return` — unusual but valid.
    UnusualMemo(String),
    /// Operation type is not commonly seen on Soroban contracts.
    NonContractOperation(String),
}

/// Validate a decoded transaction envelope.
///
/// Pure: performs no I/O and no RPC. Supply the envelope you already decoded
/// (e.g. via base64 → [`stellar_xdr::TransactionEnvelope::read_xdr`]).
pub fn validate(envelope: &TransactionEnvelope) -> TransactionValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    match envelope {
        TransactionEnvelope::Tx(tx) => validate_tx(&tx.tx, &mut errors, &mut warnings),
        TransactionEnvelope::TxV0(tx) => validate_v0(&tx.tx, &mut errors, &mut warnings),
        TransactionEnvelope::TxFeeBump(fb) => {
            // Fee-bump envelopes wrap a nested transaction; validate the inner one.
            let stellar_xdr::FeeBumpTransactionInnerTx::Tx(v1) = &fb.tx.inner_tx;
            validate_tx(&v1.tx, &mut errors, &mut warnings);
        }
    }

    TransactionValidationReport {
        valid: errors.is_empty(),
        errors,
        warnings,
    }
}

/// Validate a `TransactionEnvelope` supplied as raw bytes (already parsed).
pub fn validate_raw(raw: &[u8], envelope: &TransactionEnvelope) -> TransactionValidationReport {
    let size = raw.len();
    let mut report = validate(envelope);
    if size > MAX_XDR_SIZE {
        report.errors.push(ValidationError::XdrTooLarge {
            size,
            max: MAX_XDR_SIZE,
        });
        report.valid = false;
    }
    report
}

/// Validate a base64 `TransactionEnvelope`.
///
/// This both decodes the payload and validates it, returning a report. The
/// `base64` crate is re-exported through the `stellar_xdr` feature set; we
/// decode using `stellar_xdr::ReadXdr` from the base64 string.
pub fn validate_base64(b64: &str) -> TransactionValidationReport {
    let Ok(decoded) = <TransactionEnvelope as stellar_xdr::ReadXdr>::from_xdr_base64(
        b64,
        stellar_xdr::Limits::none(),
    ) else {
        return TransactionValidationReport {
            valid: false,
            errors: vec![ValidationError::MalformedEnvelope("bad base64".into())],
            warnings: Vec::new(),
        };
    };
    validate(&decoded)
}

fn validate_tx(
    tx: &Transaction,
    errors: &mut Vec<ValidationError>,
    warnings: &mut Vec<ValidationWarning>,
) {
    if tx.operations.is_empty() {
        errors.push(ValidationError::EmptyOperations);
    }

    if tx.fee < MIN_FEE_STROOPS {
        errors.push(ValidationError::FeeTooLow {
            fee: tx.fee,
            min: MIN_FEE_STROOPS,
        });
    }

    if tx.seq_num.0 <= 0 {
        errors.push(ValidationError::InvalidSequence(tx.seq_num.0));
    }

    match &tx.memo {
        Memo::Hash(_) | Memo::Return(_) => {
            warnings.push(ValidationWarning::UnusualMemo(format!("{:?}", tx.memo)));
        }
        _ => {}
    }

    for op in tx.operations.iter() {
        validate_operation(op, errors, warnings);
    }
}

fn validate_v0(
    tx: &stellar_xdr::TransactionV0,
    errors: &mut Vec<ValidationError>,
    warnings: &mut Vec<ValidationWarning>,
) {
    if tx.operations.is_empty() {
        errors.push(ValidationError::EmptyOperations);
    }
    if tx.fee < MIN_FEE_STROOPS {
        errors.push(ValidationError::FeeTooLow {
            fee: tx.fee,
            min: MIN_FEE_STROOPS,
        });
    }
    if tx.seq_num.0 <= 0 {
        errors.push(ValidationError::InvalidSequence(tx.seq_num.0));
    }
    match &tx.memo {
        Memo::Hash(_) | Memo::Return(_) => {
            warnings.push(ValidationWarning::UnusualMemo(format!("{:?}", tx.memo)));
        }
        _ => {}
    }
    for op in tx.operations.iter() {
        validate_operation(op, errors, warnings);
    }
}

fn validate_operation(
    op: &Operation,
    errors: &mut Vec<ValidationError>,
    warnings: &mut Vec<ValidationWarning>,
) {
    match &op.body {
        OperationBody::InvokeHostFunction(hf) => {
            use stellar_xdr::HostFunction;
            if let HostFunction::InvokeContract(args) = &hf.host_function {
                if args.function_name.to_utf8_string_lossy().is_empty() {
                    errors.push(ValidationError::MissingFunctionName);
                }
            }
        }
        // Soroban allows only a small set of operations; flag the rest as a
        // warning rather than an error (transactions can mix classic + Soroban ops).
        OperationBody::BumpSequence(_)
        | OperationBody::ManageData(_)
        | OperationBody::SetOptions(_)
        | OperationBody::ChangeTrust(_)
        | OperationBody::Payment(_)
        | OperationBody::CreateAccount(_) => {
            warnings.push(ValidationWarning::NonContractOperation(
                "classic operation(s) present".into(),
            ));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo, MuxedAccount,
        Operation, OperationBody, ScAddress, ScSymbol, SequenceNumber, Transaction,
        TransactionEnvelope, TransactionV1Envelope, Uint256, VecM, WriteXdr,
    };

    fn dummy_source() -> MuxedAccount {
        MuxedAccount::Ed25519(Uint256([1u8; 32]))
    }

    fn make_envelope(fee: u32, seq: i64, ops: Vec<Operation>, memo: Memo) -> TransactionEnvelope {
        let tx = Transaction {
            source_account: dummy_source(),
            fee,
            seq_num: SequenceNumber(seq),
            cond: stellar_xdr::Preconditions::None,
            memo,
            operations: ops.try_into().unwrap(),
            ext: stellar_xdr::TransactionExt::V0,
        };
        let env_v1 = TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        };
        TransactionEnvelope::Tx(env_v1)
    }

    fn make_invoke_contract_op(function: &str) -> Operation {
        let contract_id = stellar_xdr::ContractId(Hash([2u8; 32]));
        Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(contract_id),
                    function_name: ScSymbol(function.as_bytes().try_into().unwrap()),
                    args: VecM::default(),
                }),
                auth: VecM::default(),
            }),
        }
    }

    #[test]
    fn valid_tx() {
        let env = make_envelope(200, 10, vec![make_invoke_contract_op("hello")], Memo::None);
        let report = validate(&env);
        assert!(report.valid);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn empty_tx() {
        let env = make_envelope(200, 10, vec![], Memo::None);
        let report = validate(&env);
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::EmptyOperations)
        ));
    }

    #[test]
    fn invalid_fee() {
        let env = make_envelope(50, 10, vec![make_invoke_contract_op("hello")], Memo::None);
        let report = validate(&env);
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::FeeTooLow { fee: 50, min: 100 })
        ));
    }

    #[test]
    fn invalid_sequence() {
        let env = make_envelope(200, 0, vec![make_invoke_contract_op("hello")], Memo::None);
        let report = validate(&env);
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::InvalidSequence(0))
        ));
    }

    #[test]
    fn malformed_envelope() {
        let report = validate_base64("not base64 at all");
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::MalformedEnvelope(_))
        ));
    }

    #[test]
    fn oversize_xdr() {
        let env = make_envelope(200, 10, vec![make_invoke_contract_op("hello")], Memo::None);
        let mut buf = Vec::new();
        use stellar_xdr::Limited;
        let mut l = Limited::new(&mut buf, stellar_xdr::Limits::none());
        env.write_xdr(&mut l).unwrap();
        let oversized = vec![0u8; MAX_XDR_SIZE + 1];
        let report = validate_raw(&oversized, &env);
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::XdrTooLarge { .. })
        ));
    }

    #[test]
    fn missing_function_name() {
        let op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(stellar_xdr::ContractId(Hash([3u8; 32]))),
                    function_name: ScSymbol("".as_bytes().try_into().unwrap()),
                    args: VecM::default(),
                }),
                auth: VecM::default(),
            }),
        };
        let env = make_envelope(200, 10, vec![op], Memo::None);
        let report = validate(&env);
        assert!(!report.valid);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::MissingFunctionName)
        ));
    }

    #[test]
    fn classic_operation_warning() {
        let op = Operation {
            source_account: None,
            body: OperationBody::ManageData(stellar_xdr::ManageDataOp {
                data_name: stellar_xdr::StringM::try_from("foo".as_bytes())
                    .unwrap()
                    .into(),
                data_value: None,
            }),
        };
        let env = make_envelope(200, 10, vec![op], Memo::None);
        let report = validate(&env);
        assert!(report.valid); // warnings don't invalidate
        assert!(report
            .warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::NonContractOperation(_))));
    }
}
