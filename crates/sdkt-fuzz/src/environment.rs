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
use crate::network_profile::{NetworkProfile, ProfileStatus};

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
    /// Fresh per-case budget built from the **protocol-initial** cost model
    /// ([`crate::network_cost`]): the 23-entry CPU and memory cost-parameter
    /// tables from stellar-core's `initialCpuCostParamsEntryForV20()` /
    /// `initialMemCostParamsEntryForV20()`, plus the
    /// `InitialSorobanNetworkConfig` resource limits.
    ///
    /// Naming is deliberate: this is the protocol-initial configuration, not
    /// a live Mainnet validator configuration and not the current Protocol 29
    /// cost model — stellar-core mutates the V20 table on later upgrades
    /// (V21 rewrites `VmCachedInstantiation` and appends 21 cost types).
    /// Construction goes through the same public `Budget::try_from_configs`
    /// path as [`BudgetPlan::Capped`] — only the cost table and the limits
    /// differ. No RPC, no network access: the tables are vendored, so the
    /// budget is deterministic offline.
    ProtocolInitial,
    /// Fresh per-case budget built from a **live-observed** network profile
    /// ([`crate::network_profile::NetworkProfile`]).
    ///
    /// Unlike [`BudgetPlan::ProtocolInitial`], the cost table and limits are
    /// the values the network actually reported, not a vendored table. The
    /// profile also carries a coverage verdict ([`BudgetCoverage`]) which is
    /// stored alongside the plan so a caller can never mistake a partial model
    /// for a complete one.
    ///
    /// Construction refuses (never silently degrades) when:
    ///
    /// - the profile's config is incomplete (`ProfileStatus::Incomplete`), or
    /// - the profile's protocol exceeds [`crate::network_profile::HOST_SUPPORTED_PROTOCOL`]
    ///   (`ProfileStatus::HostUnsupported`) — the pinned host would reject the
    ///   ledger protocol anyway, and pretending otherwise would be a false
    ///   parity claim.
    ///
    /// The budget itself is still buildable from an unsupported profile's cost
    /// table (useful for differential comparison against RPC simulation); the
    /// distinction is carried by [`BudgetCoverage::status`] rather than by
    /// refusing to construct.
    NetworkFaithful {
        profile: Box<NetworkProfile>,
        coverage: BudgetCoverage,
    },
}

/// How much of the network's cost model a `NetworkFaithful` budget actually
/// reproduces.
///
/// This is the honest answer to "is this network-faithful?": the plan is
/// faithful to the observed *table*, but a table shorter than the host's cost
/// type count means the uncovered types are charged zero, so the model is
/// partial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BudgetCoverage {
    /// Profile status this budget was built from.
    pub profile_status: ProfileStatus,
    /// Cost types the observed table covers.
    pub covered_cost_types: usize,
    /// Cost types the pinned host knows about.
    pub host_cost_types: usize,
    /// `covered_cost_types >= host_cost_types` and the profile was complete
    /// for the host's protocol.
    pub complete: bool,
}

impl BudgetCoverage {
    /// Build a coverage verdict for a profile against the pinned host.
    pub fn for_profile(profile: &NetworkProfile, host_cost_types: usize) -> Self {
        let covered = profile
            .config
            .cost_params
            .value
            .as_ref()
            .map(|p| p.entry_count())
            .unwrap_or(0);
        let status = profile.status();
        let complete = status == ProfileStatus::Complete && covered >= host_cost_types;
        Self {
            profile_status: status,
            covered_cost_types: covered,
            host_cost_types,
            complete,
        }
    }

    /// One-line verdict, matching the status vocabulary the PR uses.
    pub fn as_str(&self) -> &'static str {
        if self.complete {
            "COMPLETE"
        } else if self.profile_status == ProfileStatus::HostUnsupported {
            "HOST_UNSUPPORTED"
        } else if self.covered_cost_types < self.host_cost_types {
            "INCOMPLETE"
        } else {
            "UNSUPPORTED"
        }
    }
}

/// One-line coverage verdict for the `NetworkFaithful` refusal message.
fn coverage_str(c: &BudgetCoverage) -> &'static str {
    c.as_str()
}

/// The number of `ContractCostType` variants the pinned host understands.
pub fn host_cost_type_count() -> usize {
    soroban_env_host::xdr::ContractCostType::variants().len()
}

/// Build a `ContractCostParams` from a profile's snapshot entries.
///
/// `cpu` selects the CPU or memory dimension. The entries must be ordered by
/// index (the profile validator guarantees this); a gap or a name/index
/// mismatch is an explicit error rather than a mis-mapped charge.
fn cost_params_from_snapshot(
    cpu: &[crate::network_profile::CostParamEntrySnapshot],
    mem: &[crate::network_profile::CostParamEntrySnapshot],
    for_cpu: bool,
) -> Result<ContractCostParams, FuzzError> {
    let dim = if for_cpu { cpu } else { mem };
    let mut entries = Vec::with_capacity(dim.len());
    for (i, e) in dim.iter().enumerate() {
        if e.index as usize != i {
            return Err(FuzzError::InvalidConfig(format!(
                "profile cost params: entry {} declares index {}",
                i, e.index
            )));
        }
        let cost_type =
            soroban_env_host::xdr::ContractCostType::try_from(i as i32).map_err(|_| {
                FuzzError::InvalidConfig(format!(
                    "cost type index {i} is not a valid ContractCostType"
                ))
            })?;
        if cost_type.name() != e.cost_type {
            return Err(FuzzError::InvalidConfig(format!(
                "profile cost params: index {i} is `{}` on the host but `{}` in the profile",
                cost_type.name(),
                e.cost_type
            )));
        }
        entries.push(ContractCostParamEntry {
            ext: ExtensionPoint::V0,
            const_term: e.const_term,
            linear_term: e.linear_term,
        });
    }
    ContractCostParams::try_from(entries)
        .map_err(|e| FuzzError::InvalidConfig(format!("profile cost params: {e}")))
}

/// Construct a `BudgetPlan::NetworkFaithful` from a profile.
///
/// The coverage is computed here, once, from the profile and the pinned host —
/// so the plan always carries the verdict that matches its data. A profile
/// whose status is not `Complete` still produces a plan (useful for reporting
/// and differential comparison), but [`Environment::make_budget`] refuses to
/// build a budget from it.
pub fn network_faithful_plan(profile: NetworkProfile) -> BudgetPlan {
    let coverage = BudgetCoverage::for_profile(&profile, host_cost_type_count());
    BudgetPlan::NetworkFaithful {
        profile: Box::new(profile),
        coverage,
    }
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
    /// `ProtocolInitial` uses the same path with the vendored protocol-initial
    /// cost tables and stellar-core resource limits. A protocol-initial
    /// construction failure is an explicit error, never a silent fallback to
    /// the synthetic model.
    ///
    /// `NetworkFaithful` refuses (with the coverage verdict named in the
    /// error) when the profile is incomplete or its protocol exceeds the
    /// pinned host's.
    pub fn make_budget(&self) -> Result<Budget, FuzzError> {
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
            BudgetPlan::ProtocolInitial => {
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
                .map_err(|e| FuzzError::InvalidConfig(format!("protocol-initial budget: {e}")))
            }
            BudgetPlan::NetworkFaithful { profile, coverage } => {
                if !coverage.complete {
                    // Refuse rather than silently substituting a partial model.
                    // The caller asked for network-faithful execution; telling
                    // them they got something else by returning Ok would be the
                    // false-parity claim this whole module exists to avoid.
                    return Err(FuzzError::InvalidConfig(format!(
                        "NetworkFaithful budget unavailable: {} \
                         (profile {}, covered {}/{} cost types)",
                        coverage_str(coverage),
                        coverage.profile_status.as_str(),
                        coverage.covered_cost_types,
                        coverage.host_cost_types,
                    )));
                }
                let params = profile.config.cost_params.value.as_ref().ok_or_else(|| {
                    FuzzError::InvalidConfig(
                        "NetworkFaithful budget: profile has no cost params".to_string(),
                    )
                })?;
                let cpu = profile.config.cpu_limit.value.ok_or_else(|| {
                    FuzzError::InvalidConfig(
                        "NetworkFaithful budget: profile has no CPU limit".to_string(),
                    )
                })?;
                let mem = profile.config.mem_limit.value.ok_or_else(|| {
                    FuzzError::InvalidConfig(
                        "NetworkFaithful budget: profile has no memory limit".to_string(),
                    )
                })?;
                let cpu_params = cost_params_from_snapshot(&params.cpu, &params.mem, true)?;
                let mem_params = cost_params_from_snapshot(&params.cpu, &params.mem, false)?;
                Budget::try_from_configs(cpu, mem, cpu_params, mem_params)
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
    use crate::network_cost::NETWORK_COST_ENTRY_COUNT;
    use soroban_env_host::xdr::ContractCostType;

    fn protocol_initial_budget() -> Budget {
        Environment {
            ledger: LedgerConfig::default(),
            budget: BudgetPlan::ProtocolInitial,
        }
        .make_budget()
        .unwrap()
    }

    #[test]
    fn protocol_initial_budget_carries_protocol_limits() {
        let budget = protocol_initial_budget();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 2_500_000);
        assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 2_000_000);
    }

    #[test]
    fn protocol_initial_budget_charges_the_stellar_core_table() {
        let budget = protocol_initial_budget();
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

    /// Semantics, not constant-vs-constant: the budget must charge the
    /// protocol-initial values that stellar-core's V21 upgrade later
    /// rewrites (`VmCachedInstantiation` (451626, 45405) → (41142, 634)).
    /// `get_tracker()` proves the charge was routed through the variant's
    /// own cost model rather than the host default. The memory charge is
    /// `Some(0)` because this cost type is a linear model on the memory
    /// dimension (the tracker requires a consistent Some/None input).
    #[test]
    fn protocol_initial_charges_the_pre_v21_vm_cached_instantiation_model() {
        let budget = protocol_initial_budget();
        budget
            .charge(ContractCostType::VmCachedInstantiation, Some(0))
            .unwrap();
        let tracker = budget
            .get_tracker(ContractCostType::VmCachedInstantiation)
            .unwrap();
        assert_eq!(tracker.iterations, 1, "charge must hit this cost type");
        assert_eq!(
            tracker.cpu, 451_626,
            "protocol-initial VmCachedInstantiation CPU cost (V21 rewrites to 41142)"
        );
    }

    /// The variant is not the host default: `VmCachedInstantiation` in the
    /// host default model costs 41142, not the protocol-initial 451626.
    #[test]
    fn protocol_initial_differs_from_the_host_default_model() {
        let plan = protocol_initial_budget();
        let host_default = Budget::default();
        plan.charge(ContractCostType::VmCachedInstantiation, Some(0))
            .unwrap();
        host_default
            .charge(ContractCostType::VmCachedInstantiation, Some(0))
            .unwrap();
        let plan_cpu = plan
            .get_tracker(ContractCostType::VmCachedInstantiation)
            .unwrap()
            .cpu;
        let host_cpu = host_default
            .get_tracker(ContractCostType::VmCachedInstantiation)
            .unwrap()
            .cpu;
        assert_eq!(plan_cpu, 451_626);
        assert_eq!(host_cpu, 41_142);
        assert_ne!(plan_cpu, host_cpu, "variant must not equal host default");
    }

    #[test]
    fn default_budget_is_the_host_default() {
        let budget = Environment::default().make_budget().unwrap();
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 100_000_000);
        assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 41_943_040);
    }

    /// `Default` and `ProtocolInitial` are distinct plans with distinct
    /// limits — regression guard for the variant dispatch.
    #[test]
    fn default_and_protocol_initial_are_distinct_plans() {
        let default = Environment::default().make_budget().unwrap();
        let initial = protocol_initial_budget();
        assert_ne!(
            default.get_cpu_insns_remaining().unwrap(),
            initial.get_cpu_insns_remaining().unwrap()
        );
        assert_ne!(
            default.get_mem_bytes_remaining().unwrap(),
            initial.get_mem_bytes_remaining().unwrap()
        );
        assert!(!BudgetPlan::Default.eq(&BudgetPlan::ProtocolInitial));
    }

    /// Runtime behavior for cost types beyond the 23-entry protocol-initial
    /// table. Source: `BudgetDimension::try_from_config` (soroban-env-host
    /// 28.0.2 `src/budget/dimension.rs`) only overwrites `cost_models[i]` for
    /// `i < cost_params.0.len()`; the array is sized
    /// `[MeteredCostComponent::default(); ContractCostType::variants().len()]`
    /// (`dimension.rs:50`), so every index >= 23 keeps the zero model
    /// (const 0, linear 0). `BudgetImpl::charge` (`budget.rs:235`) then
    /// charges `const_term * iterations` = 0 and never errors for an in-range
    /// index. So the variant is safe to run, but those cost types are free —
    /// the budget does not represent their protocol cost.
    #[test]
    fn cost_types_beyond_the_initial_table_are_charged_zero_not_errored() {
        let budget = protocol_initial_budget();
        // ParseWasmInstructions is index 23 — the first cost type the
        // protocol-initial table does not cover. The host's own default model
        // charges it (const 73077, linear 25410), so this is a real contrast.
        let extra = ContractCostType::ParseWasmInstructions;
        assert!(
            (extra as usize) >= NETWORK_COST_ENTRY_COUNT,
            "test requires a cost type outside the 23-entry table"
        );
        // Charging must succeed (no error) and must consume nothing.
        budget.charge(extra, Some(1_000)).unwrap();
        assert_eq!(budget.get_cpu_insns_consumed().unwrap(), 0);
        assert_eq!(budget.get_mem_bytes_consumed().unwrap(), 0);
        assert_eq!(budget.get_cpu_insns_remaining().unwrap(), 2_500_000);
        assert_eq!(budget.get_mem_bytes_remaining().unwrap(), 2_000_000);
        let tracker = budget.get_tracker(extra).unwrap();
        assert_eq!(tracker.iterations, 1, "charge must hit this cost type");
        assert_eq!(tracker.cpu, 0, "uncovered cost type must be free");
        assert_eq!(tracker.mem, 0, "uncovered cost type must be free");
    }

    /// The same uncovered cost type is *not* free under the host default
    /// model — proving the zero charge above is a property of the
    /// protocol-initial table, not of the host.
    #[test]
    fn host_default_charges_the_uncovered_cost_type() {
        let host_default = Budget::default();
        let extra = ContractCostType::ParseWasmInstructions;
        host_default.charge(extra, Some(1_000)).unwrap();
        let tracker = host_default.get_tracker(extra).unwrap();
        assert_eq!(tracker.iterations, 1);
        assert!(
            tracker.cpu > 0,
            "host default must charge ParseWasmInstructions (const 73077)"
        );
    }

    /// The uncovered cost types are exactly the ones the host charges when it
    /// parses/instantiates a module through the V1 path
    /// (`VersionedContractCodeCostInputs::V1` in
    /// `src/vm/parsed_module.rs`). Ties the zero-charge behavior to real
    /// symbols rather than to the enum length alone.
    #[test]
    fn the_host_parse_cost_types_are_outside_the_initial_table() {
        for ct in [
            ContractCostType::ParseWasmInstructions,
            ContractCostType::ParseWasmFunctions,
            ContractCostType::ParseWasmGlobals,
            ContractCostType::ParseWasmTableEntries,
            ContractCostType::ParseWasmTypes,
            ContractCostType::ParseWasmDataSegments,
            ContractCostType::ParseWasmElemSegments,
            ContractCostType::ParseWasmImports,
            ContractCostType::ParseWasmExports,
            ContractCostType::ParseWasmDataSegmentBytes,
            ContractCostType::InstantiateWasmInstructions,
            ContractCostType::InstantiateWasmFunctions,
            ContractCostType::InstantiateWasmGlobals,
            ContractCostType::InstantiateWasmTableEntries,
            ContractCostType::InstantiateWasmTypes,
            ContractCostType::InstantiateWasmDataSegments,
            ContractCostType::InstantiateWasmElemSegments,
            ContractCostType::InstantiateWasmImports,
            ContractCostType::InstantiateWasmExports,
            ContractCostType::InstantiateWasmDataSegmentBytes,
        ] {
            assert!(
                (ct as usize) >= NETWORK_COST_ENTRY_COUNT,
                "{ct:?} must sit outside the 23-entry protocol-initial table"
            );
            // And each one must be free under the variant, charged under the
            // host default — the exact asymmetry the docs describes. The
            // input shape must match the model: `ParseWasm*`/`Instantiate*`
            // are linear models (Some input); a const-only model rejects
            // Some() with a Budget InternalError, so try both shapes and
            // keep whichever the host accepts. `InstantiateWasmTypes` is
            // (0, 0) in the host default too, so it is excluded from the
            // "must be charged" half of the assertion.
            let plan = protocol_initial_budget();
            let host_default = Budget::default();
            [None, Some(64u64)]
                .into_iter()
                .find(|input| {
                    plan.charge(ct, *input).is_ok() && host_default.charge(ct, *input).is_ok()
                })
                .expect("host must accept one input shape for this cost type");
            assert_eq!(
                plan.get_tracker(ct).unwrap().cpu,
                0,
                "{ct:?} must be free under ProtocolInitial"
            );
            let host_cpu = host_default.get_tracker(ct).unwrap().cpu;
            if ct != ContractCostType::InstantiateWasmTypes {
                assert!(
                    host_cpu > 0,
                    "{ct:?} must be charged by the host default model"
                );
            }
        }
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
