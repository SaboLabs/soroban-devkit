use stellar_strkey::Strkey;
use stellar_xdr::{
    ContractId, Error as XdrError, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp,
    Limits, Memo, MuxedAccount, Operation, OperationBody, Preconditions, ScAddress, ScSymbol,
    SequenceNumber, TimeBounds, Transaction, TransactionEnvelope, TransactionExt,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
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

    /// Add an `InvokeHostFunction` operation for a smart-contract call.
    ///
    /// This is a convenience that wraps a [`HostFunction::InvokeContract`] op
    /// without requiring the caller to hand-assemble the XDR [`Operation`].
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::InvalidOperation`] if the contract ID (`C...`)
    /// or function name is invalid.
    pub fn invoke_contract(
        mut self,
        contract_id: &str,
        function: &str,
        args: Vec<stellar_xdr::ScVal>,
    ) -> Result<Self, BuilderError> {
        let hash = decode_contract_id_direct(contract_id)
            .map_err(|e| BuilderError::InvalidOperation(format!("contract ID: {e}")))?;

        let sc_symbol: stellar_xdr::StringM<32> = function
            .as_bytes()
            .try_into()
            .map_err(|_| BuilderError::InvalidOperation("function name too long".into()))?;
        let function_name = ScSymbol(sc_symbol);

        let args_vec: VecM<stellar_xdr::ScVal> = args
            .try_into()
            .map_err(|_| BuilderError::InvalidOperation("too many args".into()))?;

        let op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(hash)),
                    function_name,
                    args: args_vec,
                }),
                auth: VecM::default(),
            }),
        };
        self.operations.push(op);
        Ok(self)
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

fn decode_contract_id_direct(contract_id: &str) -> Result<Hash, String> {
    let key = Strkey::from_string(contract_id).map_err(|e| e.to_string())?;
    match key {
        Strkey::Contract(c) => Ok(Hash(c.0)),
        _ => Err("Expected C... contract StrKey".into()),
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

    const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

    #[test]
    fn test_builder_invoke_contract_success() {
        let b64 = TxBuilder::new()
            .source_account(TEST_SOURCE)
            .unwrap()
            .sequence_number(5)
            .fee(200)
            .invoke_contract(
                TEST_CONTRACT,
                "transfer",
                vec![
                    stellar_xdr::ScVal::U32(100),
                    stellar_xdr::ScVal::String(stellar_xdr::ScString(
                        "hello".as_bytes().to_vec().try_into().unwrap(),
                    )),
                    stellar_xdr::ScVal::Bool(true),
                ],
            )
            .unwrap()
            .build_base64()
            .unwrap();
        let envelope = TransactionEnvelope::from_xdr_base64(b64, Limits::none()).unwrap();
        match envelope {
            TransactionEnvelope::Tx(env) => {
                assert_eq!(env.tx.operations.len(), 1);
                match env.tx.operations.first().unwrap().body {
                    OperationBody::InvokeHostFunction(ref op) => match op.host_function {
                        stellar_xdr::HostFunction::InvokeContract(ref args) => {
                            assert_eq!(args.function_name.to_utf8_string_lossy(), "transfer");
                            assert_eq!(args.args.len(), 3);
                        }
                        _ => panic!("Expected InvokeContract"),
                    },
                    _ => panic!("Expected InvokeHostFunction operation"),
                }
            }
            _ => panic!("Expected Tx (V1)"),
        }
    }

    #[test]
    fn test_builder_invoke_contract_empty_args() {
        let b64 = TxBuilder::new()
            .source_account(TEST_SOURCE)
            .unwrap()
            .sequence_number(1)
            .invoke_contract(TEST_CONTRACT, "hello", vec![])
            .unwrap()
            .build_base64()
            .unwrap();
        let envelope = TransactionEnvelope::from_xdr_base64(b64, Limits::none()).unwrap();
        match envelope {
            TransactionEnvelope::Tx(env) => match env.tx.operations.first().unwrap().body {
                OperationBody::InvokeHostFunction(ref op) => match op.host_function {
                    stellar_xdr::HostFunction::InvokeContract(ref args) => {
                        assert_eq!(args.function_name.to_utf8_string_lossy(), "hello");
                        assert_eq!(args.args.len(), 0);
                    }
                    _ => panic!("Expected InvokeContract"),
                },
                _ => panic!("Expected InvokeHostFunction"),
            },
            _ => panic!("Expected Tx"),
        }
    }

    #[test]
    fn test_builder_invoke_contract_invalid_contract_id() {
        let res = TxBuilder::new()
            .source_account(TEST_SOURCE)
            .unwrap()
            .sequence_number(1)
            .invoke_contract("GNOTACONTRACT", "hello", vec![]);
        assert!(matches!(res, Err(BuilderError::InvalidOperation(_))));
    }
}
