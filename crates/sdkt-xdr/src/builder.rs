//! Transaction envelope building logic for Soroban.
//!
//! Provides a streamlined interface for assembling standard Stellar/Soroban
//! transactions without implementing signing (which is handled later).

use crate::DecodeError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};
use stellar_strkey::Strkey;
use stellar_xdr::{
    AccountId, BytesM, ContractExecutable, ContractId, ContractIdPreimage,
    ContractIdPreimageFromAddress, CreateContractArgs, CreateContractArgsV2, ExtendFootprintTtlOp,
    ExtensionPoint, Hash, HashIdPreimage, HashIdPreimageContractId, HostFunction,
    InvokeContractArgs, InvokeHostFunctionOp, LedgerFootprint, LedgerKey, Memo, MuxedAccount,
    Operation, OperationBody, Preconditions, PublicKey, ReadXdr, RestoreFootprintOp, ScAddress,
    ScSymbol, ScVal, SequenceNumber, SorobanAuthorizationEntry, SorobanResources,
    SorobanTransactionData, SorobanTransactionDataExt, Transaction, TransactionEnvelope,
    TransactionExt, TransactionV1Envelope, Uint256, VecM, WriteXdr,
};

/// Parameters for building a basic contract invocation transaction.
#[derive(Debug, Clone)]
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
        ext: TransactionExt::V0,
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

/// Builds a final contract-invocation envelope with `SorobanTransactionData`
/// (ext V1 — required by the network for host-function transactions) and
/// authorization entries returned by simulation. Sign after this.
pub fn build_invoke_transaction_with_data(
    params: &InvokeTransactionParams,
    soroban_data: SorobanTransactionData,
    auth_entries: Vec<SorobanAuthorizationEntry>,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let contract_hash = decode_contract_id(&params.contract_id)?;

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

    let auth_vec_m = VecM::try_from(auth_entries)
        .map_err(|_| DecodeError::Extraction("Too many auth entries".into()))?;

    let invoke_op = InvokeHostFunctionOp {
        host_function: HostFunction::InvokeContract(InvokeContractArgs {
            contract_address: ScAddress::Contract(ContractId(contract_hash)),
            function_name,
            args: args_vec,
        }),
        auth: auth_vec_m,
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Parameters for building a `UploadContractWasm` transaction.
pub struct UploadWasmParams {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops
    pub fee: u32,
    /// Raw WASM bytecode to upload
    pub wasm_bytes: Vec<u8>,
}

/// Parameters for building a `CreateContract` transaction.
#[derive(Debug, Clone)]
pub struct CreateContractParams {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops
    pub fee: u32,
    /// WASM hash (32 bytes) from a prior upload
    pub wasm_hash: [u8; 32],
    /// Source account address for contract ID derivation
    pub deployer_address: String,
    /// 20-byte salt for unique contract ID
    pub salt: [u8; 20],
}

/// Parameters for building a `CreateContractV2` transaction with constructor arguments.
#[derive(Debug, Clone)]
pub struct CreateContractV2Params {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops
    pub fee: u32,
    /// WASM hash (32 bytes) from a prior upload
    pub wasm_hash: [u8; 32],
    /// Source account address for contract ID derivation
    pub deployer_address: String,
    /// 20-byte salt for unique contract ID
    pub salt: [u8; 20],
    /// Constructor arguments (as pre-encoded ScVal base64 strings)
    pub constructor_args: Vec<String>,
}

/// Builds an initial `TransactionEnvelope` for uploading a WASM binary.
/// This is used for simulation; the final transaction uses `build_upload_wasm_tx_with_data`.
pub fn build_upload_wasm_tx(params: &UploadWasmParams) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;

    let wasm_bytes_m = BytesM::try_from(params.wasm_bytes.as_slice())
        .map_err(|_| DecodeError::Extraction("WASM bytes exceed max length".into()))?;

    let upload_op = InvokeHostFunctionOp {
        host_function: HostFunction::UploadContractWasm(wasm_bytes_m),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(upload_op),
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
        ext: TransactionExt::V0,
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a final `TransactionEnvelope` for uploading a WASM binary with SorobanTransactionData.
pub fn build_upload_wasm_tx_with_data(
    params: &UploadWasmParams,
    soroban_data: SorobanTransactionData,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;

    let wasm_bytes_m = BytesM::try_from(params.wasm_bytes.as_slice())
        .map_err(|_| DecodeError::Extraction("WASM bytes exceed max length".into()))?;

    let upload_op = InvokeHostFunctionOp {
        host_function: HostFunction::UploadContractWasm(wasm_bytes_m),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(upload_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds an initial `TransactionEnvelope` for creating a contract instance from an uploaded WASM.
/// This is used for simulation; the final transaction uses `build_create_contract_tx_with_data`.
pub fn build_create_contract_tx(params: &CreateContractParams) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContract(CreateContractArgs {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
        }),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V0,
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a final `TransactionEnvelope` for creating a contract instance with SorobanTransactionData.
pub fn build_create_contract_tx_with_data(
    params: &CreateContractParams,
    soroban_data: SorobanTransactionData,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContract(CreateContractArgs {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
        }),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a final `TransactionEnvelope` for uploading a WASM binary with SorobanTransactionData and auth entries.
pub fn build_upload_wasm_tx_with_data_and_auth(
    params: &UploadWasmParams,
    soroban_data: SorobanTransactionData,
    auth_entries: Vec<SorobanAuthorizationEntry>,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;

    let wasm_bytes_m = BytesM::try_from(params.wasm_bytes.as_slice())
        .map_err(|_| DecodeError::Extraction("WASM bytes exceed max length".into()))?;

    let auth_vec_m = VecM::try_from(auth_entries)
        .map_err(|_| DecodeError::Extraction("Too many auth entries".into()))?;

    let upload_op = InvokeHostFunctionOp {
        host_function: HostFunction::UploadContractWasm(wasm_bytes_m),
        auth: auth_vec_m,
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(upload_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a final `TransactionEnvelope` for creating a contract instance with SorobanTransactionData and auth entries.
pub fn build_create_contract_tx_with_data_and_auth(
    params: &CreateContractParams,
    soroban_data: SorobanTransactionData,
    auth_entries: Vec<SorobanAuthorizationEntry>,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let auth_vec_m = VecM::try_from(auth_entries)
        .map_err(|_| DecodeError::Extraction("Too many auth entries".into()))?;

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContract(CreateContractArgs {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
        }),
        auth: auth_vec_m,
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Parse base64-encoded ScVal strings into a VecM<ScVal>.
pub fn parse_scval_args(args: &[String]) -> Result<VecM<ScVal>, DecodeError> {
    let mut scval_args = Vec::new();
    for arg_b64 in args {
        let raw = STANDARD.decode(arg_b64)?;
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let val =
            ScVal::read_xdr(&mut l).map_err(|e| DecodeError::XdrParse("ScVal arg".into(), e))?;
        scval_args.push(val);
    }
    VecM::try_from(scval_args).map_err(|_| DecodeError::Extraction("Too many arguments".into()))
}

/// Builds an initial `TransactionEnvelope` for creating a contract instance with constructor arguments.
/// This is used for simulation; the final transaction uses `build_create_contract_v2_tx_with_data_and_auth`.
pub fn build_create_contract_v2_tx(params: &CreateContractV2Params) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let constructor_args = parse_scval_args(&params.constructor_args)?;

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContractV2(CreateContractArgsV2 {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
            constructor_args,
        }),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V0,
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a `TransactionEnvelope` for creating a contract instance with constructor arguments and SorobanTransactionData.
pub fn build_create_contract_v2_tx_with_data(
    params: &CreateContractV2Params,
    soroban_data: SorobanTransactionData,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let constructor_args = parse_scval_args(&params.constructor_args)?;

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContractV2(CreateContractArgsV2 {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
            constructor_args,
        }),
        auth: VecM::default(),
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Builds a final `TransactionEnvelope` for creating a contract instance with constructor arguments, SorobanTransactionData, and auth entries.
pub fn build_create_contract_v2_tx_with_data_and_auth(
    params: &CreateContractV2Params,
    soroban_data: SorobanTransactionData,
    auth_entries: Vec<SorobanAuthorizationEntry>,
) -> Result<String, DecodeError> {
    let source_account = decode_account_id(&params.source_account)?;
    let deployer = decode_account_id(&params.deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..params.salt.len()].copy_from_slice(&params.salt);

    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    let auth_vec_m = VecM::try_from(auth_entries)
        .map_err(|_| DecodeError::Extraction("Too many auth entries".into()))?;

    let constructor_args = parse_scval_args(&params.constructor_args)?;

    let create_op = InvokeHostFunctionOp {
        host_function: HostFunction::CreateContractV2(CreateContractArgsV2 {
            contract_id_preimage: preimage,
            executable: ContractExecutable::Wasm(Hash(params.wasm_hash)),
            constructor_args,
        }),
        auth: auth_vec_m,
    };

    let op = Operation {
        source_account: None,
        body: OperationBody::InvokeHostFunction(create_op),
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
        ext: TransactionExt::V1(soroban_data),
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;

    Ok(STANDARD.encode(&buf))
}

/// Parse base64-encoded SorobanAuthorizationEntry from simulation response.
pub fn parse_soroban_authorization_entry(
    base64_data: &str,
) -> Result<SorobanAuthorizationEntry, DecodeError> {
    let raw = STANDARD.decode(base64_data).map_err(|e| {
        DecodeError::Extraction(format!("Failed to decode SorobanAuthorizationEntry: {}", e))
    })?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    SorobanAuthorizationEntry::read_xdr(&mut l)
        .map_err(|e| DecodeError::XdrParse("SorobanAuthorizationEntry".into(), e))
}

/// Parse multiple base64-encoded SorobanAuthorizationEntry from simulation response.
pub fn parse_soroban_authorization_entries(
    auth_b64_list: &[String],
) -> Result<Vec<SorobanAuthorizationEntry>, DecodeError> {
    auth_b64_list
        .iter()
        .map(|s| parse_soroban_authorization_entry(s))
        .collect::<Result<Vec<_>, _>>()
}

/// Parse base64-encoded SorobanTransactionData from simulation response.
pub fn parse_soroban_transaction_data(
    base64_data: &str,
) -> Result<SorobanTransactionData, DecodeError> {
    let raw = STANDARD.decode(base64_data).map_err(|e| {
        DecodeError::Extraction(format!("Failed to decode SorobanTransactionData: {}", e))
    })?;
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    SorobanTransactionData::read_xdr(&mut l)
        .map_err(|e| DecodeError::XdrParse("SorobanTransactionData".into(), e))
}

/// Parameters for building an `ExtendFootprintTtl` transaction.
pub struct ExtendFootprintParams {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops
    pub fee: u32,
    /// Minimum TTL in ledgers (`extendTo`): the footprint entries will live at
    /// least this many ledgers past the last closed ledger. Relative, not an
    /// absolute ledger sequence — see `ExtendFootprintTTLOp` in
    /// `Stellar-transaction.x`.
    pub extend_to: u32,
    /// Ledger keys (base64 XDR or hex-encoded XDR) whose TTL will be extended.
    /// Placed in the read-only footprint of the simulation envelope.
    pub footprint_keys: Vec<String>,
}

/// Decode a `LedgerKey` from base64 XDR or hex-encoded XDR bytes.
pub fn decode_ledger_key(encoded: &str) -> Result<LedgerKey, DecodeError> {
    let trimmed = encoded.trim();
    if trimmed.is_empty() {
        return Err(DecodeError::Extraction("empty LedgerKey".into()));
    }
    let raw = if let Ok(bytes) = STANDARD.decode(trimmed) {
        bytes
    } else if trimmed.len().is_multiple_of(2) && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(trimmed).map_err(DecodeError::Hex)?
    } else {
        return Err(DecodeError::Extraction(
            "LedgerKey must be base64 XDR or even-length hex".into(),
        ));
    };
    if raw.is_empty() {
        return Err(DecodeError::Extraction("empty LedgerKey".into()));
    }
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerKey::read_xdr(&mut l).map_err(|e| DecodeError::XdrParse("LedgerKey".into(), e))
}

/// Deduplicate encoded LedgerKeys while preserving order. Invalid keys error.
pub fn merge_footprint_keys(keys: &[String]) -> Result<Vec<String>, DecodeError> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for k in keys {
        let decoded = decode_ledger_key(k)?;
        let mut buf = Vec::new();
        let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        decoded.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;
        let canonical = STANDARD.encode(&buf);
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
    }
    if out.is_empty() {
        return Err(DecodeError::Extraction(
            "footprint must contain at least one LedgerKey".into(),
        ));
    }
    Ok(out)
}

fn muxed_from_account(source_account: AccountId) -> MuxedAccount {
    MuxedAccount::Ed25519(match source_account.0 {
        PublicKey::PublicKeyTypeEd25519(u) => u,
    })
}

fn encode_envelope(envelope: TransactionEnvelope) -> Result<String, DecodeError> {
    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    envelope.write_xdr(&mut l).map_err(DecodeError::XdrWrite)?;
    Ok(STANDARD.encode(&buf))
}

fn empty_soroban_data_with_keys(keys: &[LedgerKey]) -> Result<SorobanTransactionData, DecodeError> {
    let read_only = VecM::try_from(keys.to_vec())
        .map_err(|_| DecodeError::Extraction("Too many footprint keys".into()))?;
    Ok(SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only,
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    })
}

fn build_extend_envelope(
    params: &ExtendFootprintParams,
    soroban_data: SorobanTransactionData,
) -> Result<String, DecodeError> {
    if params.extend_to == 0 {
        return Err(DecodeError::Extraction(
            "extend_to must be greater than 0".into(),
        ));
    }
    let source_account = decode_account_id(&params.source_account)?;
    let op = Operation {
        source_account: None,
        body: OperationBody::ExtendFootprintTtl(ExtendFootprintTtlOp {
            ext: ExtensionPoint::V0,
            extend_to: params.extend_to,
        }),
    };
    let tx = Transaction {
        source_account: muxed_from_account(source_account),
        fee: params.fee,
        seq_num: SequenceNumber(params.sequence),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: VecM::try_from(vec![op]).unwrap(),
        ext: TransactionExt::V1(soroban_data),
    };
    encode_envelope(TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    }))
}

/// Builds an initial `TransactionEnvelope` for `ExtendFootprintTtl` (simulation).
///
/// Uses `TransactionExt::V1` with a zero resource budget and the requested
/// read-only footprint so `simulateTransaction` can fill in real costs.
pub fn build_extend_footprint_tx(params: &ExtendFootprintParams) -> Result<String, DecodeError> {
    let merged = merge_footprint_keys(&params.footprint_keys)?;
    let decoded: Result<Vec<LedgerKey>, _> = merged.iter().map(|k| decode_ledger_key(k)).collect();
    let data = empty_soroban_data_with_keys(&decoded?)?;
    build_extend_envelope(params, data)
}

/// Builds the final `TransactionEnvelope` for `ExtendFootprintTtl` using the
/// `SorobanTransactionData` returned by simulation (authoritative footprint + fees).
pub fn build_extend_footprint_tx_with_data(
    params: &ExtendFootprintParams,
    soroban_data: SorobanTransactionData,
) -> Result<String, DecodeError> {
    build_extend_envelope(params, soroban_data)
}

/// Parameters for building a `RestoreFootprint` transaction.
#[derive(Debug, Clone)]
pub struct RestoreFootprintParams {
    /// Source account public key (G...)
    pub source_account: String,
    /// Next sequence number for the source account
    pub sequence: i64,
    /// Transaction fee in stroops (must be >= min_resource_fee from preamble)
    pub fee: u32,
    /// `SorobanTransactionData` from the restore preamble (authoritative footprint)
    pub soroban_data: SorobanTransactionData,
}

/// Builds a `TransactionEnvelope` for `RestoreFootprint`.
///
/// Uses the `SorobanTransactionData` from the restore preamble, which contains
/// the authoritative footprint and resource bounds needed to restore archived entries.
///
/// Rejects an empty footprint (nothing to restore) and a fee below the
/// preamble's `resource_fee`, which the network would refuse.
pub fn build_restore_footprint_tx(params: &RestoreFootprintParams) -> Result<String, DecodeError> {
    let footprint = &params.soroban_data.resources.footprint;
    if footprint.read_only.is_empty() && footprint.read_write.is_empty() {
        return Err(DecodeError::Extraction(
            "restore footprint is empty: no archived entries to restore".into(),
        ));
    }
    if i64::from(params.fee) < params.soroban_data.resource_fee {
        return Err(DecodeError::Extraction(format!(
            "restore fee {} is below the preamble resource fee {}",
            params.fee, params.soroban_data.resource_fee
        )));
    }
    let source_account = decode_account_id(&params.source_account)?;
    let op = Operation {
        source_account: None,
        body: OperationBody::RestoreFootprint(RestoreFootprintOp {
            ext: ExtensionPoint::V0,
        }),
    };
    let tx = Transaction {
        source_account: muxed_from_account(source_account),
        fee: params.fee,
        seq_num: SequenceNumber(params.sequence),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: VecM::try_from(vec![op]).unwrap(),
        ext: TransactionExt::V1(params.soroban_data.clone()),
    };
    encode_envelope(TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: VecM::default(),
    }))
}

/// Derives the contract ID from network ID, deployer address, and salt.
///
/// Uses Stellar's official formula:
/// 1. Build `HashIdPreimage::ContractId(HashIdPreimageContractId { network_id, contract_id_preimage })`
///    where `contract_id_preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress { address, salt })`.
/// 2. SHA-256 the XDR serialization of the full `HashIdPreimage`.
///
/// The `wasm_hash` parameter is not directly part of ID derivation but is retained
/// in the signature for interface consistency with the full `CreateContract` flow.
pub fn derive_contract_id(
    network_id: &[u8; 32],
    deployer_address: &str,
    salt: &[u8; 20],
    wasm_hash: &[u8; 32],
) -> Result<String, DecodeError> {
    let deployer = decode_account_id(deployer_address)?;

    // Pad 20-byte salt to 32 bytes for Uint256 (left-aligned, big-endian)
    let mut salt_bytes = [0u8; 32];
    salt_bytes[..salt.len()].copy_from_slice(salt);

    let contract_id_preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
            match deployer.0 {
                PublicKey::PublicKeyTypeEd25519(u) => u,
            },
        ))),
        salt: Uint256(salt_bytes),
    });

    // Official Stellar/Soroban: SHA-256(XDR(HashIdPreimage::ContractId))
    let full_preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(*network_id),
        contract_id_preimage,
    });

    let mut buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    full_preimage
        .write_xdr(&mut l)
        .map_err(DecodeError::XdrWrite)?;

    let hash = Sha256::digest(&buf);
    let _ = wasm_hash;
    Ok(hex::encode(hash))
}

#[cfg(test)]
fn realistic_soroban_transaction_data() -> SorobanTransactionData {
    use stellar_xdr::{
        ContractDataDurability, LedgerFootprint, LedgerKey, LedgerKeyContractData, ScVal,
        SorobanResources, SorobanTransactionDataExt,
    };

    let contract_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    });

    SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::try_from(vec![contract_key]).unwrap(),
                read_write: VecM::default(),
            },
            instructions: 100_000_000,
            disk_read_bytes: 100_000,
            write_bytes: 100_000,
        },
        resource_fee: 50_000,
    }
}

/// Create a test SorobanAuthorizationEntry with SourceAccount credentials.
#[cfg(test)]
fn test_auth_entry() -> SorobanAuthorizationEntry {
    use stellar_xdr::{SorobanAuthorizedFunction, SorobanAuthorizedInvocation, SorobanCredentials};

    SorobanAuthorizationEntry {
        credentials: SorobanCredentials::SourceAccount,
        root_invocation: SorobanAuthorizedInvocation {
            function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
                contract_address: ScAddress::Contract(ContractId(Hash([0; 32]))),
                function_name: ScSymbol::try_from("hello").unwrap(),
                args: VecM::default(),
            }),
            sub_invocations: VecM::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{OperationBody, SorobanCredentials};

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

    #[test]
    fn test_build_upload_wasm_tx_produces_valid_xdr() {
        let params = UploadWasmParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 1,
            fee: 100,
            wasm_bytes: vec![0x00, 0x61, 0x73, 0x6d], // Minimal WASM magic
        };

        let envelope = build_upload_wasm_tx(&params).unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify structure
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 100);
                assert_eq!(v1.tx.seq_num.0, 1);
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_create_contract_tx_produces_valid_xdr() {
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let params = CreateContractParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 2,
            fee: 100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
        };

        let envelope = build_create_contract_tx(&params).unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify structure
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 100);
                assert_eq!(v1.tx.seq_num.0, 2);
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_derive_contract_id_matches_stellar_formula() {
        // Deterministic test vector for Stellar contract ID derivation:
        // SHA-256(XDR(HashIdPreimage::ContractId(HashIdPreimageContractId {
        //     network_id, contract_id_preimage: ContractIdPreimage::Address(...)
        // })))
        //
        // Reference: soroban-env-host src/host/lifecycle.rs + src/test/lifecycle.rs
        //   let full_id_preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        //       network_id, contract_id_preimage });
        //   let contract_id = sha256_hash_id_preimage(full_id_preimage);
        //
        // Network: "Test SDF Network ; September 2015"
        // Source: GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF (all zeros)
        // Salt (20 bytes): 0x03 * 20, left-padded to 32 bytes
        //
        // Expected: e52f714abcb6555da317cb8d4265ffce88401c06e40e60c2d1f721445f683c3c
        let mut network_id = [0u8; 32];
        network_id.copy_from_slice(
            &hex::decode("cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472")
                .unwrap(),
        );

        let source = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let salt = [3u8; 20];
        let wasm_hash = [4u8; 32];

        let result = derive_contract_id(&network_id, source, &salt, &wasm_hash);
        assert!(result.is_ok());

        let contract_id = result.unwrap();
        assert_eq!(
            contract_id,
            "e52f714abcb6555da317cb8d4265ffce88401c06e40e60c2d1f721445f683c3c"
        );
    }

    #[test]
    fn test_derive_contract_id_different_networks_produce_different_ids() {
        // Same inputs but different network should produce different contract ID
        let mut testnet_network_id = [0u8; 32];
        testnet_network_id.copy_from_slice(
            &hex::decode("cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472")
                .unwrap(),
        );

        let mut mainnet_network_id = [0u8; 32];
        mainnet_network_id.copy_from_slice(
            &hex::decode("7ac33997544e3264d040315e569b3cd71dbbc6eb9a5aa6f2b7ecf1df4b49e5e1")
                .unwrap(),
        );

        let source = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let salt = [3u8; 20];
        let wasm_hash = [4u8; 32];

        let testnet_id =
            derive_contract_id(&testnet_network_id, source, &salt, &wasm_hash).unwrap();
        let mainnet_id =
            derive_contract_id(&mainnet_network_id, source, &salt, &wasm_hash).unwrap();

        assert_ne!(testnet_id, mainnet_id);
    }

    #[test]
    fn test_build_upload_wasm_tx_with_data() {
        let params = UploadWasmParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 1,
            fee: 50_100, // inclusion_fee + resource_fee
            wasm_bytes: vec![0x00, 0x61, 0x73, 0x6d],
        };

        let soroban_data = realistic_soroban_transaction_data();
        let envelope = build_upload_wasm_tx_with_data(&params, soroban_data).unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify it has V1 extension with realistic data
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 50_100);
                match v1.tx.ext {
                    TransactionExt::V1(data) => {
                        assert_eq!(data.resource_fee, 50_000);
                        assert_eq!(data.resources.instructions, 100_000_000);
                        assert_eq!(data.resources.disk_read_bytes, 100_000);
                        assert_eq!(data.resources.write_bytes, 100_000);
                        assert_eq!(data.resources.footprint.read_only.len(), 1);
                    }
                    _ => panic!("Expected V1 extension"),
                }
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_create_contract_tx_with_data() {
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let params = CreateContractParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 2,
            fee: 50_100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
        };

        let soroban_data = realistic_soroban_transaction_data();
        let envelope = build_create_contract_tx_with_data(&params, soroban_data).unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify it has V1 extension with realistic data
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 50_100);
                match v1.tx.ext {
                    TransactionExt::V1(data) => {
                        assert_eq!(data.resource_fee, 50_000);
                        assert_eq!(data.resources.instructions, 100_000_000);
                    }
                    _ => panic!("Expected V1 extension"),
                }
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_parse_soroban_transaction_data() {
        // Create a valid SorobanTransactionData and encode it
        let data = realistic_soroban_transaction_data();
        let mut buf = Vec::new();
        let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        data.write_xdr(&mut l).unwrap();
        let encoded = STANDARD.encode(&buf);

        // Parse it back
        let result = parse_soroban_transaction_data(&encoded);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.resource_fee, 50_000);
        assert_eq!(parsed.resources.instructions, 100_000_000);
        assert_eq!(parsed.resources.footprint.read_only.len(), 1);
    }

    #[test]
    fn test_build_upload_wasm_tx_with_auth() {
        let params = UploadWasmParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 1,
            fee: 50_100,
            wasm_bytes: vec![0x00, 0x61, 0x73, 0x6d],
        };

        let soroban_data = realistic_soroban_transaction_data();
        let auth_entries = vec![test_auth_entry()];
        let envelope =
            build_upload_wasm_tx_with_data_and_auth(&params, soroban_data, auth_entries).unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify it has V1 extension and auth entries
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => match v1.tx.ext {
                TransactionExt::V1(data) => {
                    assert_eq!(data.resource_fee, 50_000);
                    // Verify auth entries are present in the operation
                    let op = &v1.tx.operations[0];
                    match &op.body {
                        OperationBody::InvokeHostFunction(host_fn) => {
                            assert_eq!(host_fn.auth.len(), 1);
                            assert_eq!(
                                host_fn.auth[0].credentials,
                                SorobanCredentials::SourceAccount
                            );
                        }
                        _ => panic!("Expected InvokeHostFunction"),
                    }
                }
                _ => panic!("Expected V1 extension"),
            },
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_create_contract_tx_with_auth() {
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let params = CreateContractParams {
            source_account: TEST_SOURCE.to_string(),
            sequence: 2,
            fee: 50_100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
        };

        let soroban_data = realistic_soroban_transaction_data();
        let auth_entries = vec![test_auth_entry()];
        let envelope =
            build_create_contract_tx_with_data_and_auth(&params, soroban_data, auth_entries)
                .unwrap();
        assert!(!envelope.is_empty());

        // Parse back and verify it has V1 extension and auth entries
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => match v1.tx.ext {
                TransactionExt::V1(data) => {
                    assert_eq!(data.resource_fee, 50_000);
                    // Verify auth entries are present in the operation
                    let op = &v1.tx.operations[0];
                    match &op.body {
                        OperationBody::InvokeHostFunction(host_fn) => {
                            assert_eq!(host_fn.auth.len(), 1);
                            assert_eq!(
                                host_fn.auth[0].credentials,
                                SorobanCredentials::SourceAccount
                            );
                        }
                        _ => panic!("Expected InvokeHostFunction"),
                    }
                }
                _ => panic!("Expected V1 extension"),
            },
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_parse_invalid_auth_entry() {
        let result = parse_soroban_authorization_entry("invalid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_auth_list() {
        let auth_list: Vec<String> = vec![];
        let result = parse_soroban_authorization_entries(&auth_list);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_build_create_contract_v2_tx_produces_valid_xdr() {
        use crate::scval_to_base64;
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let arg_b64 = scval_to_base64(&ScVal::U32(42)).unwrap();
        let params = CreateContractV2Params {
            source_account: TEST_SOURCE.to_string(),
            sequence: 2,
            fee: 100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
            constructor_args: vec![arg_b64],
        };

        let envelope = build_create_contract_v2_tx(&params).unwrap();
        assert!(!envelope.is_empty());

        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.fee, 100);
                assert_eq!(v1.tx.seq_num.0, 2);
                let op = &v1.tx.operations[0];
                match &op.body {
                    OperationBody::InvokeHostFunction(host_fn) => match &host_fn.host_function {
                        HostFunction::CreateContractV2(args) => {
                            assert_eq!(args.constructor_args.len(), 1);
                            assert_eq!(args.constructor_args[0], ScVal::U32(42));
                            assert_eq!(args.executable, ContractExecutable::Wasm(Hash(wasm_hash)));
                        }
                        _ => panic!("Expected CreateContractV2 host function"),
                    },
                    _ => panic!("Expected InvokeHostFunction operation"),
                }
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_create_contract_v2_tx_with_data() {
        use crate::scval_to_base64;
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let arg_b64 = scval_to_base64(&ScVal::I32(-7)).unwrap();
        let params = CreateContractV2Params {
            source_account: TEST_SOURCE.to_string(),
            sequence: 3,
            fee: 100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
            constructor_args: vec![arg_b64],
        };

        let soroban_data = realistic_soroban_transaction_data();
        let envelope = build_create_contract_v2_tx_with_data(&params, soroban_data).unwrap();
        assert!(!envelope.is_empty());

        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => match v1.tx.ext {
                TransactionExt::V1(data) => {
                    assert_eq!(data.resource_fee, 50_000);
                    let op = &v1.tx.operations[0];
                    match &op.body {
                        OperationBody::InvokeHostFunction(host_fn) => {
                            match &host_fn.host_function {
                                HostFunction::CreateContractV2(args) => {
                                    assert_eq!(args.constructor_args.len(), 1);
                                    assert_eq!(args.constructor_args[0], ScVal::I32(-7));
                                }
                                _ => panic!("Expected CreateContractV2"),
                            }
                        }
                        _ => panic!("Expected InvokeHostFunction"),
                    }
                }
                _ => panic!("Expected V1 extension"),
            },
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_create_contract_v2_tx_with_auth() {
        use crate::scval_to_base64;
        let wasm_hash = [1u8; 32];
        let salt = [2u8; 20];

        let arg_b64 = scval_to_base64(&ScVal::U32(99)).unwrap();
        let params = CreateContractV2Params {
            source_account: TEST_SOURCE.to_string(),
            sequence: 4,
            fee: 50_100,
            wasm_hash,
            deployer_address: TEST_SOURCE.to_string(),
            salt,
            constructor_args: vec![arg_b64],
        };

        let soroban_data = realistic_soroban_transaction_data();
        let auth_entries = vec![test_auth_entry()];
        let envelope =
            build_create_contract_v2_tx_with_data_and_auth(&params, soroban_data, auth_entries)
                .unwrap();
        assert!(!envelope.is_empty());

        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();

        match env {
            TransactionEnvelope::Tx(v1) => {
                let op = &v1.tx.operations[0];
                match &op.body {
                    OperationBody::InvokeHostFunction(host_fn) => {
                        assert_eq!(host_fn.auth.len(), 1);
                        assert_eq!(
                            host_fn.auth[0].credentials,
                            SorobanCredentials::SourceAccount
                        );
                        match &host_fn.host_function {
                            HostFunction::CreateContractV2(args) => {
                                assert_eq!(args.constructor_args.len(), 1);
                                assert_eq!(args.constructor_args[0], ScVal::U32(99));
                            }
                            _ => panic!("Expected CreateContractV2"),
                        }
                    }
                    _ => panic!("Expected InvokeHostFunction"),
                }
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_contract_id_derivation_identical_between_v1_and_v2() {
        use crate::scval_to_base64;

        let network_id = [42u8; 32];
        let deployer = TEST_SOURCE;
        let salt = [7u8; 20];
        let wasm_hash = [99u8; 32];

        let v1_params = CreateContractParams {
            source_account: deployer.to_string(),
            sequence: 100,
            fee: 50_000,
            wasm_hash,
            deployer_address: deployer.to_string(),
            salt,
        };
        let v1_envelope = build_create_contract_tx(&v1_params).unwrap();

        let arg_b64 = scval_to_base64(&ScVal::U32(42)).unwrap();
        let v2_params = CreateContractV2Params {
            source_account: deployer.to_string(),
            sequence: 100,
            fee: 50_000,
            wasm_hash,
            deployer_address: deployer.to_string(),
            salt,
            constructor_args: vec![arg_b64],
        };
        let v2_envelope = build_create_contract_v2_tx(&v2_params).unwrap();

        let decode_env = |b64: &str| -> TransactionEnvelope {
            let raw = STANDARD.decode(b64).unwrap();
            let mut cursor = std::io::Cursor::new(&raw);
            let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
            TransactionEnvelope::read_xdr(&mut l).unwrap()
        };

        let v1_preimage = match decode_env(&v1_envelope) {
            TransactionEnvelope::Tx(v1) => match &v1.tx.operations[0].body {
                OperationBody::InvokeHostFunction(hf) => match &hf.host_function {
                    HostFunction::CreateContract(args) => args.contract_id_preimage.clone(),
                    _ => panic!("Expected CreateContract in V1"),
                },
                _ => panic!("Expected InvokeHostFunction"),
            },
            _ => panic!("Expected V1 envelope"),
        };

        let v2_preimage = match decode_env(&v2_envelope) {
            TransactionEnvelope::Tx(v1) => match &v1.tx.operations[0].body {
                OperationBody::InvokeHostFunction(hf) => match &hf.host_function {
                    HostFunction::CreateContractV2(args) => args.contract_id_preimage.clone(),
                    _ => panic!("Expected CreateContractV2 in V2"),
                },
                _ => panic!("Expected InvokeHostFunction"),
            },
            _ => panic!("Expected V1 envelope"),
        };

        assert_eq!(v1_preimage, v2_preimage);

        // Contract ID derivation is identical whether deploying via V1 or V2
        let id1 = derive_contract_id(&network_id, deployer, &salt, &wasm_hash).unwrap();
        let id2 = derive_contract_id(&network_id, deployer, &salt, &wasm_hash).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_auth_entry_create_contract_v2_host_fn() {
        use stellar_xdr::{
            SorobanAuthorizedFunction, SorobanAuthorizedInvocation, SorobanCredentials,
        };

        let deployer = decode_account_id(TEST_SOURCE).unwrap();
        let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
            address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                match deployer.0 {
                    PublicKey::PublicKeyTypeEd25519(u) => u,
                },
            ))),
            salt: Uint256([0; 32]),
        });

        let constructor_args = VecM::try_from(vec![ScVal::U32(100)]).unwrap();

        let auth_entry = SorobanAuthorizationEntry {
            credentials: SorobanCredentials::SourceAccount,
            root_invocation: SorobanAuthorizedInvocation {
                function: SorobanAuthorizedFunction::CreateContractV2HostFn(CreateContractArgsV2 {
                    contract_id_preimage: preimage,
                    executable: ContractExecutable::Wasm(Hash([1; 32])),
                    constructor_args,
                }),
                sub_invocations: VecM::default(),
            },
        };

        let mut buf = Vec::new();
        let mut l = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
        auth_entry.write_xdr(&mut l).unwrap();
        let b64 = STANDARD.encode(&buf);

        let parsed = parse_soroban_authorization_entry(&b64).unwrap();
        match parsed.root_invocation.function {
            SorobanAuthorizedFunction::CreateContractV2HostFn(args) => {
                assert_eq!(args.executable, ContractExecutable::Wasm(Hash([1; 32])));
                assert_eq!(args.constructor_args.len(), 1);
                assert_eq!(args.constructor_args[0], ScVal::U32(100));
            }
            _ => panic!("Expected CreateContractV2HostFn"),
        }
    }
}

#[cfg(test)]
mod extract_tests {
    use crate::extract_contract_data_value;
    use base64::Engine;
    use stellar_xdr::{
        ContractDataDurability, ContractDataEntry, ContractId, ExtensionPoint, Hash, LedgerEntry,
        LedgerEntryData, LedgerEntryExt, Limited, Limits, ScAddress, ScVal, WriteXdr,
    };

    #[test]
    fn test_extract_contract_data_value_returns_val() {
        let entry = LedgerEntry {
            last_modified_ledger_seq: 1,
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
                key: ScVal::U32(1),
                durability: ContractDataDurability::Persistent,
                val: ScVal::U32(42),
            }),
            ext: LedgerEntryExt::V0,
        };
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        entry.write_xdr(&mut l).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);

        let cd = extract_contract_data_value(&b64).unwrap();
        assert!(matches!(cd.val, ScVal::U32(42)));
        assert_eq!(cd.key, ScVal::U32(1));
        assert_eq!(cd.durability, ContractDataDurability::Persistent);
    }

    #[test]
    fn test_extract_contract_data_value_rejects_non_contract_data() {
        let result = extract_contract_data_value("AAAABAAAAAE=");
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod extend_tests {
    use super::*;
    use crate::builder::{
        build_extend_footprint_tx, build_extend_footprint_tx_with_data,
        realistic_soroban_transaction_data, ExtendFootprintParams,
    };
    use stellar_xdr::{
        ContractDataDurability, Hash, LedgerKey, LedgerKeyContractData, Limited, Limits,
        OperationBody, ScAddress, ScVal, TransactionEnvelope, TransactionExt, WriteXdr,
    };

    #[test]
    fn test_build_extend_footprint_tx_round_trip() {
        let k = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        k.write_xdr(&mut l).unwrap();
        let key = STANDARD.encode(&buf);

        let params = ExtendFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 7,
            fee: 100,
            extend_to: 100_000,
            footprint_keys: vec![key],
        };
        let envelope = build_extend_footprint_tx(&params).unwrap();
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();
        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.seq_num.0, 7);
                match &v1.tx.operations[0].body {
                    OperationBody::ExtendFootprintTtl(op) => {
                        assert_eq!(op.extend_to, 100_000);
                    }
                    other => panic!("expected ExtendFootprintTtl, got {other:?}"),
                }
                match v1.tx.ext {
                    TransactionExt::V1(data) => {
                        assert_eq!(data.resources.footprint.read_only.len(), 1);
                        assert!(data.resources.footprint.read_write.is_empty());
                    }
                    other => panic!("expected V1 extension, got {other:?}"),
                }
            }
            _ => panic!("Expected V1 envelope"),
        }
    }

    #[test]
    fn test_build_extend_footprint_tx_rejects_zero_ledgers() {
        let k = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        k.write_xdr(&mut l).unwrap();
        let key = STANDARD.encode(&buf);

        let params = ExtendFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 1,
            fee: 100,
            extend_to: 0,
            footprint_keys: vec![key],
        };
        let err = build_extend_footprint_tx(&params).unwrap_err();
        assert!(err.to_string().contains("greater than 0"));
    }

    #[test]
    fn test_build_extend_footprint_tx_with_data_preserves_simulation_resources() {
        let k = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        k.write_xdr(&mut l).unwrap();
        let key = STANDARD.encode(&buf);

        let params = ExtendFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 2,
            fee: 100,
            extend_to: 50_000,
            footprint_keys: vec![key],
        };
        let data = realistic_soroban_transaction_data();
        let envelope = build_extend_footprint_tx_with_data(&params, data).unwrap();
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();
        match env {
            TransactionEnvelope::Tx(v1) => match v1.tx.ext {
                TransactionExt::V1(data) => {
                    assert_eq!(data.resource_fee, 50_000);
                    assert_eq!(data.resources.instructions, 100_000_000);
                }
                _ => panic!("Expected V1 extension"),
            },
            _ => panic!("Expected V1 envelope"),
        }
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::builder::{build_restore_footprint_tx, RestoreFootprintParams};
    use stellar_xdr::{
        ContractDataDurability, Hash, LedgerFootprint, LedgerKey, LedgerKeyContractData, Limited,
        Limits, OperationBody, ScAddress, ScVal, SorobanResources, SorobanTransactionData,
        SorobanTransactionDataExt, TransactionEnvelope, TransactionExt, VecM,
    };

    fn create_test_soroban_data() -> SorobanTransactionData {
        let k = LedgerKey::ContractData(LedgerKeyContractData {
            contract: ScAddress::Contract(ContractId(Hash([1; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
        });
        SorobanTransactionData {
            ext: SorobanTransactionDataExt::V0,
            resources: SorobanResources {
                footprint: LedgerFootprint {
                    read_only: VecM::try_from(vec![k]).unwrap(),
                    read_write: VecM::default(),
                },
                instructions: 1000,
                disk_read_bytes: 500,
                write_bytes: 200,
            },
            resource_fee: 10000,
        }
    }

    #[test]
    fn test_build_restore_footprint_tx_round_trip() {
        let soroban_data = create_test_soroban_data();
        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 10,
            fee: 10_200,
            soroban_data,
        };
        let envelope = build_restore_footprint_tx(&params).unwrap();
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();
        match env {
            TransactionEnvelope::Tx(v1) => {
                assert_eq!(v1.tx.seq_num.0, 10);
                assert_eq!(v1.tx.fee, 10_200);
                match &v1.tx.operations[0].body {
                    OperationBody::RestoreFootprint(_) => {
                        // RestoreFootprintOp only has ext field (V0)
                    }
                    other => panic!("expected RestoreFootprint, got {other:?}"),
                }
                // Verify the SorobanTransactionData from preamble is preserved
                match &v1.tx.ext {
                    TransactionExt::V1(soroban_data) => {
                        assert_eq!(soroban_data.resource_fee, 10000);
                        assert_eq!(soroban_data.resources.instructions, 1000);
                        assert!(!soroban_data.resources.footprint.read_only.is_empty());
                    }
                    other => panic!("expected V1 extension, got {other:?}"),
                }
            }
            other => panic!("expected v1 envelope, got {other:?}"),
        }
    }

    #[test]
    fn test_build_restore_footprint_tx_preserves_preamble_footprint() {
        let soroban_data = create_test_soroban_data();
        let original_footprint_keys = soroban_data.resources.footprint.read_only.clone();

        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 11,
            fee: 10_300,
            soroban_data,
        };
        let envelope = build_restore_footprint_tx(&params).unwrap();
        let raw = STANDARD.decode(&envelope).unwrap();
        let mut cursor = std::io::Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let env = TransactionEnvelope::read_xdr(&mut l).unwrap();
        match env {
            TransactionEnvelope::Tx(v1) => match &v1.tx.ext {
                TransactionExt::V1(soroban_data) => {
                    assert_eq!(
                        soroban_data.resources.footprint.read_only,
                        original_footprint_keys
                    );
                }
                other => panic!("expected V1 extension, got {other:?}"),
            },
            other => panic!("expected v1 envelope, got {other:?}"),
        }
    }

    #[test]
    fn test_build_restore_footprint_tx_fee_floor_enforcement() {
        let soroban_data = create_test_soroban_data();
        let min_fee = soroban_data.resource_fee as u32;

        // Test with fee exactly at minimum
        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 12,
            fee: min_fee,
            soroban_data: soroban_data.clone(),
        };
        assert!(build_restore_footprint_tx(&params).is_ok());

        // Test with fee above minimum
        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 13,
            fee: min_fee + 100,
            soroban_data: soroban_data.clone(),
        };
        assert!(build_restore_footprint_tx(&params).is_ok());

        // A fee below the preamble resource fee is rejected, not submitted.
        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 14,
            fee: min_fee - 1,
            soroban_data,
        };
        let err = build_restore_footprint_tx(&params).unwrap_err();
        assert!(err.to_string().contains("below the preamble resource fee"));
    }

    #[test]
    fn test_build_restore_footprint_tx_rejects_empty_footprint() {
        let mut soroban_data = create_test_soroban_data();
        soroban_data.resources.footprint = LedgerFootprint {
            read_only: VecM::default(),
            read_write: VecM::default(),
        };
        let params = RestoreFootprintParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
            sequence: 15,
            fee: 20_000,
            soroban_data,
        };
        let err = build_restore_footprint_tx(&params).unwrap_err();
        assert!(err.to_string().contains("footprint is empty"));
    }
}
