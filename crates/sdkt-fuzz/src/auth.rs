//! Explicit authorization model.
//!
//! Auth is exercised through the host's real enforcement path:
//! `e2e_invoke::invoke_host_function` installs supplied auth entries via
//! `Host::set_authorization_entries`, which builds an *enforcing*
//! `AuthorizationManager`. Three modes:
//!
//! - [`AuthMode::NoAuth`] — no entries. A host function that requires
//!   authorization must fail with an `Auth`-typed error.
//! - [`AuthMode::CorrectAuth`] — a `SourceAccount` credential entry whose
//!   root invocation matches the executed host function. Source-account
//!   credentials authenticate without a signature check (host behavior for
//!   the transaction source), so this is a valid required authorization.
//! - [`AuthMode::WrongAuth`] — an `AddressV2` entry for an account that is
//!   *not* the source account, carrying a fixed all-zero signature. Host-side
//!   signature verification (or address matching) must fail.
//!
//! Scope honesty: single-signer, single-entry auth — source-account vs
//! address credentials. This is **not** multi-signer or auth-tree fuzzing,
//! and no claim is made about either. The host does not expose an
//! authorization trace through its public API, so expectations are expressed
//! only as declared oracle outcomes.

use soroban_env_host::xdr::{
    AccountId, CreateContractArgsV2, Hash, InvokeContractArgs, PublicKey, ScAddress, ScBytes,
    ScVal, SorobanAddressCredentials, SorobanAuthorizationEntry, SorobanAuthorizedFunction,
    SorobanAuthorizedInvocation, SorobanCredentials, StringM, Uint256, VecM, WriteXdr,
};

use crate::case::FunctionCall;
use crate::error::FuzzError;
use soroban_env_host::xdr::{ContractExecutable, ContractIdPreimage, Limits};

/// How a case authenticates its invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// No authorization entries; the host enforces against an empty set.
    NoAuth,
    /// Valid source-account authorization for this exact call.
    CorrectAuth,
    /// An authorization entry that cannot authenticate the call.
    WrongAuth,
}

impl AuthMode {
    /// Inverse of [`AuthMode::name`] for artifact/CLI round-tripping.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "no_auth" => Some(AuthMode::NoAuth),
            "correct_auth" => Some(AuthMode::CorrectAuth),
            "wrong_auth" => Some(AuthMode::WrongAuth),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AuthMode::NoAuth => "no_auth",
            AuthMode::CorrectAuth => "correct_auth",
            AuthMode::WrongAuth => "wrong_auth",
        }
    }
}

/// The ed25519 account the host's `source_account` is set to for every
/// campaign execution. Shared with the executor's deployer identity.
pub const AUTH_ACCOUNT: [u8; 32] = [123; 32];
/// A second account, never the source: used for wrong-authorizations.
pub const FOREIGN_ACCOUNT: [u8; 32] = [77; 32];

fn account_id(pk: [u8; 32]) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(pk)))
}

/// The source account (also the transaction source for every case).
pub fn source_account() -> AccountId {
    account_id(AUTH_ACCOUNT)
}

/// `ScAddress` of the source account — the address a contract must
/// `require_auth` for these entries to authorize it.
pub fn source_address() -> ScAddress {
    ScAddress::Account(source_account())
}

/// `ScAddress` of a non-source account — no entry can authenticate calls
/// requiring it under [`AuthMode::CorrectAuth`].
pub fn foreign_address() -> ScAddress {
    ScAddress::Account(account_id(FOREIGN_ACCOUNT))
}

/// Encode one auth entry as XDR for the host invocation path.
fn encode(entry: &SorobanAuthorizationEntry) -> Result<Vec<u8>, FuzzError> {
    entry
        .to_xdr(Limits::none())
        .map_err(|e| FuzzError::Execution(format!("auth entry encode: {e}")))
}

/// Build the auth entries for an `InvokeContract` case.
pub fn invoke_auth_entries(
    mode: AuthMode,
    contract: &ScAddress,
    call: &FunctionCall,
) -> Result<Vec<Vec<u8>>, FuzzError> {
    match mode {
        AuthMode::NoAuth => Ok(Vec::new()),
        AuthMode::CorrectAuth => {
            let root = invoke_root_invocation(contract, call)?;
            Ok(vec![encode(&source_account_entry(root))?])
        }
        AuthMode::WrongAuth => {
            let root = invoke_root_invocation(contract, call)?;
            Ok(vec![encode(&foreign_address_entry(root))?])
        }
    }
}

/// Build the auth entries for a `CreateContract` host function
/// (used by the authorization probe).
pub fn create_contract_auth_entries(
    mode: AuthMode,
    preimage: &ContractIdPreimage,
    wasm_hash: &Hash,
) -> Result<Vec<Vec<u8>>, FuzzError> {
    let executable = ContractExecutable::Wasm(wasm_hash.clone());
    let root = SorobanAuthorizedInvocation {
        function: SorobanAuthorizedFunction::CreateContractV2HostFn(CreateContractArgsV2 {
            contract_id_preimage: preimage.clone(),
            executable,
            constructor_args: VecM::default(),
        }),
        sub_invocations: VecM::default(),
    };
    match mode {
        AuthMode::NoAuth => Ok(Vec::new()),
        AuthMode::CorrectAuth => Ok(vec![encode(&source_account_entry(root))?]),
        AuthMode::WrongAuth => Ok(vec![encode(&foreign_address_entry(root))?]),
    }
}

fn invoke_root_invocation(
    contract: &ScAddress,
    call: &FunctionCall,
) -> Result<SorobanAuthorizedInvocation, FuzzError> {
    let function_name = StringM::try_from(call.function.as_str())
        .map_err(|_| FuzzError::Execution(format!("invalid ScSymbol: {}", call.function)))?;
    let args =
        VecM::try_from(call.args.clone()).map_err(|e| FuzzError::Execution(e.to_string()))?;
    Ok(SorobanAuthorizedInvocation {
        function: SorobanAuthorizedFunction::ContractFn(InvokeContractArgs {
            contract_address: contract.clone(),
            function_name: function_name.into(),
            args,
        }),
        sub_invocations: VecM::default(),
    })
}

/// A `SourceAccount`-credential entry: valid for the transaction source.
fn source_account_entry(root: SorobanAuthorizedInvocation) -> SorobanAuthorizationEntry {
    SorobanAuthorizationEntry {
        credentials: SorobanCredentials::SourceAccount,
        root_invocation: root,
    }
}

/// An `AddressV2`-credential entry that cannot authenticate: the address is
/// not the transaction source, and the signature is a fixed all-zero blob
/// that will not verify for the authorization payload.
fn foreign_address_entry(root: SorobanAuthorizedInvocation) -> SorobanAuthorizationEntry {
    SorobanAuthorizationEntry {
        credentials: SorobanCredentials::AddressV2(SorobanAddressCredentials {
            address: foreign_address(),
            nonce: 1,
            signature_expiration_ledger: u32::MAX,
            signature: ScVal::Bytes(ScBytes(
                soroban_env_host::xdr::BytesM::try_from(vec![0u8; 64])
                    .expect("64 bytes fit ScBytes limits"),
            )),
        }),
        root_invocation: root,
    }
}

/// The contract-code hash a deployment would install (for probe footprints).
pub fn wasm_hash_entry_key(wasm_hash: &Hash) -> soroban_env_host::xdr::LedgerKey {
    soroban_env_host::xdr::LedgerKey::ContractCode(soroban_env_host::xdr::LedgerKeyContractCode {
        hash: wasm_hash.clone(),
    })
}
