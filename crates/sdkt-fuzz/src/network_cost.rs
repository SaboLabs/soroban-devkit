//! Protocol-initial cost model for fuzz budgets.
//!
//! The CPU and memory cost-parameter tables embedded here are the
//! **protocol-initial** cost model: the values stellar-core writes into the
//! per-protocol `ConfigSetting` ledger entries when Soroban was first
//! enabled, as defined by `NetworkConfig.cpp::initialCpuCostParamsEntryForV20()`
//! and `initialMemCostParamsEntryForV20()`.
//!
//! ## What this is — and what it is not
//!
//! - It is the **initial** (V20) cost parameters plus the
//!   `InitialSorobanNetworkConfig` resource limits. Those limits are also the
//!   protocol **floor**: `MinimumSorobanNetworkConfig::TX_MAX_INSTRUCTIONS`
//!   and `MEMORY_LIMIT` are the values an upgrade must not go below.
//! - It is **not** a live Mainnet validator configuration, and it is **not**
//!   the Protocol 29 cost model. stellar-core *does* mutate the V20 table on
//!   later protocol upgrades — `updateCpuCostParamsEntryForV21` rewrites
//!   `VmCachedInstantiation` from `(451626, 45405)` to `(41142, 634)` and
//!   appends 21 new cost types, and V22/V25/V26 append more. So the 23-entry
//!   table below describes the protocol-initial state, not the current one.
//! - No RPC, no network access, no fetched state: the values are a fixed,
//!   vendored copy of the protocol configuration. Anything that needs the
//!   *current* network table must read it from a ledger, not from here.
//!
//! ## Cost types outside the table
//!
//! The vendored table is 23 entries, but the host's budget dimension is sized
//! `ContractCostType::variants().len()` and `Budget::try_from_configs` only
//! overwrites the first 23 models. Every cost type at index >= 23 therefore
//! keeps the zero model (const 0, linear 0) and is charged **zero** — it is
//! never an error and never a fallback, but it is also not priced. The host
//! charges several of those entries on real paths (`ParseWasm*` and
//! `InstantiateWasm*` during module parse/instantiation), so a
//! `ProtocolInitial` run is **cheaper than the network would be** for those
//! operations. Treat the variant as a protocol-initial *floor*, not as a
//! faithful total: it is useful for exercising the covered cost types and the
//! resource ceilings, not for reproducing the full network cost of a
//! contract. See `the_host_parse_cost_types_are_outside_the_initial_table` in
//! `environment.rs` for the executable statement of this behavior.
//!
//! ## Separate concerns
//!
//! The **host version** (soroban-env-host 28.0.2), the **protocol version**
//! (28, pinned by the host), the **cost parameters** (below), the
//! **resource limits** (below), the **ledger TTL** fields, and the
//! **transaction fee** model are all distinct. This module only carries the
//! cost parameters and the resource limits; nothing else is touched.
//!
//! The tables are the single source of truth for the
//! [`BudgetPlan::ProtocolInitial`](crate::environment::BudgetPlan::ProtocolInitial)
//! budget; they are not duplicated anywhere else.

use soroban_env_host::xdr::{
    ContractCostParamEntry, ContractCostParams, ContractCostType, ExtensionPoint,
};

/// stellar-core `InitialSorobanNetworkConfig::TX_MAX_INSTRUCTIONS`:
/// per-transaction CPU instruction ceiling. Also the
/// `MinimumSorobanNetworkConfig::TX_MAX_INSTRUCTIONS` protocol floor.
pub const NETWORK_CPU_LIMIT: u64 = 2_500_000;

/// stellar-core `InitialSorobanNetworkConfig::MEMORY_LIMIT`:
/// per-transaction memory byte ceiling. Also the
/// `MinimumSorobanNetworkConfig::MEMORY_LIMIT` protocol floor.
pub const NETWORK_MEM_LIMIT: u64 = 2_000_000;

/// Number of cost types the protocol-initial tables cover
/// (`WasmInsnExec` = 0 through `ChaCha20DrawBytes` = 22, inclusive).
pub const NETWORK_COST_ENTRY_COUNT: usize = 23;

/// stellar-core `NetworkConfig.cpp::initialCpuCostParamsEntryForV20()`
/// values, `(const_term, linear_term)` per cost type in XDR enum order
/// (index 0 = `WasmInsnExec` … index 22 = `ChaCha20DrawBytes`).
///
/// Note: `VmCachedInstantiation` (index 12) is the entry that V21 later
/// rewrites to `(41142, 634)`; the value here is the protocol-initial one.
const V20_CPU: [(u64, u64); NETWORK_COST_ENTRY_COUNT] = [
    (4, 0),
    (434, 16),
    (42, 16),
    (44, 16),
    (310, 0),
    (61, 0),
    (230, 29),
    (59052, 4001),
    (3738, 7012),
    (40253, 0),
    (377524, 4068),
    (451626, 45405),
    (451626, 45405),
    (1948, 0),
    (3766, 5969),
    (710, 0),
    (2315295, 0),
    (4404, 0),
    (4947, 0),
    (4911, 0),
    (4286, 0),
    (913, 0),
    (1058, 501),
];

/// stellar-core `NetworkConfig.cpp::initialMemCostParamsEntryForV20()`
/// values, `(const_term, linear_term)` per cost type in the same XDR enum
/// order as [`V20_CPU`].
const V20_MEM: [(u64, u64); NETWORK_COST_ENTRY_COUNT] = [
    (0, 0),
    (16, 128),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
    (242, 384),
    (0, 384),
    (0, 0),
    (0, 0),
    (0, 0),
    (130065, 5064),
    (130065, 5064),
    (14, 0),
    (0, 0),
    (0, 0),
    (181, 0),
    (99, 0),
    (99, 0),
    (99, 0),
    (99, 0),
    (99, 0),
    (0, 0),
];

/// Build a `ContractCostParams` from a fixed `(const, linear)` table.
///
/// Fails (instead of silently falling back) when a table index does not map
/// to a valid `ContractCostType` or when the XDR conversion rejects the
/// entries.
fn params_from_table(
    table: &[(u64, u64); NETWORK_COST_ENTRY_COUNT],
) -> Result<ContractCostParams, String> {
    let mut entries: Vec<ContractCostParamEntry> = Vec::with_capacity(table.len());
    for (i, (const_term, linear_term)) in table.iter().enumerate() {
        let cost_type = ContractCostType::try_from(i as i32)
            .map_err(|_| format!("cost type index {i} is not a valid ContractCostType"))?;
        // The table order must match the XDR enum order exactly. A mismatch
        // here means the vendored table drifted from stellar-core's
        // initialCpuCostParamsEntryForV20/initialMemCostParamsEntryForV20
        // — fail loudly rather than charge the wrong cost type.
        let expected = match i {
            0 => "WasmInsnExec",
            1 => "MemAlloc",
            2 => "MemCpy",
            3 => "MemCmp",
            4 => "DispatchHostFunction",
            5 => "VisitObject",
            6 => "ValSer",
            7 => "ValDeser",
            8 => "ComputeSha256Hash",
            9 => "ComputeEd25519PubKey",
            10 => "VerifyEd25519Sig",
            11 => "VmInstantiation",
            12 => "VmCachedInstantiation",
            13 => "InvokeVmFunction",
            14 => "ComputeKeccak256Hash",
            15 => "DecodeEcdsaCurve256Sig",
            16 => "RecoverEcdsaSecp256k1Key",
            17 => "Int256AddSub",
            18 => "Int256Mul",
            19 => "Int256Div",
            20 => "Int256Pow",
            21 => "Int256Shift",
            22 => "ChaCha20DrawBytes",
            other => {
                unreachable!("table is fixed at {NETWORK_COST_ENTRY_COUNT} entries, got {other}")
            }
        };
        if cost_type.name() != expected {
            return Err(format!(
                "cost type order mismatch at index {i}: expected {expected}, got {}",
                cost_type.name()
            ));
        }
        entries.push(ContractCostParamEntry {
            ext: ExtensionPoint::V0,
            const_term: *const_term as i64,
            linear_term: *linear_term as i64,
        });
    }
    ContractCostParams::try_from(entries).map_err(|e| format!("cost params conversion: {e}"))
}

/// The protocol-initial CPU cost-parameter table.
pub fn network_cpu_cost_params() -> Result<ContractCostParams, String> {
    params_from_table(&V20_CPU)
}

/// The protocol-initial memory cost-parameter table.
pub fn network_mem_cost_params() -> Result<ContractCostParams, String> {
    params_from_table(&V20_MEM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_table_has_expected_entry_count_and_representative_values() {
        let params = network_cpu_cost_params().unwrap();
        assert_eq!(params.0.len(), NETWORK_COST_ENTRY_COUNT);
        // Representative spot-checks against stellar-core
        // initialCpuCostParamsEntryForV20.
        assert_eq!(params.0[0].const_term, 4, "WasmInsnExec const");
        assert_eq!(params.0[0].linear_term, 0, "WasmInsnExec linear");
        assert_eq!(params.0[1].const_term, 434, "MemAlloc const");
        assert_eq!(params.0[1].linear_term, 16, "MemAlloc linear");
        assert_eq!(params.0[11].const_term, 451626, "VmInstantiation const");
        assert_eq!(params.0[11].linear_term, 45405, "VmInstantiation linear");
        assert_eq!(params.0[22].const_term, 1058, "ChaCha20DrawBytes const");
        assert_eq!(params.0[22].linear_term, 501, "ChaCha20DrawBytes linear");
    }

    #[test]
    fn mem_table_has_expected_entry_count_and_representative_values() {
        let params = network_mem_cost_params().unwrap();
        assert_eq!(params.0.len(), NETWORK_COST_ENTRY_COUNT);
        assert_eq!(params.0[1].const_term, 16, "MemAlloc const");
        assert_eq!(params.0[1].linear_term, 128, "MemAlloc linear");
        assert_eq!(params.0[6].const_term, 242, "ValSer const");
        assert_eq!(params.0[6].linear_term, 384, "ValSer linear");
        assert_eq!(params.0[7].const_term, 0, "ValDeser const");
        assert_eq!(params.0[7].linear_term, 384, "ValDeser linear");
        assert_eq!(params.0[11].const_term, 130065, "VmInstantiation const");
        assert_eq!(params.0[11].linear_term, 5064, "VmInstantiation linear");
    }

    #[test]
    fn limits_match_stellar_core_initial_network_config() {
        assert_eq!(NETWORK_CPU_LIMIT, 2_500_000);
        assert_eq!(NETWORK_MEM_LIMIT, 2_000_000);
    }

    /// The table is protocol-initial, not current: stellar-core's V21 update
    /// rewrites `VmCachedInstantiation` and appends 21 cost types. Guard
    /// against the table silently claiming to be the current model.
    #[test]
    fn table_is_protocol_initial_not_the_current_protocol_model() {
        let params = network_cpu_cost_params().unwrap();
        // The XDR enum carries far more cost types than the initial table.
        assert!(
            ContractCostType::variants().len() > NETWORK_COST_ENTRY_COUNT,
            "newer cost types exist: the vendored table is protocol-initial, \
             not the current protocol model"
        );
        // VmCachedInstantiation keeps its protocol-initial value here, which
        // is the entry V21 rewrites to (41142, 634).
        assert_eq!(
            (params.0[12].const_term, params.0[12].linear_term),
            (451626, 45405),
            "VmCachedInstantiation must stay protocol-initial (V21 rewrites it)"
        );
    }
}
