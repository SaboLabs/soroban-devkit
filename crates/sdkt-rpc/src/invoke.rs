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
    /// Transaction hash on the network.
    pub hash: String,
    /// "SUCCESS" | "FAILED" | "PENDING"
    pub status: String,
    pub contract_id: String,
    pub function: String,
    /// Total fee (inclusion + resource) actually submitted, in stroops.
    pub fee: u32,
    /// Base64 `TransactionResult` XDR from the settled transaction, if any.
    pub result_xdr: Option<String>,
    /// Error code from the network when status == FAILED.
    pub error_code: Option<String>,
    /// Base64 `TransactionResult` XDR of the error when status == FAILED.
    pub error_result_xdr: Option<String>,
    /// Diagnostic events (base64 XDR) when status == FAILED.
    pub diagnostic_events: Vec<String>,
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

/// Base inclusion fee added on top of the resource fee reported by simulation.
///
/// The resource fee dominates for Soroban transactions; this is only the
/// per-operation inclusion component.
pub const INCLUSION_FEE: u32 = 100;

/// What a simulation pass tells us about an invocation.
///
/// Produced by [`simulate_invoke`] and consumed both by [`invoke_contract`],
/// which goes on to sign and submit, and by `sdkt tx build`, which stops at the
/// unsigned envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedInvoke {
    /// Authoritative footprint and resources returned by the network.
    pub soroban_data: SorobanTransactionData,
    /// Authorization entries returned by simulation, if any.
    pub auth_entries: Vec<SorobanAuthorizationEntry>,
    /// Resource fee reported by simulation, in stroops.
    pub min_resource_fee: u32,
    /// [`INCLUSION_FEE`] + [`Self::min_resource_fee`], saturating.
    pub total_fee: u32,
}

/// Simulate an invocation and adopt what the network reports: footprint, auth
/// entries and resource fee.
///
/// `params.sequence` is used as given — callers that need the account's current
/// sequence should fetch it first (see [`get_next_sequence`]).
///
/// # Errors
///
/// Returns [`RpcError::Rpc`] if the envelope cannot be built, the simulation
/// call fails, the network reports a simulation error, or the response carries
/// no `SorobanTransactionData`.
pub async fn simulate_invoke(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
) -> Result<SimulatedInvoke, RpcError> {
    // Simulate with an empty V1 SorobanData (network requires ext V1).
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

    // Auth entries returned by simulation (e.g. for contract-authorized calls).
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

/// Build an empty `SorobanTransactionData` (ext V0) for the simulation pass.
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

/// Invoke a contract function end-to-end: fetch sequence, simulate for the
/// authoritative footprint/auth/fees, build the final envelope, sign, submit,
/// and optionally poll until the transaction settles.
///
/// `params.args` are base64-encoded `ScVal` strings (the format produced by
/// the CLI typed-argument parser).
pub async fn invoke_contract(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
    poll: &PollConfig,
    wait: bool,
) -> Result<InvokeResult, RpcError> {
    // 1. Current sequence for the source account.
    let sequence = get_next_sequence(client, &params.source_account).await?;
    let sim_params = InvokeTransactionParams {
        sequence,
        ..params.clone()
    };

    // 2-4. Simulate, then adopt the authoritative footprint, auth entries and
    // resource fee the network reports.
    let simulated = simulate_invoke(client, &sim_params).await?;
    let total_fee = simulated.total_fee;

    // 5. Final envelope with real fees, then sign.
    let final_params = InvokeTransactionParams {
        fee: total_fee,
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

    // 6. Submit and optionally poll until settled.
    let submission = submit_and_wait(client, &signed_envelope, wait, poll).await?;

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
        fee: total_fee,
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
}
