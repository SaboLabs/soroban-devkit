//! Bridge: turn a `sdkt-rpc` network capture into a `NetworkProfile`.
//!
//! This module owns the only crossing point between the transport-level
//! capture (`sdkt-rpc::network_capture`) and the fuzz engine's profile model
//! (`network_profile`). It exists so each side stays dependency-clean: the
//! RPC crate never links the host, and the fuzz crate never links reqwest.

use crate::network_profile::{
    CostParamEntrySnapshot, CostParamsSnapshot, NetworkConfigSnapshot, NetworkProfile, Observed,
};

/// Transport-level capture values (mirrored structurally from
/// `sdkt-rpc::network_capture::CapturedNetworkConfig` so no host-free crate
/// needs to depend on this one's types).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureInput {
    pub passphrase: String,
    pub protocol_version: u32,
    pub ledger_sequence: u32,
    pub source_endpoint: String,
    pub captured_at_unix: u64,
    pub cpu_limit: Option<(u64, u32)>,
    pub mem_limit: Option<(u64, u32)>,
    pub ledger_max_instructions: Option<(u64, u32)>,
    pub fee_rate_per_instructions_increment: Option<(u64, u32)>,
    pub max_contract_size_bytes: Option<(u32, u32)>,
    pub tx_max_size_bytes: Option<(u32, u32)>,
    pub tx_max_contract_events_size_bytes: Option<(u32, u32)>,
    pub fee_contract_events_1kb: Option<(i64, u32)>,
    pub cpu_cost_params: Option<Vec<CostParamEntrySnapshot>>,
    pub mem_cost_params: Option<Vec<CostParamEntrySnapshot>>,
    /// Ledger at which the cost-parameter entries last changed.
    pub cost_params_observed_at: Option<u32>,
}

/// Assemble a [`NetworkProfile`] from captured values.
///
/// Every captured pair is `(value, last_modified_ledger)`. A parameter the
/// capture did not fetch stays `Observed::unavailable()`, and the profile's
/// status reflects that (`Incomplete`) — never a silent zero.
pub fn profile_from_capture(input: CaptureInput) -> NetworkProfile {
    let obs = |v: Option<(u64, u32)>| match v {
        Some((value, ledger)) => Observed::live(value, ledger),
        None => Observed::unavailable(),
    };
    let obs32 = |v: Option<(u32, u32)>| match v {
        Some((value, ledger)) => Observed::live(value, ledger),
        None => Observed::unavailable(),
    };
    let obs64 = |v: Option<(i64, u32)>| match v {
        Some((value, ledger)) => Observed::live(value, ledger),
        None => Observed::unavailable(),
    };
    let cost_params = match (
        input.cpu_cost_params.clone(),
        input.mem_cost_params.clone(),
        input.cost_params_observed_at,
    ) {
        (Some(cpu), Some(mem), Some(ledger)) => {
            Observed::live(CostParamsSnapshot { cpu, mem }, ledger)
        }
        _ => Observed::unavailable(),
    };
    let config = NetworkConfigSnapshot {
        cpu_limit: obs(input.cpu_limit),
        mem_limit: obs(input.mem_limit),
        ledger_max_instructions: obs(input.ledger_max_instructions),
        fee_rate_per_instructions_increment: obs(input.fee_rate_per_instructions_increment),
        max_contract_size_bytes: obs32(input.max_contract_size_bytes),
        tx_max_size_bytes: obs32(input.tx_max_size_bytes),
        tx_max_contract_events_size_bytes: obs32(input.tx_max_contract_events_size_bytes),
        fee_contract_events_1kb: obs64(input.fee_contract_events_1kb),
        cost_params,
    };
    NetworkProfile::new(
        input.passphrase,
        input.protocol_version,
        input.ledger_sequence,
        input.source_endpoint,
        input.captured_at_unix,
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_profile::{ProfileStatus, HOST_SUPPORTED_PROTOCOL};

    fn capture(protocol: u32) -> CaptureInput {
        let params: Vec<CostParamEntrySnapshot> = (0..86)
            .map(|i| CostParamEntrySnapshot {
                cost_type: format!("Type{i}"),
                index: i,
                const_term: i as i64,
                linear_term: 0,
            })
            .collect();
        CaptureInput {
            passphrase: "Public Global Stellar Network ; September 2015".to_string(),
            protocol_version: protocol,
            ledger_sequence: 64_846_678,
            source_endpoint: "https://mainnet.sorobanrpc.com".to_string(),
            captured_at_unix: 1_791_518_867,
            cpu_limit: Some((400_000_000, 62_447_231)),
            mem_limit: Some((41_943_040, 62_447_231)),
            ledger_max_instructions: Some((580_000_000, 62_447_231)),
            fee_rate_per_instructions_increment: Some((7, 62_447_231)),
            max_contract_size_bytes: Some((131_072, 60_993_066)),
            tx_max_size_bytes: Some((132_096, 60_993_066)),
            tx_max_contract_events_size_bytes: Some((16_384, 60_993_066)),
            fee_contract_events_1kb: Some((200, 60_993_066)),
            cpu_cost_params: Some(params.clone()),
            mem_cost_params: Some(params),
            cost_params_observed_at: Some(62_447_231),
        }
    }

    #[test]
    fn mainnet_capture_becomes_host_unsupported_profile() {
        let p = profile_from_capture(capture(29));
        assert_eq!(
            p.network_id,
            crate::network_id_from_passphrase("Public Global Stellar Network ; September 2015")
        );
        assert_eq!(p.status(), ProfileStatus::HostUnsupported);
        assert!(!p.status().is_complete_execution_ready());
        // The configuration itself is fully observed.
        assert!(p.has_complete_configuration());
        assert_eq!(p.config.cpu_limit.value, Some(400_000_000));
        assert_eq!(p.config.mem_limit.value, Some(41_943_040));
    }

    #[test]
    fn protocol_28_capture_is_complete() {
        let p = profile_from_capture(capture(28));
        assert_eq!(p.status(), ProfileStatus::Complete);
    }

    #[test]
    fn partial_capture_is_incomplete() {
        let mut c = capture(28);
        c.cpu_limit = None;
        c.cpu_cost_params = None;
        c.mem_cost_params = None;
        c.cost_params_observed_at = None;
        let p = profile_from_capture(c);
        assert_eq!(p.status(), ProfileStatus::Incomplete);
        assert_eq!(p.config.cpu_limit.value, None);
        assert_eq!(p.config.cost_params.value, None);
    }

    #[test]
    fn host_supported_protocol_constant_is_28() {
        assert_eq!(HOST_SUPPORTED_PROTOCOL, 28);
    }
}
