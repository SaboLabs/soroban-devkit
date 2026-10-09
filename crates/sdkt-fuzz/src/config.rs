//! Execution configuration for the fuzz core.
//!
//! Everything the executor needs to be deterministic across processes:
//! seed, network identity, and ledger context. Deliberately small — future
//! phases (generation, mutation, campaign) extend this, they do not
//! redesign it.

use crate::error::FuzzError;

/// Soroban protocol version pinned by the host dependency.
///
/// Derived from the host's own `meta::INTERFACE_VERSION.protocol` rather than
/// hardcoded: `soroban-env-host` refuses a `LedgerInfo` whose
/// `protocol_version` differs from that value (`check_ledger_protocol_supported`
/// — "ledger protocol version too old/new for host"), so a mismatch is always
/// an invalid setup, never a fuzz case. Bumping the host dependency moves this
/// constant with it.
pub const PROTOCOL_VERSION: u32 = soroban_env_host::meta::INTERFACE_VERSION.protocol;

/// TTL window granted to each baseline ledger entry, in ledgers.
pub(crate) const BASELINE_TTL_WINDOW: u32 = 1_000_000;

/// Campaign-level configuration.
///
/// Invariants:
/// - `seed` and `network_id` are fixed-width byte strings: no hidden RNG
///   state influences them.
/// - the ledger sequence must leave room for [`BASELINE_TTL_WINDOW`] and
///   stay inside the network TTL bound, enforced by [`FuzzConfig::validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzConfig {
    /// Deterministic base PRNG seed handed to the Soroban host per case.
    /// The same seed + wasm + case must reproduce the same observation.
    pub seed: [u8; 32],
    /// Network id the contract instance / contract id hashes bind to.
    pub network_id: [u8; 32],
    /// Ledger context for executions (sequence, timestamp).
    pub ledger: LedgerConfig,
    /// Per-case budget configuration.
    pub budget: BudgetConfig,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        Self {
            seed: *b"sdkt-fuzz-phase1-default-seed!!!",
            network_id: [5; 32],
            ledger: LedgerConfig::default(),
            budget: BudgetConfig::default(),
        }
    }
}

impl FuzzConfig {
    /// Reject configurations the executor cannot honour.
    pub(crate) fn validate(&self) -> Result<(), FuzzError> {
        let seq = self.ledger.sequence_number;
        if seq > u32::MAX - BASELINE_TTL_WINDOW {
            return Err(FuzzError::InvalidConfig(format!(
                "ledger sequence_number {seq} leaves no room for the baseline TTL window"
            )));
        }
        if self.budget.cpu_limit.is_some() || self.budget.mem_limit.is_some() {
            return Err(FuzzError::InvalidConfig(
                "custom budget limits require the network cost-parameter table; \
                 Phase 1 only supports the default budget"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Ledger info handed to every host construction.
    pub(crate) fn ledger_info(&self) -> soroban_env_host::LedgerInfo {
        soroban_env_host::LedgerInfo {
            protocol_version: PROTOCOL_VERSION,
            sequence_number: self.ledger.sequence_number,
            timestamp: self.ledger.timestamp,
            network_id: self.network_id,
            base_reserve: 5_000_000,
            min_temp_entry_ttl: 16,
            min_persistent_entry_ttl: 100_000,
            max_entry_ttl: 10_000_000,
        }
    }

    /// `live_until_ledger` given to each baseline entry.
    #[allow(dead_code)] // Phase 2 executions use `Environment::ledger_info`.
    pub(crate) fn baseline_live_until(&self) -> u32 {
        self.ledger
            .sequence_number
            .saturating_add(BASELINE_TTL_WINDOW)
    }
}

/// Minimal ledger context for deterministic executions.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LedgerConfig {
    /// Current ledger sequence for the execution.
    pub sequence_number: u32,
    /// Ledger close timestamp. Fixed, never wall-clock.
    pub timestamp: u64,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            sequence_number: 1_000_000,
            timestamp: 0,
        }
    }
}

/// Per-case budget configuration.
///
/// Phase 1 supports only the host default budget (fresh `Budget::default()`
/// per case — the per-case budget reset). Building a budget with custom
/// limits requires the full network cost-parameter table, which is not part
/// of this crate's boundary yet; setting either limit to `Some` is an
/// [`FuzzError::InvalidConfig`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetConfig {
    /// Reserved for a future phase that supplies network cost params.
    pub cpu_limit: Option<u64>,
    /// Reserved for a future phase that supplies network cost params.
    pub mem_limit: Option<u64>,
}
