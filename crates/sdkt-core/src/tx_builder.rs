use stellar_strkey::Strkey;
use stellar_xdr::{
    Error as XdrError, Limits, Memo, MuxedAccount, Operation, Preconditions, SequenceNumber,
    TimeBounds, Transaction, TransactionEnvelope, TransactionExt, TransactionV1Envelope, Uint256,
    VecM, WriteXdr,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("Invalid source account: {0}")]
    InvalidSourceAccount(String),
    #[error("Invalid memo: {0}")]
    InvalidMemo(String),
    #[error("XDR encoding error: {0}")]
    XdrError(#[from] XdrError),
    #[error("Missing operation")]
    MissingOperation,
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

/// A builder for constructing Stellar `TransactionEnvelope`s (V1).
pub struct TxBuilder {
    source_account: Option<MuxedAccount>,
    sequence_number: Option<i64>,
    fee: u32,
    memo: Memo,
    preconditions: Preconditions,
    operations: Vec<Operation>,
    ext: TransactionExt,
}

impl Default for TxBuilder {
    fn default() -> Self {
        Self {
            source_account: None,
            sequence_number: None,
            fee: 100, // Default minimum fee
            memo: Memo::None,
            preconditions: Preconditions::None,
            operations: Vec::new(),
            ext: TransactionExt::V0,
        }
    }
}

impl TxBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source account from a public key (G...)
    pub fn source_account(mut self, address: &str) -> Result<Self, BuilderError> {
        let key = Strkey::from_string(address)
            .map_err(|e| BuilderError::InvalidSourceAccount(e.to_string()))?;

        if let Strkey::PublicKeyEd25519(pk) = key {
            self.source_account = Some(MuxedAccount::Ed25519(Uint256(pk.0)));
            Ok(self)
        } else {
            Err(BuilderError::InvalidSourceAccount(
                "Must be a G... public key".into(),
            ))
        }
    }

    /// Set the sequence number
    pub fn sequence_number(mut self, seq: i64) -> Self {
        self.sequence_number = Some(seq);
        self
    }

    /// Set the base fee
    pub fn fee(mut self, fee: u32) -> Self {
        self.fee = fee;
        self
    }

    /// Set a text memo
    pub fn memo_text(mut self, text: &str) -> Result<Self, BuilderError> {
        let bytes = text.as_bytes();
        let vec_m = bytes
            .try_into()
            .map_err(|_| BuilderError::InvalidMemo("Text too long".into()))?;
        self.memo = Memo::Text(vec_m);
        Ok(self)
    }

    /// Set an ID memo
    pub fn memo_id(mut self, id: u64) -> Self {
        self.memo = Memo::Id(id);
        self
    }

    /// Add a pre-built Operation
    pub fn add_operation(mut self, op: Operation) -> Self {
        self.operations.push(op);
        self
    }

    /// Set ext to V1 with SorobanTransactionData (for host functions)
    pub fn set_ext(mut self, ext: TransactionExt) -> Self {
        self.ext = ext;
        self
    }

    /// Set timebounds timeout (from now to `0` means unbounded for now, or we can just leave None)
    /// Real implementations might take a minTime and maxTime. We provide a basic max_time setter.
    pub fn max_time(mut self, max_time: u64) -> Self {
        self.preconditions = Preconditions::Time(TimeBounds {
            min_time: stellar_xdr::TimePoint(0),
            max_time: stellar_xdr::TimePoint(max_time),
        });
        self
    }

    /// Build the `TransactionEnvelope` (unsigned).
    pub fn build(self) -> Result<TransactionEnvelope, BuilderError> {
        let source_account = self
            .source_account
            .ok_or_else(|| BuilderError::InvalidSourceAccount("Missing source account".into()))?;
        let seq = self
            .sequence_number
            .ok_or_else(|| BuilderError::InvalidSourceAccount("Missing sequence number".into()))?;

        if self.operations.is_empty() {
            return Err(BuilderError::MissingOperation);
        }

        let ops_vec: VecM<Operation, 100> = self
            .operations
            .try_into()
            .map_err(|_| BuilderError::InvalidOperation("Too many operations".into()))?;

        let tx = Transaction {
            source_account,
            fee: self.fee,
            seq_num: SequenceNumber(seq),
            cond: self.preconditions,
            memo: self.memo,
            operations: ops_vec,
            ext: self.ext,
        };

        let env_v1 = TransactionV1Envelope {
            tx,
            signatures: VecM::default(), // Empty signatures
        };

        Ok(TransactionEnvelope::Tx(env_v1))
    }

    /// Build and encode to base64
    pub fn build_base64(self) -> Result<String, BuilderError> {
        let envelope = self.build()?;
        let b64 = envelope.to_xdr_base64(Limits::none())?;
        Ok(b64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{OperationBody, ReadXdr};

    #[test]
    fn test_builder_missing_source() {
        let builder = TxBuilder::new().sequence_number(1);
        assert!(matches!(
            builder.build(),
            Err(BuilderError::InvalidSourceAccount(_))
        ));
    }

    const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    #[test]
    fn test_builder_missing_operations() {
        let builder = TxBuilder::new()
            .source_account(TEST_SOURCE)
            .unwrap()
            .sequence_number(1);
        assert!(matches!(
            builder.build(),
            Err(BuilderError::MissingOperation)
        ));
    }

    #[test]
    fn test_builder_invalid_memo() {
        let res = TxBuilder::new().memo_text(
            "this text is way too long to fit into a stellar memo which has a 28 char limit",
        );
        assert!(matches!(res, Err(BuilderError::InvalidMemo(_))));
    }

    #[test]
    fn test_valid_build_and_encode() {
        let op = Operation {
            source_account: None,
            body: OperationBody::BumpSequence(stellar_xdr::BumpSequenceOp {
                bump_to: SequenceNumber(10),
            }),
        };

        let b64 = TxBuilder::new()
            .source_account(TEST_SOURCE)
            .unwrap()
            .sequence_number(123)
            .fee(150)
            .memo_id(456)
            .add_operation(op)
            .build_base64()
            .unwrap();

        // Valid base64 output that decodes back
        let envelope = TransactionEnvelope::from_xdr_base64(b64, Limits::none()).unwrap();
        match envelope {
            TransactionEnvelope::Tx(env) => {
                assert_eq!(env.tx.fee, 150);
                assert_eq!(env.tx.seq_num.0, 123);
                match env.tx.memo {
                    Memo::Id(id) => assert_eq!(id, 456),
                    _ => panic!("Wrong memo"),
                }
            }
            _ => panic!("Expected Tx (V1)"),
        }
    }
}
