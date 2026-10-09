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
use crate::network_cost::{
    network_cpu_cost_params, network_mem_cost_params, NETWORK_CPU_LIMIT, NETWORK_MEM_LIMIT,
};

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
    /// Fresh per-case budget built from the network-faithful Protocol 29
    /// cost model derived from stellar-core protocol configuration
    /// ([`crate::network_cost`]): the 23-entry CPU and memory cost-parameter
    /// tables plus the stellar-core `InitialSorobanNetworkConfig` resource
    /// limits. Construction goes through the same public
    /// `Budget::try_from_configs` path as [`BudgetPlan::Capped`] — only the
    /// cost table and the limits differ. No RPC, no network access: the
    /// tables are vendored, so the budget is deterministic offline.
    NetworkFaithful,
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
    /// `try_from_configs` path with the minimal cost model documented above;
    /// `NetworkFaithful` uses the same path with the vendored
    /// network-faithful Protocol 29 cost tables and stellar-core resource
    /// limits. A network-faithful construction failure is an explicit error,
    /// never a silent fallback to the synthetic model.
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
            BudgetPlan::NetworkFaithful => {
                let cpu_params = network_cpu_cost_params().map_err(|e| {
                    FuzzError::InvalidConfig(format!("network cpu cost params: {e}"))
                })?;
                let mem_params = network_mem_cost_params().map_err(|e| {
                    FuzzError::InvalidConfig(format!("network mem cost params: {e}"))
                })?;
                Budget::try_from_configs(
                    NETWORK_CPU_LIMIT,
                    NETWORK_MEM_LIMIT,
                    cpu_params,
                    mem_params,
                )
                .map_err(|e| FuzzError::InvalidConfig(format!("network-faithful budget: {e}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_env_host::xdr::ContractCostType;

    fn network_faithful_budget() -> Budget {
        Environment {
            ledger: LedgerConfig::default(),
            budget: BudgetPlan::NetworkFaithful,
        }
        .make_budget()
        .unwrap()
    }

    #[test]
    fn network_faithful_budget_carries_network_limits() {
        let budget = network_faithful_budget();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 2_500_000);
        assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 2_000_000);
    }

    #[test]
    fn network_faithful_budget_charges_the_stellar_core_table() {
        let budget = network_faithful_budget();
        // CPU: WasmInsnExec const 4, linear 0.
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        assert_eq!(budget.get_cpu_insns_consumed().unwrap(), 4);
        // MEM: MemAlloc const 16, linear 128 (initialMemCostParamsEntryForV20).
        // The linear term is scaled down by 2^7 internally, so input 128
        // yields linear 128 and total 16 + 128 = 144.
        budget
            .charge(ContractCostType::MemAlloc, Some(128))
            .unwrap();
        assert_eq!(budget.get_mem_bytes_consumed().unwrap(), 16 + 128);
    }

    #[test]
    fn default_budget_is_the_host_default() {
        let budget = Environment::default().make_budget().unwrap();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 100_000_000);
        assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 41_943_040);
    }

    #[test]
    fn capped_budget_still_uses_the_synthetic_single_entry_model() {
        let budget = Environment {
            ledger: LedgerConfig::default(),
            budget: BudgetPlan::Capped { cpu: 1, mem: 1 },
        }
        .make_budget()
        .unwrap();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 1);
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 0);
        let err = budget
            .charge(ContractCostType::WasmInsnExec, None)
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("Budget"),
            "expected a budget-class error, got {err:?}"
        );
    }
}
