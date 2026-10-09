//! Network-faithful Protocol 29 cost model for fuzz budgets.
//!
//! The CPU and memory cost-parameter tables embedded here are the
//! **network-faithful Protocol 29 cost model derived from stellar-core
//! protocol configuration**: the initial per-protocol `ConfigSetting`
//! ledger entries as defined by stellar-core's
//! `NetworkConfig.cpp::initialCpuCostParamsEntryForV20()` and
//! `initialMemCostParamsEntryForV20()`. Those 23-entry tables (one entry per
//! cost type `WasmInsnExec` .. `ChaCha20DrawBytes`) are the values the
//! network carried when Protocol 29 activated, and are the table this
//! `BudgetPlan::NetworkFaithful` budget is built from.
//!
//! ## What this is and is not
//!
//! - It is the **cost parameters** (per-cost-type linear models) plus the
//!   **resource limits** (`txMaxInstructions`, `txMemoryLimit`) from
//!   stellar-core's `InitialSorobanNetworkConfig`.
//! - It is **not** a live Mainnet validator configuration. No RPC, no
//!   network access, no fetched state — the values are a fixed, vendored
//!   copy of the protocol configuration.
//! - The **host version** (soroban-env-host 28.0.2), the **protocol
//!   version** (28, pinned by the host), the **ledger TTL** fields, and the
//!   **transaction fee** model are all separate concerns and are untouched
//!   by this module.
//!
//! The tables are the single source of truth for the
//! [`BudgetPlan::NetworkFaithful`](crate::environment::BudgetPlan::NetworkFaithful)
//! budget; they are not duplicated anywhere else.

use soroban_env_host::xdr::{
    ContractCostParamEntry, ContractCostParams, ContractCostType, ExtensionPoint,
};

/// stellar-core `InitialSorobanNetworkConfig::TX_MAX_INSTRUCTIONS`:
/// per-transaction CPU instruction ceiling.
pub const NETWORK_CPU_LIMIT: u64 = 2_500_000;

/// stellar-core `InitialSorobanNetworkConfig::MEMORY_LIMIT`:
/// per-transaction memory byte ceiling.
pub const NETWORK_MEM_LIMIT: u64 = 2_000_000;

/// Number of cost types the Protocol 29 initial tables cover
/// (`WasmInsnExec` = 0 through `ChaCha20DrawBytes` = 22, inclusive).
pub const NETWORK_COST_ENTRY_COUNT: usize = 23;

/// stellar-core `NetworkConfig.cpp::initialCpuCostParamsEntryForV20()`
/// values, `(const_term, linear_term)` per cost type in XDR enum order
/// (index 0 = `WasmInsnExec` … index 22 = `ChaCha20DrawBytes`).
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
        // `initialCpuCostParamsEntryForV20`/`initialMemCostParamsEntryForV20`
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

/// The network-faithful Protocol 29 CPU cost-parameter table.
pub fn network_cpu_cost_params() -> Result<ContractCostParams, String> {
    params_from_table(&V20_CPU)
}

/// The network-faithful Protocol 29 memory cost-parameter table.
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
}
