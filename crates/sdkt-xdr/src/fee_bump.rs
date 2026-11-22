//! Build fee-bump transaction envelopes while preserving the signed inner transaction.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Serialize;
use stellar_strkey::Strkey;
use stellar_xdr::{
    FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionExt,
    FeeBumpTransactionInnerTx, Limits, MuxedAccount, ReadXdr, Transaction, TransactionEnvelope,
    TransactionExt, Uint256, WriteXdr,
};
use thiserror::Error;

/// Minimum inclusion fee per operation used when validating a locally built wrapper.
const MIN_BASE_FEE: i64 = 100;

#[derive(Debug, Error)]
pub enum FeeBumpError {
    #[error("invalid transaction envelope: {0}")]
    InvalidEnvelope(String),
    #[error("invalid fee source: {0}")]
    InvalidFeeSource(String),
    #[error("fee must be positive")]
    InvalidFee,
    #[error(
        "fee-bump fee must be at least the minimum of {minimum} stroops (provided {provided})"
    )]
    FeeTooLow { minimum: i64, provided: i64 },
    #[error("failed to serialize fee-bump envelope: {0}")]
    Serialization(String),
}

/// Input arguments for [`wrap_fee_bump`].
#[derive(Debug, Clone)]
pub struct FeeBumpWrapParams {
    pub envelope: String,
    pub fee_source: String,
    pub fee: i64,
}

/// Result of wrapping a transaction in a fee-bump envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeeBumpResult {
    pub envelope: String,
    pub fee_source: String,
    pub inner_fee: i64,
    pub fee: i64,
    pub minimum_fee: i64,
}

/// Wrap a signed or unsigned V1 transaction in a fee-bump envelope.
///
/// The inner transaction and its signatures are retained byte-for-byte at the
/// XDR value level. Nested fee-bump envelopes and legacy V0 envelopes are not
/// valid fee-bump inner transactions and are rejected.
pub fn wrap_fee_bump(params: &FeeBumpWrapParams) -> Result<FeeBumpResult, FeeBumpError> {
    if params.fee <= 0 {
        return Err(FeeBumpError::InvalidFee);
    }

    let compact: String = params
        .envelope
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if compact.is_empty() {
        return Err(FeeBumpError::InvalidEnvelope("payload is empty".into()));
    }
    let bytes = STANDARD
        .decode(compact)
        .map_err(|e| FeeBumpError::InvalidEnvelope(e.to_string()))?;
    let envelope = TransactionEnvelope::from_xdr(&bytes, Limits::none())
        .map_err(|e| FeeBumpError::InvalidEnvelope(e.to_string()))?;
    let inner = match envelope {
        TransactionEnvelope::Tx(inner) => inner,
        TransactionEnvelope::TxV0(_) => {
            return Err(FeeBumpError::InvalidEnvelope(
                "legacy V0 envelopes cannot be fee-bump inner transactions".into(),
            ));
        }
        TransactionEnvelope::TxFeeBump(_) => {
            return Err(FeeBumpError::InvalidEnvelope(
                "nested fee-bump envelopes are not supported".into(),
            ));
        }
    };

    let fee_source_key = Strkey::from_string(params.fee_source.trim())
        .map_err(|e| FeeBumpError::InvalidFeeSource(e.to_string()))?;
    let fee_source = match fee_source_key {
        Strkey::PublicKeyEd25519(key) => MuxedAccount::Ed25519(Uint256(key.0)),
        _ => {
            return Err(FeeBumpError::InvalidFeeSource(
                "expected an Ed25519 G... account".into(),
            ));
        }
    };

    let (_, minimum_fee) = minimum_fee_for_inner(&inner.tx)?;
    if params.fee < minimum_fee {
        return Err(FeeBumpError::FeeTooLow {
            minimum: minimum_fee,
            provided: params.fee,
        });
    }

    let inner_fee = i64::from(inner.tx.fee);
    let fee_source_text = fee_source.to_string();
    let wrapped = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
        tx: FeeBumpTransaction {
            fee: params.fee,
            fee_source,
            inner_tx: FeeBumpTransactionInnerTx::Tx(inner),
            ext: FeeBumpTransactionExt::V0,
        },
        signatures: Default::default(),
    });
    let bytes = wrapped
        .to_xdr(Limits::none())
        .map_err(|e| FeeBumpError::Serialization(e.to_string()))?;

    Ok(FeeBumpResult {
        envelope: STANDARD.encode(bytes),
        fee_source: fee_source_text,
        inner_fee,
        fee: params.fee,
        minimum_fee,
    })
}

/// Compute a fee-bump total that meets both the base-fee floor and the inner
/// transaction's inclusion-fee rate. Soroban resource fees are carried
/// through unchanged; only the inclusion fee is raised by the wrapper.
fn minimum_fee_for_inner(inner: &Transaction) -> Result<(i64, i64), FeeBumpError> {
    let operation_count =
        i64::try_from(inner.operations.len()).map_err(|_| FeeBumpError::InvalidFee)?;
    if operation_count <= 0 {
        return Err(FeeBumpError::InvalidFee);
    }
    let resource_fee = match &inner.ext {
        TransactionExt::V0 => 0,
        TransactionExt::V1(data) => data.resource_fee,
    };
    if resource_fee < 0 {
        return Err(FeeBumpError::InvalidFee);
    }
    let inner_inclusion_fee = i64::from(inner.fee)
        .checked_sub(resource_fee)
        .filter(|fee| *fee >= 0)
        .ok_or(FeeBumpError::InvalidFee)?;
    let effective_operation_count = operation_count
        .checked_add(1)
        .ok_or(FeeBumpError::InvalidFee)?;
    let base_fee_minimum = MIN_BASE_FEE
        .checked_mul(effective_operation_count)
        .ok_or(FeeBumpError::InvalidFee)?;
    let rate_numerator = inner_inclusion_fee
        .checked_mul(effective_operation_count)
        .ok_or(FeeBumpError::InvalidFee)?;
    let rate_minimum = rate_numerator
        .checked_div(operation_count)
        .and_then(|fee| {
            fee.checked_add(if rate_numerator % operation_count != 0 {
                1
            } else {
                0
            })
        })
        .ok_or(FeeBumpError::InvalidFee)?;
    let minimum_fee = resource_fee
        .checked_add(base_fee_minimum.max(rate_minimum))
        .ok_or(FeeBumpError::InvalidFee)?;
    Ok((resource_fee, minimum_fee))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        LedgerFootprint, Operation, SorobanResources, SorobanTransactionData,
        SorobanTransactionDataExt, TransactionV1Envelope, VecM,
    };

    const FEE_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn input_envelope(fee: u32) -> String {
        input_envelope_with(fee, 1, TransactionExt::V0)
    }

    fn input_envelope_with(fee: u32, operation_count: usize, ext: TransactionExt) -> String {
        let tx = Transaction {
            fee,
            operations: VecM::try_from(vec![Operation::default(); operation_count]).unwrap(),
            ext,
            ..Transaction::default()
        };
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: Default::default(),
        });
        STANDARD.encode(envelope.to_xdr(Limits::none()).unwrap())
    }

    #[test]
    fn wraps_transaction_and_keeps_inner_fee() {
        let result = wrap_fee_bump(&FeeBumpWrapParams {
            envelope: input_envelope(100),
            fee_source: FEE_SOURCE.into(),
            fee: 1_000,
        })
        .unwrap();
        let bytes = STANDARD.decode(result.envelope).unwrap();
        let TransactionEnvelope::TxFeeBump(envelope) =
            TransactionEnvelope::from_xdr(&bytes, Limits::none()).unwrap()
        else {
            panic!("expected fee-bump envelope");
        };
        assert_eq!(envelope.tx.fee, 1_000);
        let FeeBumpTransactionInnerTx::Tx(inner) = envelope.tx.inner_tx;
        assert_eq!(inner.tx.fee, 100);
        assert_eq!(inner.tx.operations.len(), 1);
    }

    #[test]
    fn rejects_fee_below_minimum() {
        let error = wrap_fee_bump(&FeeBumpWrapParams {
            envelope: input_envelope(100),
            fee_source: FEE_SOURCE.into(),
            fee: 1,
        })
        .unwrap_err();
        assert!(error.to_string().contains("minimum"));
    }

    #[test]
    fn one_operation_fee_bump_preserves_or_exceeds_inner_fee_rate() {
        let params = |fee| FeeBumpWrapParams {
            envelope: input_envelope(1_000),
            fee_source: FEE_SOURCE.into(),
            fee,
        };
        let error = wrap_fee_bump(&params(1_999)).unwrap_err();
        assert!(error.to_string().contains("minimum of 2000"));
        assert_eq!(wrap_fee_bump(&params(2_000)).unwrap().minimum_fee, 2_000);
    }

    #[test]
    fn two_operation_minimum_uses_the_inner_inclusion_fee_rate() {
        let params = |fee| FeeBumpWrapParams {
            envelope: input_envelope_with(200, 2, TransactionExt::V0),
            fee_source: FEE_SOURCE.into(),
            fee,
        };
        let error = wrap_fee_bump(&params(299)).unwrap_err();
        assert!(error.to_string().contains("minimum of 300"));
        assert_eq!(wrap_fee_bump(&params(300)).unwrap().minimum_fee, 300);
    }

    #[test]
    fn soroban_resource_fee_is_preserved_outside_inclusion_fee_calculation() {
        let data = SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint::default(),
                instructions: 0,
                disk_read_bytes: 0,
                write_bytes: 0,
            },
            resource_fee: 10_000,
        };
        let params = |fee| FeeBumpWrapParams {
            envelope: input_envelope_with(10_100, 1, TransactionExt::V1(data.clone())),
            fee_source: FEE_SOURCE.into(),
            fee,
        };
        let error = wrap_fee_bump(&params(10_199)).unwrap_err();
        assert!(error.to_string().contains("minimum of 10200"));
        let result = wrap_fee_bump(&params(10_200)).unwrap();
        assert_eq!(result.inner_fee, 10_100);
        assert_eq!(result.minimum_fee, 10_200);
    }
}
