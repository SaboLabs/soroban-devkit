//! Differential comparison: local execution vs RPC `simulateTransaction`.
//!
//! ## Scope and honesty
//!
//! `simulateTransaction` is **not** an oracle. It returns a subset of what the
//! host computes internally: a resource cost (`cpuInsns`, `memBytes`), the
//! transaction data, per-operation auth entries, and (optionally) state
//! changes and events. It does **not** expose the full cost-type breakdown,
//! the fee configuration, or the internal budget counters.
//!
//! This module therefore compares only what both sides actually report, and
//! classifies every difference into a named category. A mismatch is never
//! silently "explained away": it is recorded with the inputs and metadata
//! needed to reproduce it.

use serde::{Deserialize, Serialize};

use crate::network_profile::NetworkProfile;

/// Why a local-vs-RPC comparison could not be made, or what it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchClass {
    /// The local host's protocol version differs from the network's. No
    /// comparison is meaningful; the local execution is not a model of this
    /// network.
    ProtocolMismatch,
    /// The local host version differs from the network's host build. The
    /// protocol may match, but the cost model may not.
    HostMismatch,
    /// The RPC simulation ran against a ledger sequence far from the local
    /// execution's ledger, so state-dependent results can differ.
    StaleLedgerState,
    /// The local and RPC resource costs differ beyond the documented
    /// tolerance. This is the interesting case: it means the local cost model
    /// does not reproduce the network's.
    ResourceDrift,
    /// The execution outcomes differ (return value, error, state changes,
    /// events) in a way not explained by the above.
    ExecutionDivergence,
    /// The network profile is incomplete, so the comparison cannot be
    /// claimed to be faithful.
    IncompleteProfile,
    /// The comparison succeeded within tolerance.
    Match,
}

impl MismatchClass {
    pub fn as_str(self) -> &'static str {
        match self {
            MismatchClass::ProtocolMismatch => "PROTOCOL_MISMATCH",
            MismatchClass::HostMismatch => "HOST_MISMATCH",
            MismatchClass::StaleLedgerState => "STALE_LEDGER_STATE",
            MismatchClass::ResourceDrift => "RESOURCE_DRIFT",
            MismatchClass::ExecutionDivergence => "EXECUTION_DIVERGENCE",
            MismatchClass::IncompleteProfile => "INCOMPLETE_PROFILE",
            MismatchClass::Match => "MATCH",
        }
    }

    /// A mismatch is "explained" when it is a known, documented limitation
    /// rather than a divergence in the thing being measured.
    pub fn is_explained_limitation(self) -> bool {
        matches!(
            self,
            MismatchClass::ProtocolMismatch
                | MismatchClass::HostMismatch
                | MismatchClass::StaleLedgerState
                | MismatchClass::IncompleteProfile
        )
    }
}

/// The local side of a comparison: what the pinned host actually computed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalExecutionMetrics {
    /// Ledger sequence the local execution used.
    pub ledger_sequence: u32,
    /// Protocol version the local host enforces.
    pub protocol_version: u32,
    /// CPU instructions consumed (host `get_cpu_insns_consumed`).
    pub cpu_insns: u64,
    /// Memory bytes consumed (host `get_mem_bytes_consumed`, the peak).
    pub mem_bytes: u64,
    /// Whether the call returned successfully (no contract/host error).
    pub succeeded: bool,
    /// `ScErrorType` name when the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
}

/// The RPC side of a comparison: what `simulateTransaction` reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcSimulationMetrics {
    /// Ledger sequence the simulation ran against.
    pub ledger_sequence: u32,
    /// Protocol version the network reports.
    pub protocol_version: u32,
    /// CPU instructions reported by the simulation.
    pub cpu_insns: u64,
    /// Memory bytes reported by the simulation.
    pub mem_bytes: u64,
    /// Whether the simulation reported an error.
    pub error: bool,
}

/// A recorded comparison, with everything needed to reproduce it.
///
/// `f64` tolerances are stored as their bit patterns (`u64`) so the record
/// stays `Eq`-comparable and hash-stable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DifferentialRecord {
    /// Case identity (deterministic, from the campaign).
    pub case_id: String,
    /// Function invoked.
    pub function: String,
    /// Profile content hash the local side was built from.
    pub profile_content_hash: String,
    /// Network name and protocol, for human inspection.
    pub network: String,
    pub protocol_version: u32,
    pub local: LocalExecutionMetrics,
    pub rpc: RpcSimulationMetrics,
    pub classification: MismatchClass,
    /// Human-readable explanation, always present (never an empty string).
    pub reason: String,
    /// Relative CPU tolerance applied (see [`compare`]).
    pub cpu_tolerance_bits: u64,
    /// Relative memory tolerance applied.
    pub mem_tolerance_bits: u64,
}

impl DifferentialRecord {
    pub fn cpu_tolerance(&self) -> f64 {
        f64::from_bits(self.cpu_tolerance_bits)
    }

    pub fn mem_tolerance(&self) -> f64 {
        f64::from_bits(self.mem_tolerance_bits)
    }
}

/// Compare a local execution against an RPC simulation.
///
/// The comparison is ordered: protocol first (a protocol mismatch makes every
/// other comparison meaningless), then ledger currency, then resource drift,
/// then execution outcome. The first failing check wins.
///
/// `cpu_tolerance` / `mem_tolerance` are relative (0.01 = 1%). They exist
/// because the RPC cost is computed by a different code path (the network's
/// host build) and small rounding differences are expected; they are
/// **recorded** in the artifact so a widened tolerance is visible, never
/// hidden.
pub fn compare(
    case_id: &str,
    function: &str,
    profile: &NetworkProfile,
    local: LocalExecutionMetrics,
    rpc: RpcSimulationMetrics,
    cpu_tolerance: f64,
    mem_tolerance: f64,
) -> DifferentialRecord {
    let classification = classify(profile, &local, &rpc, cpu_tolerance, mem_tolerance);
    let reason = reason_for(
        classification,
        profile,
        &local,
        &rpc,
        cpu_tolerance,
        mem_tolerance,
    );
    DifferentialRecord {
        case_id: case_id.to_string(),
        function: function.to_string(),
        profile_content_hash: profile.content_hash_hex(),
        network: profile.network_name.clone(),
        protocol_version: profile.protocol_version,
        local,
        rpc,
        classification,
        reason,
        cpu_tolerance_bits: cpu_tolerance.to_bits(),
        mem_tolerance_bits: mem_tolerance.to_bits(),
    }
}

fn classify(
    profile: &NetworkProfile,
    local: &LocalExecutionMetrics,
    rpc: &RpcSimulationMetrics,
    cpu_tolerance: f64,
    mem_tolerance: f64,
) -> MismatchClass {
    // 1. Protocol: the local host enforces its own protocol; the network may
    //    be ahead. A protocol mismatch invalidates every other comparison.
    if local.protocol_version != rpc.protocol_version {
        return MismatchClass::ProtocolMismatch;
    }
    // 2. Profile completeness: an incomplete profile means the local cost
    //    model is knowingly partial, so a resource difference is not
    //    evidence of divergence.
    if profile.status() != crate::network_profile::ProfileStatus::Complete {
        return MismatchClass::IncompleteProfile;
    }
    // 3. Ledger currency: state-dependent results can differ if the two
    //    sides ran against different ledgers.
    let ledger_gap = (local.ledger_sequence as i64 - rpc.ledger_sequence as i64).unsigned_abs();
    if ledger_gap > MAX_COMPARISON_LEDGER_GAP {
        return MismatchClass::StaleLedgerState;
    }
    // 4. Resource drift: the local cost model must reproduce the network's
    //    reported cost within tolerance.
    if !within_tolerance(local.cpu_insns, rpc.cpu_insns, cpu_tolerance)
        || !within_tolerance(local.mem_bytes, rpc.mem_bytes, mem_tolerance)
    {
        return MismatchClass::ResourceDrift;
    }
    // 5. Execution outcome: success/failure must agree.
    if local.succeeded == rpc.error {
        return MismatchClass::ExecutionDivergence;
    }
    MismatchClass::Match
}

fn reason_for(
    class: MismatchClass,
    profile: &NetworkProfile,
    local: &LocalExecutionMetrics,
    rpc: &RpcSimulationMetrics,
    cpu_tolerance: f64,
    mem_tolerance: f64,
) -> String {
    match class {
        MismatchClass::ProtocolMismatch => format!(
            "local protocol {} != network protocol {}; local execution is not a model of this network",
            local.protocol_version, rpc.protocol_version
        ),
        MismatchClass::HostMismatch => format!(
            "local host build differs from the network's host build (profile protocol {})",
            profile.protocol_version
        ),
        MismatchClass::StaleLedgerState => format!(
            "ledger gap {} exceeds {} (local {}, rpc {})",
            (local.ledger_sequence as i64 - rpc.ledger_sequence as i64).unsigned_abs(),
            MAX_COMPARISON_LEDGER_GAP,
            local.ledger_sequence,
            rpc.ledger_sequence
        ),
        MismatchClass::ResourceDrift => format!(
            "resource drift beyond tolerance (cpu {} vs {}, mem {} vs {}, tolerances {:.4}/{:.4})",
            local.cpu_insns,
            rpc.cpu_insns,
            local.mem_bytes,
            rpc.mem_bytes,
            cpu_tolerance,
            mem_tolerance
        ),
        MismatchClass::ExecutionDivergence => format!(
            "execution outcome differs (local succeeded={}, rpc error={})",
            local.succeeded, rpc.error
        ),
        MismatchClass::IncompleteProfile => format!(
            "profile is {} (covered {}/{} cost types); comparison is not faithful",
            profile.status().as_str(),
            profile
                .config
                .cost_params
                .value
                .as_ref()
                .map(|p| p.entry_count())
                .unwrap_or(0),
            crate::environment::host_cost_type_count(),
        ),
        MismatchClass::Match => "local execution reproduces the RPC simulation within tolerance".to_string(),
    }
}

/// Maximum ledger gap between the local execution and the RPC simulation
/// before the comparison is classified as stale.
pub const MAX_COMPARISON_LEDGER_GAP: u64 = 100;

fn within_tolerance(local: u64, rpc: u64, tolerance: f64) -> bool {
    if rpc == 0 {
        return local == 0;
    }
    let diff = (local as i64 - rpc as i64).unsigned_abs() as f64;
    diff / rpc as f64 <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_profile::{
        CostParamEntrySnapshot, CostParamsSnapshot, NetworkConfigSnapshot, Observed, ProfileStatus,
    };

    const MAINNET: &str = "Public Global Stellar Network ; September 2015";

    fn profile(protocol: u32, complete: bool) -> NetworkProfile {
        let n = if complete { 86 } else { 23 };
        let params = CostParamsSnapshot {
            cpu: (0..n)
                .map(|i| CostParamEntrySnapshot {
                    cost_type: format!("T{i}"),
                    index: i as u32,
                    const_term: 1,
                    linear_term: 0,
                })
                .collect(),
            mem: (0..n)
                .map(|i| CostParamEntrySnapshot {
                    cost_type: format!("T{i}"),
                    index: i as u32,
                    const_term: 1,
                    linear_term: 0,
                })
                .collect(),
        };
        let mut config = NetworkConfigSnapshot::default();
        if complete {
            config.cpu_limit = Observed::live(400_000_000, 1);
            config.mem_limit = Observed::live(41_943_040, 1);
            config.ledger_max_instructions = Observed::live(580_000_000, 1);
            config.fee_rate_per_instructions_increment = Observed::live(7, 1);
            config.max_contract_size_bytes = Observed::live(131_072, 1);
            config.tx_max_size_bytes = Observed::live(132_096, 1);
            config.tx_max_contract_events_size_bytes = Observed::live(16_384, 1);
            config.fee_contract_events_1kb = Observed::live(200, 1);
            config.cost_params = Observed::live(params, 1);
        }
        NetworkProfile::new(
            MAINNET.to_string(),
            protocol,
            1000,
            "https://example.invalid".to_string(),
            0,
            config,
        )
    }

    fn local(protocol: u32, cpu: u64, mem: u64, succeeded: bool) -> LocalExecutionMetrics {
        LocalExecutionMetrics {
            ledger_sequence: 1000,
            protocol_version: protocol,
            cpu_insns: cpu,
            mem_bytes: mem,
            succeeded,
            error_type: None,
        }
    }

    fn rpc(protocol: u32, cpu: u64, mem: u64, error: bool) -> RpcSimulationMetrics {
        RpcSimulationMetrics {
            ledger_sequence: 1000,
            protocol_version: protocol,
            cpu_insns: cpu,
            mem_bytes: mem,
            error,
        }
    }

    #[test]
    fn protocol_mismatch_is_detected_first() {
        let p = profile(28, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100, 100, true),
            rpc(29, 100, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::ProtocolMismatch);
        assert!(rec.reason.contains("protocol"));
    }

    #[test]
    fn incomplete_profile_is_not_a_faithful_comparison() {
        let p = profile(28, false);
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100, 100, true),
            rpc(28, 100, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::IncompleteProfile);
        assert!(rec.reason.contains("INCOMPLETE"));
    }

    #[test]
    fn stale_ledger_state_is_detected() {
        let p = profile(28, true);
        let mut r = rpc(28, 100, 100, false);
        r.ledger_sequence = 1000 + MAX_COMPARISON_LEDGER_GAP as u32 + 1;
        let rec = compare("c", "f", &p, local(28, 100, 100, true), r, 0.01, 0.01);
        assert_eq!(rec.classification, MismatchClass::StaleLedgerState);
    }

    #[test]
    fn resource_drift_beyond_tolerance_is_detected() {
        let p = profile(28, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100_000, 100, true),
            rpc(28, 100, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::ResourceDrift);
        assert!(rec.reason.contains("resource drift"));
    }

    #[test]
    fn small_differences_within_tolerance_match() {
        let p = profile(28, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100_500, 100, true),
            rpc(28, 100_000, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::Match);
    }

    #[test]
    fn execution_divergence_is_detected() {
        let p = profile(28, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100, 100, true),
            rpc(28, 100, 100, true),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::ExecutionDivergence);
    }

    #[test]
    fn record_serializes_with_reproduction_metadata() {
        let p = profile(28, true);
        let rec = compare(
            "case-1",
            "increment",
            &p,
            local(28, 100, 100, true),
            rpc(28, 100, 100, false),
            0.01,
            0.01,
        );
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("case-1"));
        assert!(json.contains(&p.content_hash_hex()));
        assert!(json.contains("cpu_tolerance"));
        let back: DifferentialRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.classification, MismatchClass::Match);
    }

    #[test]
    fn mainnet_profile_is_classified_host_unsupported() {
        let p = profile(29, true);
        assert_eq!(p.status(), ProfileStatus::HostUnsupported);
        // A comparison against a protocol-29 network is a protocol mismatch,
        // which is the honest classification.
        let rec = compare(
            "c",
            "f",
            &p,
            local(28, 100, 100, true),
            rpc(29, 100, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::ProtocolMismatch);
    }
}
