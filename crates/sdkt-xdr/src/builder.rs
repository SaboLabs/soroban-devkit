//! Transaction envelope building logic for Soroban.
//!
//! Provides a streamlined interface for assembling standard Stellar/Soroban
//! transactions without implementing signing (which is handled later).

use crate::DecodeError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use stellar_strkey::Strkey;
use stellar_xdr::{
    AccountId, ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Memo,
    MuxedAccount, Operation, OperationBody, Preconditions, PublicKey, ReadXdr, ScAddress, ScSymbol,
    SequenceNumber, Transaction, TransactionEnvelope, TransactionV1Envelope, Uint256, VecM,
    WriteXdr,
};

/// Parameters for building a basic contract invocation transaction.
pub struct InvokeTransactionParams {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops
    pub fee: u32,
    /// Contract ID to invoke (C...)
    pub contract_id: String,
    /// Function name
    pub function: String,
    /// Optional arguments (as pre-encoded ScVal base64 strings)
    pub args: Vec<String>,
}

/// Decode a G... StrKey into an `AccountId`.
pub fn decode_account_id(pubkey: &str) -> Result<AccountId, DecodeError> {
    let key = Strkey::from_string(pubkey)
        .map_err(|e| DecodeError::Extraction(format!("Invalid public key: {}", e)))?;

    match key {
        Strkey::PublicKeyEd25519(pk) => {
            Ok(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk.0))))
        }
        _ => Err(DecodeError::Extraction(
            "Expected ED25519 Public Key".into(),
        )),
    }
}

/// Decode a C... StrKey into a 32-byte `Hash`.
pub fn decode_contract_id(contract_id: &str) -> Result<Hash, DecodeError> {
    let key = Strkey::from_string(contract_id)
        .map_err(|e| DecodeError::Extraction(format!("Invalid contract ID: {}", e)))?;

    match key {
        Strkey::Contract(c) => Ok(Hash(c.0)),
        _ => Err(DecodeError::Extraction("Expected Contract ID".into())),
    }
}

/// Builds a `TransactionEnvelope` (V1) for invoking a smart contract.
pub fn build_invoke_transaction(params: &InvokeTransactionParams) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let contract_hash = decode_contract_id(&params.contract_id)?;

    // Parse ScVal args from Base64
    let mut scval_args = Vec::new();
    for arg_b64 in &params.args {
        let raw = STANDARD.decode(arg_b64)?;
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let val = stellar_xdr::ScVal::read_xdr(&mut l)
            .map_err(|e| DecodeError::XdrParse("ScVal arg".into(), e))?;
        scval_args.push(val);
    }
    let args_vec = VecM::try_from(scval_args)
        .map_err(|_| DecodeError::Extraction("Too many arguments".into()))?;

    let function_name = ScSymbol(
        params
            .function
            .as_bytes()
            .try_into()
            .map_err(|_| DecodeError::Extraction("Function name too long".into()))?,
    );

    let invoke_op = InvokeHostFunctionOp {
        host_function: HostFunction::InvokeContract(InvokeContractArgs {
            contract_address: ScAddress::Contract(ContractId(contract_hash)),
            function_name,
            args: args_vec,
        }),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(invoke_op),
    };

    let tx = Transaction {
        source_account: MuxedAccount::Ed25519(match source_account.0 {
            PublicKey::PublicKeyTypeEd25519(u) => u,
        }),
        fee: params.fee,
        seq_num: SequenceNumber(params.sequence),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: VecM::try_from(vec![op]).unwrap(),
        ext: stellar_xdr::TransactionExt::V0,
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(), // No signatures applied yet
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

    #[test]
    fn test_decode_account_id_valid() {
        let acc = decode_account_id(TEST_SOURCE).unwrap();
        match acc.0 {
            PublicKey::PublicKeyTypeEd25519(_) => {}
        }
    }

    #[test]
    fn test_decode_account_id_invalid() {
        assert!(
            decode_account_id("GBZXLHQZGOWBZY6W3U4Z7GZGGXYVQBZWYM3XEQZ7W5Z4QXYZ5Z3XYY").is_err()
        ); // too short
        assert!(
            decode_account_id("CAZXLHQZGOWBZY6W3U4Z7GZGGXYVQBZWYM3XEQZ7W5Z4QXYZ5Z3XYYYY").is_err()
        ); // Wrong type (Contract)
        assert!(decode_account_id("not-a-key").is_err());
    }

    #[test]
    fn test_decode_contract_id_valid() {
        let hash = decode_contract_id(TEST_CONTRACT).unwrap();
        assert_eq!(hash.0.len(), 32);
    }

    #[test]
    fn test_decode_contract_id_invalid() {
        assert!(decode_contract_id(TEST_SOURCE).is_err()); // Wrong type (Account)
    }

    #[test]
    fn test_build_invoke_transaction() {
        let params = InvokeTransactionParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 12345,
            fee: 100,
            contract_id: TEST_CONTRACT.to_string(),
            function: "hello".to_string(),
            args: vec![],
        };

        let envelope = build_invoke_transaction(&params).unwrap();

        // Ensure it encodes to base64
        let raw = STANDARD.decode(&envelope).unwrap();

        // We can parse it back
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 100);
                assert_eq!(v1.tx.seq_num.0, 12345);
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_invoke_transaction_with_args() {
        // ScVal::I32(42) base64 encoded
        let arg_b64 = "AAAABAAAACo=";

        let params = InvokeTransactionParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 1,
            fee: 100,
            contract_id: TEST_CONTRACT.to_string(),
            function: "add".to_string(),
            args: vec![arg_b64.to_string()],
        };

        let envelope = build_invoke_transaction(&params).unwrap();
        assert!(!envelope.is_empty());
    }
}
