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

/// Why the RPC side of a differential run was unavailable.
///
/// Separate variants — not a single string — so a test or report can
/// distinguish "the RPC call failed" from "the RPC call succeeded but the
/// response carried no cost metrics". The diagnostic detail is preserved in
/// each variant; the category is structural.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "category")]
pub enum RpcBlockReason {
    /// `simulateTransaction` succeeded but the response had no `cost` block,
    /// so no CPU/memory numbers exist to compare. The response shape is
    /// recorded (what it carried instead) — never zero-filled.
    MissingCost {
        /// Human-readable detail (what the response did carry).
        detail: String,
    },
    /// The `simulateTransaction` RPC call itself failed (transport, decode,
    /// or a malformed envelope the network rejected).
    RpcFailure {
        /// The underlying RPC error, verbatim.
        detail: String,
    },
    /// The invoke envelope could not be built (bad source account, bad
    /// contract id, sequence lookup failure, builder error).
    EnvelopeFailure {
        /// The underlying builder/lookup error, verbatim.
        detail: String,
    },
    /// Network metadata (`getNetwork` / `getLatestLedger`) could not be
    /// fetched, so the RPC metrics cannot be attributed to a protocol or
    /// ledger.
    NetworkMetadataFailure {
        /// The underlying error, verbatim.
        detail: String,
    },
}

impl RpcBlockReason {
    /// The category name, matching the task vocabulary.
    pub fn category(&self) -> &'static str {
        match self {
            RpcBlockReason::MissingCost { .. } => "missing_cost",
            RpcBlockReason::RpcFailure { .. } => "rpc_failure",
            RpcBlockReason::EnvelopeFailure { .. } => "envelope_failure",
            RpcBlockReason::NetworkMetadataFailure { .. } => "network_metadata_failure",
        }
    }

    /// The diagnostic detail, preserved verbatim.
    pub fn detail(&self) -> &str {
        match self {
            RpcBlockReason::MissingCost { detail }
            | RpcBlockReason::RpcFailure { detail }
            | RpcBlockReason::EnvelopeFailure { detail }
            | RpcBlockReason::NetworkMetadataFailure { detail } => detail,
        }
    }

    pub fn missing_cost(detail: impl Into<String>) -> Self {
        RpcBlockReason::MissingCost {
            detail: detail.into(),
        }
    }

    pub fn rpc_failure(detail: impl Into<String>) -> Self {
        RpcBlockReason::RpcFailure {
            detail: detail.into(),
        }
    }

    pub fn envelope_failure(detail: impl Into<String>) -> Self {
        RpcBlockReason::EnvelopeFailure {
            detail: detail.into(),
        }
    }

    pub fn network_metadata_failure(detail: impl Into<String>) -> Self {
        RpcBlockReason::NetworkMetadataFailure {
            detail: detail.into(),
        }
    }
}

/// The outcome of a differential run: either a real comparison, or an
/// explicit blocker.
///
/// The two are separate **types**, not a classification value, so a blocked
/// run can never be mistaken for a measured comparison. A `Blocked` outcome
/// carries no metric comparison and can never yield `Match`, `ResourceDrift`,
/// or `ExecutionDivergence`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DifferentialOutcome {
    /// The RPC side returned real metrics and the comparison ran.
    Compared {
        #[serde(flatten)]
        record: Box<DifferentialRecord>,
    },
    /// The RPC side could not be obtained. No metric comparison exists.
    Blocked {
        /// Why the RPC side was unavailable (typed category + detail).
        block: RpcBlockReason,
        /// Profile the local side ran under.
        profile_content_hash: String,
        /// Network and protocol, for inspection.
        network: String,
        protocol_version: u32,
        /// What the local execution actually measured.
        local: LocalExecutionMetrics,
        /// Function that was invoked locally.
        function: String,
    },
}

impl DifferentialOutcome {
    /// The comparison record, when the run actually compared.
    pub fn record(&self) -> Option<&DifferentialRecord> {
        match self {
            DifferentialOutcome::Compared { record } => Some(record),
            DifferentialOutcome::Blocked { .. } => None,
        }
    }

    /// True when the RPC side was unavailable. A blocked run is never a
    /// parity claim.
    pub fn is_blocked(&self) -> bool {
        matches!(self, DifferentialOutcome::Blocked { .. })
    }

    /// The blocker, when blocked.
    pub fn block_reason(&self) -> Option<&RpcBlockReason> {
        match self {
            DifferentialOutcome::Blocked { block, .. } => Some(block),
            DifferentialOutcome::Compared { .. } => None,
        }
    }

    /// The classification, when a comparison actually ran.
    pub fn classification(&self) -> Option<MismatchClass> {
        self.record().map(|r| r.classification)
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
    use crate::network_profile::HOST_SUPPORTED_PROTOCOL;
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
    fn protocol_above_the_host_is_classified_protocol_mismatch() {
        // The local host's protocol is pinned to the host's own metadata. A
        // network one step above it is a protocol mismatch: the local
        // execution is not a model of that network.
        let p = profile(HOST_SUPPORTED_PROTOCOL + 1, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(HOST_SUPPORTED_PROTOCOL, 100, 100, true),
            rpc(HOST_SUPPORTED_PROTOCOL + 1, 100, 100, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::ProtocolMismatch);
        assert!(rec.reason.contains("protocol"));
    }

    #[test]
    fn matching_protocol_with_matching_metrics_is_a_match() {
        // With the host on protocol 29, a protocol-29 network comparison is
        // now a real comparison rather than a protocol mismatch.
        let p = profile(29, true);
        assert_eq!(p.status(), ProfileStatus::Complete);
        let rec = compare(
            "c",
            "f",
            &p,
            local(29, 100_000, 50_000, true),
            rpc(29, 100_000, 50_000, false),
            0.01,
            0.01,
        );
        assert_eq!(rec.classification, MismatchClass::Match);
    }

    // --- DifferentialOutcome: a blocked run is never a parity claim --------

    /// Build a blocked outcome for a given category, with a real local
    /// execution attached so the evidence is reproducible.
    fn blocked(category: RpcBlockReason) -> DifferentialOutcome {
        let p = profile(29, true);
        DifferentialOutcome::Blocked {
            block: category,
            profile_content_hash: p.content_hash_hex(),
            network: p.network_name.clone(),
            protocol_version: p.protocol_version,
            local: local(29, 554_025, 1_336_202, false),
            function: "pause".to_string(),
        }
    }

    #[test]
    fn a_blocked_outcome_has_no_classification() {
        let out = blocked(RpcBlockReason::rpc_failure("connection refused"));
        assert!(out.is_blocked());
        assert_eq!(out.record(), None);
        assert_eq!(out.classification(), None);
        assert_eq!(
            out.block_reason().map(|b| b.category()),
            Some("rpc_failure")
        );
        // The critical property: a blocked run cannot be read as any
        // comparison verdict.
        assert!(!matches!(
            out.classification(),
            Some(MismatchClass::Match)
                | Some(MismatchClass::ResourceDrift)
                | Some(MismatchClass::ExecutionDivergence)
        ));
    }

    #[test]
    fn a_blocked_outcome_serializes_as_blocked_not_as_a_record() {
        let out = blocked(RpcBlockReason::rpc_failure("connection refused"));
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""outcome":"blocked""#), "{json}");
        // It must not carry any comparison classification.
        assert!(!json.contains("classification"), "{json}");
        // It must carry the evidence needed to reproduce the local side.
        assert!(json.contains("554025"), "{json}");
        assert!(json.contains("profile_content_hash"), "{json}");
        assert!(json.contains("connection refused"), "{json}");
        let back: DifferentialOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(back, out);
        assert!(back.is_blocked());
    }

    // --- Per-category blockers: each is distinct and carries its detail ----

    #[test]
    fn missing_cost_is_its_own_category() {
        let reason = RpcBlockReason::missing_cost(
            "simulateTransaction returned no cost metrics (transactionData=272 bytes, \
             minResourceFee=247484390, latestLedger=5102023)",
        );
        assert_eq!(reason.category(), "missing_cost");
        assert!(reason.detail().contains("no cost metrics"));
        assert!(reason.detail().contains("247484390"));

        let out = blocked(reason);
        assert!(out.is_blocked());
        assert_eq!(out.classification(), None);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""category":"missing_cost""#), "{json}");
        assert!(!json.contains("classification"), "{json}");
    }

    #[test]
    fn rpc_failure_is_its_own_category() {
        let reason = RpcBlockReason::rpc_failure("simulateTransaction: connection refused");
        assert_eq!(reason.category(), "rpc_failure");
        assert!(reason.detail().contains("connection refused"));

        let out = blocked(reason);
        assert!(out.is_blocked());
        assert_eq!(out.classification(), None);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""category":"rpc_failure""#), "{json}");
        assert!(!json.contains("classification"), "{json}");
    }

    #[test]
    fn envelope_failure_is_its_own_category() {
        let reason = RpcBlockReason::envelope_failure(
            "build_invoke_transaction: Invalid public key: not a G address",
        );
        assert_eq!(reason.category(), "envelope_failure");
        assert!(reason.detail().contains("Invalid public key"));

        let out = blocked(reason);
        assert!(out.is_blocked());
        assert_eq!(out.classification(), None);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""category":"envelope_failure""#), "{json}");
        assert!(!json.contains("classification"), "{json}");
    }

    #[test]
    fn network_metadata_failure_is_its_own_category() {
        let reason = RpcBlockReason::network_metadata_failure("getNetwork: timeout");
        assert_eq!(reason.category(), "network_metadata_failure");
        assert!(reason.detail().contains("timeout"));

        let out = blocked(reason);
        assert!(out.is_blocked());
        assert_eq!(out.classification(), None);
        let json = serde_json::to_string(&out).unwrap();
        assert!(
            json.contains(r#""category":"network_metadata_failure""#),
            "{json}"
        );
        assert!(!json.contains("classification"), "{json}");
    }

    /// A transport failure must never be reported as a missing-cost blocker:
    /// the categories are structurally distinct, so mislabelling is a type
    /// error rather than a string match.
    #[test]
    fn categories_are_structurally_distinct() {
        let transport = blocked(RpcBlockReason::rpc_failure("connection refused"));
        let missing = blocked(RpcBlockReason::missing_cost("no cost block"));
        assert_ne!(transport, missing);
        assert_ne!(
            transport.block_reason().map(|b| b.category()),
            missing.block_reason().map(|b| b.category())
        );
        // And neither carries a comparison verdict.
        assert_eq!(transport.classification(), None);
        assert_eq!(missing.classification(), None);
    }

    /// Every category round-trips through serialization with its detail
    /// intact and still carries no classification.
    #[test]
    fn every_category_round_trips_without_classification() {
        let all = [
            RpcBlockReason::missing_cost("no cost block"),
            RpcBlockReason::rpc_failure("connection refused"),
            RpcBlockReason::envelope_failure("bad key"),
            RpcBlockReason::network_metadata_failure("timeout"),
        ];
        for reason in all {
            let out = blocked(reason.clone());
            let json = serde_json::to_string(&out).unwrap();
            assert!(!json.contains("classification"), "{json}");
            assert!(json.contains(reason.detail()), "{json}");
            let back: DifferentialOutcome = serde_json::from_str(&json).unwrap();
            assert_eq!(back, out);
            assert_eq!(back.classification(), None);
            assert_eq!(
                back.block_reason().map(|b| b.category()),
                Some(reason.category())
            );
        }
    }

    #[test]
    fn a_compared_outcome_carries_its_classification() {
        let p = profile(29, true);
        let rec = compare(
            "c",
            "f",
            &p,
            local(29, 100_000, 50_000, true),
            rpc(29, 100_000, 50_000, false),
            0.01,
            0.01,
        );
        let out = DifferentialOutcome::Compared {
            record: Box::new(rec.clone()),
        };
        assert!(!out.is_blocked());
        assert_eq!(out.classification(), Some(MismatchClass::Match));
        assert_eq!(out.record(), Some(&rec));
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""outcome":"compared""#), "{json}");
        assert!(json.contains("classification"), "{json}");
    }
}
