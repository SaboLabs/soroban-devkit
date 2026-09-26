//! End-to-end state-changing contract invocation: `sdkt invoke`.
//!
//! Drives the full lifecycle using the existing infrastructure:
//! sequence fetch ([`get_next_sequence`]) → simulation
//! ([`simulate_transaction`]) → final envelope build
//! ([`sdkt_xdr::build_invoke_transaction_with_data`]) → signing
//! ([`sdkt_xdr::sign_transaction`]) → submission + polling
//! ([`submit_and_wait`]).

use crate::account::get_next_sequence;
use crate::error::RpcError;
use crate::simulate::simulate_transaction;
use crate::submission::{submit_and_wait, PollConfig, TransactionStatus};
use crate::SorobanRpcClient;
use sdkt_xdr::sign::{Ed25519Signer, Network, SigningOptions};
use sdkt_xdr::sign_transaction;
use sdkt_xdr::InvokeTransactionParams;
use stellar_xdr::{
    LedgerFootprint, SorobanAuthorizationEntry, SorobanResources, SorobanTransactionData,
    SorobanTransactionDataExt, VecM,
};

/// Final result of a state-changing contract invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct InvokeResult {
    pub hash: String,
    pub status: String,
    pub contract_id: String,
    pub function: String,
    pub fee: u32,
    pub result_xdr: Option<String>,
    pub error_code: Option<String>,
    pub error_result_xdr: Option<String>,
    pub diagnostic_events: Vec<String>,
}

/// Result of preparing, but not submitting, an invocation envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct InvokeBuildResult {
    pub envelope_xdr: String,
    pub fee: u32,
    pub sequence: i64,
    pub contract_id: String,
    pub function: String,
}

struct PreparedInvoke {
    signed_envelope: String,
    fee: u32,
    sequence: i64,
}

fn parse_min_resource_fee(raw: &str) -> Result<u32, RpcError> {
    let parsed: u64 = raw.parse().map_err(|_| {
        RpcError::Rpc(format!(
            "simulation returned invalid min_resource_fee: {raw:?}"
        ))
    })?;
    u32::try_from(parsed)
        .map_err(|_| RpcError::Rpc(format!("simulation min_resource_fee overflowed u32: {raw}")))
}

pub const INCLUSION_FEE: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedInvoke {
    pub soroban_data: SorobanTransactionData,
    pub auth_entries: Vec<SorobanAuthorizationEntry>,
    pub min_resource_fee: u32,
    pub total_fee: u32,
}

/// Simulate an invocation and adopt the footprint, auth entries and fee returned by RPC.
pub async fn simulate_invoke(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
) -> Result<SimulatedInvoke, RpcError> {
    let sim_envelope =
        sdkt_xdr::build_invoke_transaction_with_data(params, empty_soroban_data(), Vec::new())
            .map_err(|e| RpcError::Rpc(format!("Failed to build invoke transaction: {e}")))?;
    let simulation = simulate_transaction(client, &sim_envelope)
        .await
        .map_err(|e| RpcError::Rpc(format!("Invoke simulation failed: {e}")))?;

    if let Some(err) = &simulation.error {
        return Err(RpcError::Rpc(format!("Invoke simulation error: {err}")));
    }
    if simulation.transaction_data.is_empty() {
        return Err(RpcError::Rpc(
            "Simulation did not return SorobanTransactionData".into(),
        ));
    }

    let soroban_data = sdkt_xdr::parse_soroban_transaction_data(&simulation.transaction_data)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse SorobanTransactionData: {e}")))?;
    let min_resource_fee = parse_min_resource_fee(&simulation.min_resource_fee)?;
    let auth_entries = if simulation.results.is_empty() {
        Vec::new()
    } else {
        sdkt_xdr::builder::parse_soroban_authorization_entries(&simulation.results[0].auth)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse auth entries: {e}")))?
    };

    Ok(SimulatedInvoke {
        soroban_data,
        auth_entries,
        min_resource_fee,
        total_fee: INCLUSION_FEE.saturating_add(min_resource_fee),
    })
}

fn empty_soroban_data() -> SorobanTransactionData {
    SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    }
}

async fn prepare_invoke_envelope(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
) -> Result<PreparedInvoke, RpcError> {
    let sequence = get_next_sequence(client, &params.source_account).await?;
    let sim_params = InvokeTransactionParams {
        sequence,
        ..params.clone()
    };
    let simulated = simulate_invoke(client, &sim_params).await?;
    let final_params = InvokeTransactionParams {
        fee: simulated.total_fee,
        ..sim_params
    };
    let final_envelope = sdkt_xdr::build_invoke_transaction_with_data(
        &final_params,
        simulated.soroban_data,
        simulated.auth_entries,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to build final invoke transaction: {e}")))?;
    let signing_opts = SigningOptions::with(network);
    let signed_envelope = sign_transaction(&final_envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign invoke transaction: {e}")))?;

    Ok(PreparedInvoke {
        signed_envelope,
        fee: final_params.fee,
        sequence,
    })
}

/// Build and sign an invocation without submitting it.
pub async fn build_invoke_envelope(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
) -> Result<InvokeBuildResult, RpcError> {
    let prepared = prepare_invoke_envelope(client, params, signer, network).await?;
    Ok(InvokeBuildResult {
        envelope_xdr: prepared.signed_envelope,
        fee: prepared.fee,
        sequence: prepared.sequence,
        contract_id: params.contract_id.clone(),
        function: params.function.clone(),
    })
}

/// Invoke a contract function, submitting and optionally polling for settlement.
pub async fn invoke_contract(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
    poll: &PollConfig,
    wait: bool,
) -> Result<InvokeResult, RpcError> {
    let prepared = prepare_invoke_envelope(client, params, signer, network).await?;
    let submission = submit_and_wait(client, &prepared.signed_envelope, wait, poll).await?;
    let status = match submission.status {
        TransactionStatus::Success => "SUCCESS",
        TransactionStatus::Failed => "FAILED",
        _ => "PENDING",
    };

    Ok(InvokeResult {
        hash: submission.hash,
        status: status.to_string(),
        contract_id: params.contract_id.clone(),
        function: params.function.clone(),
        fee: prepared.fee,
        result_xdr: submission.result_xdr,
        error_code: submission.error_code,
        error_result_xdr: submission.error_result_xdr,
        diagnostic_events: submission.diagnostic_events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_min_resource_fee_accepts_valid() {
        assert_eq!(parse_min_resource_fee("150").unwrap(), 150);
    }

    #[test]
    fn parse_min_resource_fee_rejects_garbage() {
        assert!(parse_min_resource_fee("abc").is_err());
    }

    #[test]
    fn invoke_result_fields_roundtrip() {
        let r = InvokeResult {
            hash: "h".into(),
            status: "SUCCESS".into(),
            contract_id: "C".into(),
            function: "f".into(),
            fee: 250,
            result_xdr: Some("xdr".into()),
            error_code: None,
            error_result_xdr: None,
            diagnostic_events: Vec::new(),
        };
        assert_eq!(r.status, "SUCCESS");
        assert_eq!(r.fee, 250);
    }

    #[test]
    fn invoke_build_result_fields_roundtrip() {
        let r = InvokeBuildResult {
            envelope_xdr: "AAAAAgAAA...".into(),
            fee: 250,
            sequence: 42,
            contract_id: "C...".into(),
            function: "increment".into(),
        };
        assert_eq!(r.fee, 250);
        assert_eq!(r.sequence, 42);
        assert_eq!(r.function, "increment");
        assert!(r.envelope_xdr.starts_with("AAAA"));
    }
}
