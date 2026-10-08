//! Deterministic environment model for fuzz executions.
//!
//! An [`Environment`] carries exactly what `soroban-env-host` can honour
//! through its public API: ledger sequence, ledger timestamp, and a CPU/mem
//! budget cap. Protocol version is **not** configurable: host 28.0.2 rejects
//! ledger protocols other than its own via `set_ledger_info`, so protocol
//! variation is marked unsupported / not exercised rather than faked.

use soroban_env_host::budget::Budget;
use soroban_env_host::xdr::{ContractCostParamEntry, ContractCostParams, ExtensionPoint};
use soroban_env_host::LedgerInfo;

use crate::config::{FuzzConfig, LedgerConfig, PROTOCOL_VERSION};
use crate::error::FuzzError;

/// How the per-case budget is built.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BudgetPlan {
    /// Fresh `Budget::default()` per case (Phase 1 behavior).
    Default,
    /// Fresh per-case budget with explicit CPU/mem ceilings.
    ///
    /// Honesty note: `Budget::try_from_configs` is public but takes the full
    /// network cost-parameter table. This crate does not have network
    /// parameters, so capped budgets use a minimal, fixed instruction-cost
    /// model (one charged cost type, `WasmInsnExec`, const term 1 per
    /// execution). The ceilings are therefore **deterministic and
    /// reproducible but not network-faithful**: they exist so a case can
    /// exercise the budget-exhaustion path at a known limit, not to mimic
    /// fee computation. `resources.instructions` is left at its maximum:
    /// enforcement is by host budget, which is the deterministic boundary.
    Capped { cpu: u64, mem: u64 },
}

/// Per-execution environment: ledger context + budget plan.
///
/// Invariant: all fields are fixed-width integers — no wall-clock, no
/// randomness. Same `Environment` ⇒ same `LedgerInfo` ⇒ same execution
/// context (given the same host version).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Environment {
    pub ledger: LedgerConfig,
    pub budget: BudgetPlan,
}

impl Environment {
    /// Environment derived from a campaign config (Phase 1 defaults).
    pub fn from_config(config: &FuzzConfig) -> Self {
        Self {
            ledger: config.ledger.clone(),
            budget: BudgetPlan::Default,
        }
    }

    /// Ledger info for this environment against the campaign network id.
    pub(crate) fn ledger_info(&self, network_id: [u8; 32]) -> LedgerInfo {
        LedgerInfo {
            protocol_version: PROTOCOL_VERSION,
            sequence_number: self.ledger.sequence_number,
            timestamp: self.ledger.timestamp,
            network_id,
            base_reserve: 5_000_000,
            min_temp_entry_ttl: 16,
            min_persistent_entry_ttl: 100_000,
            max_entry_ttl: 10_000_000,
        }
    }

    /// Build the fresh per-case budget. `Default` returns the host default
    /// budget (identical to Phase 1); `Capped` uses the public
    /// `try_from_configs` path with the minimal cost model documented above.
    pub(crate) fn make_budget(&self) -> Result<Budget, FuzzError> {
        match &self.budget {
            BudgetPlan::Default => Ok(Budget::default()),
            BudgetPlan::Capped { cpu, mem } => {
                // Minimal cost table: only `WasmInsnExec` (index 0) charges.
                // Every other cost type stays at the zero default, which is
                // exactly why these limits are deterministic-but-synthetic.
                let params = ContractCostParams::try_from(vec![ContractCostParamEntry {
                    ext: ExtensionPoint::V0,
                    const_term: 1,
                    linear_term: 0,
                }])
                .map_err(|e| FuzzError::InvalidConfig(format!("cost params: {e}")))?;
                Budget::try_from_configs(*cpu, *mem, params.clone(), params)
                    .map_err(|e| FuzzError::InvalidConfig(format!("budget: {e}")))
            }
        }
    }

    /// Validate the environment for a campaign config (TTL window fit).
    pub(crate) fn validate(&self, config: &FuzzConfig) -> Result<(), FuzzError> {
        config.validate()?;
        let seq = self.ledger.sequence_number;
        if seq > u32::MAX - crate::config::BASELINE_TTL_WINDOW {
            return Err(FuzzError::InvalidConfig(format!(
                "environment ledger sequence_number {seq} leaves no room for the baseline TTL window"
            )));
        }
        Ok(())
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            ledger: LedgerConfig::default(),
            budget: BudgetPlan::Default,
        }
    }
}
