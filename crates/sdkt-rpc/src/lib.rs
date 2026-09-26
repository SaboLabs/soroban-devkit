//! RPC module handles network requests.
//!
//! Exposes clients and methods to query the Soroban RPC endpoint for contract
//! inspection, XDR retrieval, and storage proofs.

pub mod account;
pub mod client;
pub mod deploy;
pub mod error;
pub mod events;
pub mod fee;
pub mod inspect;
pub mod invoke;
pub mod simulate;
pub mod storage;
pub mod submission;
pub mod transaction;
pub mod wasm;

pub use account::{
    get_next_sequence, inspect_account, AccountBalance, AccountInspection, AccountSigner,
};
pub use client::SorobanRpcClient;
pub use client::{fund_account, FundResult};
pub use deploy::{
    create_contract, deploy_contract, deploy_contract_with_args, format_json, format_pretty,
    CreateContractArgs, DeployOutcome, DeployResult, PartialDeployResult,
};
pub use error::RpcError;
pub use events::{
    get_contract_events, resolve_ledger_range, ContractEvent, EventFilter, GetEventsRequest,
};
pub use fee::{estimate_dynamic_fee, get_fee_stats, FeeDistribution, FeeStats};
pub use inspect::{inspect_contract, ContractInspection, StorageKeyInfo, TtlInfoSummary};
pub use invoke::{
    build_invoke_envelope, invoke_contract, simulate_invoke, InvokeBuildResult, InvokeResult,
    SimulatedInvoke, INCLUSION_FEE,
};
pub use simulate::{
    simulate_transaction, validate_envelope, SimulateCost, SimulateOperationResult,
    SimulateResponse, SimulateTransactionRequest,
};
pub use storage::{
    calculate_extension_cost, collect_extend_keys, extend_footprint, get_ttl_info,
    read_contract_state, read_ledger_entry, ExtendResult, StateReadResult, TtlEntry, TtlInfo,
};
pub use submission::{
    get_transaction_status, poll_transaction, send_transaction, submit_and_wait, PollConfig,
    SendTransactionRequest, SendTransactionResponse, SubmissionResult, TransactionStatus,
    TransactionStatusResponse,
};
pub use transaction::{inspect_transaction, TransactionInspection};
pub use wasm::get_wasm_metadata;
