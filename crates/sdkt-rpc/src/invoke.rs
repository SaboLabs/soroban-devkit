//! End-to-end state-changing contract invocation: `sdkt invoke`.
//!
//! Drives the full lifecycle using the existing infrastructure:
//! sequence fetch ([`get_next_sequence`]) → simulation
//! ([`simulate_transaction`]) → final envelope build
//! ([`sdkt_xdr::build_invoke_transaction_with_data`]) → signing
//! ([`sdkt_xdr::sign_transaction`]) → submission + polling
//! ([`submit_and_wait`]).
//!
//! [`build_invoke_envelope`] runs the same preparation stages and stops before
//! submission, backing `sdkt invoke --build-only`.

use crate::account::get_next_sequence;
use crate::error::RpcError;
use crate::simulate::simulate_transaction;
use crate::submission::{submit_and_wait, PollConfig, TransactionStatus};
use crate::SorobanRpcClient;
use sdkt_xdr::sign::{Ed25519Signer, Network, SigningOptions};
use sdkt_xdr::sign_transaction;
use sdkt_xdr::InvokeTransactionParams;
use stellar_xdr::{
    LedgerFootprint, SorobanResources, SorobanTransactionData, SorobanTransactionDataExt, VecM,
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

/// Result of preparing — but deliberately not submitting — an invocation
/// envelope: the output of `sdkt invoke --build-only`.
///
/// `envelope_xdr` is the signed base64 `TransactionEnvelope` produced by the
/// exact same sequence → simulate → build → sign pipeline that
/// [`invoke_contract`] would have submitted, so it is byte-for-byte usable
/// with `sdkt tx submit --envelope <xdr>`.
#[derive(Debug, Clone, PartialEq)]
pub struct InvokeBuildResult {
    /// Signed base64 `TransactionEnvelope` XDR, ready to submit.
    pub envelope_xdr: String,
    /// Total fee (inclusion + simulated resource fee) actually built into it, in stroops.
    pub fee: u32,
    /// Source-account sequence number the envelope was built with.
    pub sequence: i64,
    pub contract_id: String,
    pub function: String,
}

/// Prepared-but-unsigned-to-the-wire state shared by [`invoke_contract`] and
/// [`build_invoke_envelope`].
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

/// Run the preparation stages of an invocation — sequence fetch, simulation,
/// authoritative footprint/auth/fee adoption, envelope build and signing — and
/// stop before submission.
///
/// Every failure mode matches the submit path because the submit path *is*
/// this function followed by [`submit_and_wait`].
async fn prepare_invoke_envelope(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
) -> Result<PreparedInvoke, RpcError> {
    // 1. Current sequence for the source account.
    let sequence = get_next_sequence(client, &params.source_account).await?;
    let sim_params = InvokeTransactionParams {
        sequence,
        ..params.clone()
    };

    // 2. Simulate with an empty V1 SorobanData (network requires ext V1).
    let sim_envelope =
        sdkt_xdr::build_invoke_transaction_with_data(&sim_params, empty_soroban_data(), Vec::new())
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

    // 3. Adopt the authoritative footprint + resource fee from simulation.
    let soroban_data = sdkt_xdr::parse_soroban_transaction_data(&simulation.transaction_data)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse SorobanTransactionData: {e}")))?;

    let min_resource_fee = parse_min_resource_fee(&simulation.min_resource_fee)?;
    let inclusion_fee: u32 = 100;
    let total_fee = inclusion_fee.saturating_add(min_resource_fee);

    // 4. Auth entries returned by simulation (e.g. for contract-authorized calls).
    let auth_entries = if simulation.results.is_empty() {
        Vec::new()
    } else {
        sdkt_xdr::builder::parse_soroban_authorization_entries(&simulation.results[0].auth)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse auth entries: {e}")))?
    };

    // 5. Final envelope with real fees, then sign.
    let final_params = InvokeTransactionParams {
        fee: total_fee,
        ..sim_params
    };
    let final_envelope =
        sdkt_xdr::build_invoke_transaction_with_data(&final_params, soroban_data, auth_entries)
            .map_err(|e| RpcError::Rpc(format!("Failed to build final invoke transaction: {e}")))?;

    let signing_opts = SigningOptions::with(network);
    let signed_envelope = sign_transaction(&final_envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign invoke transaction: {e}")))?;

    Ok(PreparedInvoke {
        signed_envelope,
        fee: total_fee,
        sequence,
    })
}

/// Prepare an invocation envelope — fetch sequence, simulate for the
/// authoritative footprint/auth/fees, build the final envelope, sign it — and
/// return it *without* submitting.
///
/// Backs `sdkt invoke --build-only`: no `sendTransaction`, no polling, no
/// state change. The returned envelope is the exact bytes
/// [`invoke_contract`] would have submitted.
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

/// Invoke a contract function end-to-end: fetch sequence, simulate for the
/// authoritative footprint/auth/fees, build the final envelope, sign, submit,
/// and poll until the transaction settles.
///
/// `params.args` are base64-encoded `ScVal` strings (the format produced by
/// the CLI typed-argument parser).
pub async fn invoke_contract(
    client: &SorobanRpcClient,
    params: &InvokeTransactionParams,
    signer: &Ed25519Signer,
    network: Network,
    poll: &PollConfig,
) -> Result<InvokeResult, RpcError> {
    // 1-5. Sequence → simulate → adopt footprint/auth/fees → build → sign.
    let prepared = prepare_invoke_envelope(client, params, signer, network).await?;

    // 6. Submit and poll until settled.
    let submission = submit_and_wait(client, &prepared.signed_envelope, true, poll).await?;

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
