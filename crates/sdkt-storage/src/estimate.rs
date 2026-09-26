//! Offline storage rent estimation for Soroban WASM contracts.
//!
//! Computes an offline baseline estimate of storage extension cost derived from the
//! parsed [`sdkt_wasm::ContractSpec`] ABI, using the rent approximation formula
//! from [`sdkt_rpc::storage::calculate_extension_cost`].
//!
//! # Ceiling and Honest Derivation
//!
//! An offline static analyzer reading only the WASM ABI can guarantee the contract
//! instance singleton (1 entry), and observe declared custom data types (UDTs) that
//! imply persistent storage schemas. It cannot predict dynamic runtime storage
//! growth (e.g. arbitrary user account entries or map keys created by transaction
//! execution) or ephemeral temporary storage usage.
//!
//! The estimate therefore represents a **structural lower-bound baseline** for the
//! contract's initial footprint over the specified ledger duration.

use sdkt_rpc::storage::calculate_extension_cost;
use sdkt_wasm::ContractSpec;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Default ledger horizon: 17,280 ledgers (~1 day at ~5 seconds per ledger).
pub const DEFAULT_ESTIMATE_LEDGERS: u32 = 17_280;

/// Conversion constant: 1 XLM = 10,000,000 stroops (10^7).
pub const STROOPS_PER_XLM: u64 = 10_000_000;

/// Format stroops into human-readable XLM decimal string without trailing zeroes.
pub fn format_stroops_to_xlm(stroops: u64) -> String {
    let whole = stroops / STROOPS_PER_XLM;
    let frac = stroops % STROOPS_PER_XLM;
    if frac == 0 {
        format!("{whole}")
    } else {
        let s = format!("{whole}.{frac:07}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Breakdown for a single storage class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassEstimate {
    /// Baseline number of storage entries implied for this class.
    pub entry_count: u64,
    /// Estimated extension cost in stroops for this class over the specified ledger duration.
    pub cost_stroops: u64,
    /// Estimated extension cost in XLM formatted as a decimal string.
    pub cost_xlm: String,
    /// Machine-readable derivation category tag.
    pub derivation: String,
    /// Explicit human-readable explanation of precision and derivation limits.
    pub notes: String,
}

/// Breakdown across the three Soroban storage classes (Instance, Persistent, Temporary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageClassesEstimate {
    pub instance: ClassEstimate,
    pub persistent: ClassEstimate,
    pub temporary: ClassEstimate,
}

/// Aggregated total estimate across all storage classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotalEstimate {
    /// Total baseline entries across all classes.
    pub baseline_entries: u64,
    /// Total estimated extension cost in stroops.
    pub cost_stroops: u64,
    /// Total estimated extension cost in XLM.
    pub cost_xlm: String,
}

/// Contract spec complexity metrics extracted from the WASM ABI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecMetrics {
    pub functions_count: usize,
    pub custom_types_count: usize,
    pub events_count: usize,
}

/// Full offline storage cost estimate report for a Soroban contract WASM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageCostEstimate {
    /// File path of the WASM analyzed.
    pub wasm_path: String,
    /// Ledger duration over which the extension cost is estimated.
    pub ledgers: u32,
    /// Extension cost per storage entry for the specified ledger duration in stroops.
    pub cost_per_entry_stroops: u64,
    /// Extension cost per storage entry in XLM.
    pub cost_per_entry_xlm: String,
    /// Per-storage-class breakdown.
    pub classes: StorageClassesEstimate,
    /// Aggregated total across classes.
    pub total: TotalEstimate,
    /// Spec metrics (functions, UDTs, events).
    pub spec_metrics: SpecMetrics,
    /// Explicit documentation of approximation formula and runtime ceiling.
    pub approximation_ceiling: String,
}

impl fmt::Display for StorageCostEstimate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Storage Cost Estimate for {}", self.wasm_path)?;
        writeln!(f, "{}", "=".repeat(60))?;

        let days = (self.ledgers as f64 * 5.0) / (24.0 * 3600.0);
        let days_str = if (days - days.round()).abs() < 0.001 {
            format!("{:.0} day(s)", days.round())
        } else if days >= 0.1 {
            format!("{:.1} day(s)", days)
        } else {
            format!("{:.2} day(s)", days)
        };

        writeln!(
            f,
            "Ledger Horizon: {} ledgers (~{} at 5s/ledger)",
            self.ledgers, days_str
        )?;
        writeln!(
            f,
            "Contract Spec:  {} function(s), {} custom type(s), {} event(s)",
            self.spec_metrics.functions_count,
            self.spec_metrics.custom_types_count,
            self.spec_metrics.events_count
        )?;
        writeln!(f)?;
        writeln!(f, "Storage Class Breakdown:")?;
        writeln!(f, "  Instance:")?;
        writeln!(f, "    Entries:     {}", self.classes.instance.entry_count)?;
        writeln!(
            f,
            "    Cost:        {} stroops ({} XLM)",
            self.classes.instance.cost_stroops, self.classes.instance.cost_xlm
        )?;
        writeln!(f, "    Derivation:  {}", self.classes.instance.derivation)?;
        writeln!(f, "    Note:        {}", self.classes.instance.notes)?;
        writeln!(f, "  Persistent:")?;
        writeln!(
            f,
            "    Entries:     {}",
            self.classes.persistent.entry_count
        )?;
        writeln!(
            f,
            "    Cost:        {} stroops ({} XLM)",
            self.classes.persistent.cost_stroops, self.classes.persistent.cost_xlm
        )?;
        writeln!(f, "    Derivation:  {}", self.classes.persistent.derivation)?;
        writeln!(f, "    Note:        {}", self.classes.persistent.notes)?;
        writeln!(f, "  Temporary:")?;
        writeln!(f, "    Entries:     {}", self.classes.temporary.entry_count)?;
        writeln!(
            f,
            "    Cost:        {} stroops ({} XLM)",
            self.classes.temporary.cost_stroops, self.classes.temporary.cost_xlm
        )?;
        writeln!(f, "    Derivation:  {}", self.classes.temporary.derivation)?;
        writeln!(f, "    Note:        {}", self.classes.temporary.notes)?;
        writeln!(f)?;
        writeln!(f, "Total Estimate:")?;
        writeln!(f, "  Baseline Entries: {}", self.total.baseline_entries)?;
        writeln!(
            f,
            "  Total Cost:       {} stroops ({} XLM)",
            self.total.cost_stroops, self.total.cost_xlm
        )?;
        writeln!(f)?;
        writeln!(f, "Approximation Ceiling & Limitations:")?;
        writeln!(f, "  {}", self.approximation_ceiling)?;
        Ok(())
    }
}

/// Compute an offline storage cost estimate from a parsed [`ContractSpec`].
///
/// This is a pure function over the parsed contract spec:
/// - **Instance**: 1 entry guaranteed for any deployed contract (the contract instance singleton).
/// - **Persistent**: Implied baseline lower bound from declared custom data types (structs, unions, enums),
///   excluding error enums. Dynamic runtime entries (e.g. user balances) cannot be predicted statically.
/// - **Temporary**: Ephemeral storage is invocation-dependent and cannot be derived statically (0 baseline).
/// - **Cost**: Derived using `sdkt_rpc::storage::calculate_extension_cost(ledgers)` per entry.
pub fn estimate_storage_from_spec(
    spec: &ContractSpec,
    wasm_path: &str,
    ledgers: u32,
) -> StorageCostEstimate {
    let cost_per_entry_stroops = calculate_extension_cost(ledgers);
    let cost_per_entry_xlm = format_stroops_to_xlm(cost_per_entry_stroops);

    // 1. Instance storage: every deployed contract has exactly 1 contract instance entry.
    let instance_entry_count = 1u64;
    let instance_cost_stroops = instance_entry_count * cost_per_entry_stroops;
    let instance = ClassEstimate {
        entry_count: instance_entry_count,
        cost_stroops: instance_cost_stroops,
        cost_xlm: format_stroops_to_xlm(instance_cost_stroops),
        derivation: "guaranteed_instance_singleton".to_string(),
        notes: "Contract instance singleton entry required for contract deployment".to_string(),
    };

    // 2. Persistent storage: custom data types (structs, unions, enums) define contract
    // data schemas / storage keys. Error enums define errors, not storage.
    let data_udts = spec
        .custom_types
        .iter()
        .filter(|t| t.kind != "error_enum")
        .count() as u64;

    let (persistent_entry_count, persistent_derivation, persistent_notes) = if data_udts > 0 {
        (
            data_udts,
            "implied_udt_baseline".to_string(),
            format!(
                "Baseline lower bound from {data_udts} declared data UDT(s); dynamic runtime entries cannot be determined from spec alone"
            ),
        )
    } else {
        (
            0,
            "zero_udts_declared".to_string(),
            "No data UDTs declared in spec; dynamic runtime entries cannot be determined from spec alone".to_string(),
        )
    };
    let persistent_cost_stroops = persistent_entry_count * cost_per_entry_stroops;
    let persistent = ClassEstimate {
        entry_count: persistent_entry_count,
        cost_stroops: persistent_cost_stroops,
        cost_xlm: format_stroops_to_xlm(persistent_cost_stroops),
        derivation: persistent_derivation,
        notes: persistent_notes,
    };

    // 3. Temporary storage: ephemeral and invocation-dependent; cannot be derived offline.
    let temporary = ClassEstimate {
        entry_count: 0,
        cost_stroops: 0,
        cost_xlm: "0".to_string(),
        derivation: "ephemeral_runtime_only".to_string(),
        notes: "Temporary storage entries are ephemeral and invocation-dependent; cannot be derived from spec alone".to_string(),
    };

    // Total aggregation
    let baseline_entries = instance.entry_count + persistent.entry_count + temporary.entry_count;
    let total_cost_stroops =
        instance.cost_stroops + persistent.cost_stroops + temporary.cost_stroops;
    let total_cost_xlm = format_stroops_to_xlm(total_cost_stroops);

    let total = TotalEstimate {
        baseline_entries,
        cost_stroops: total_cost_stroops,
        cost_xlm: total_cost_xlm,
    };

    let spec_metrics = SpecMetrics {
        functions_count: spec.functions.len(),
        custom_types_count: spec.custom_types.len(),
        events_count: spec.events.len(),
    };

    let approximation_ceiling = "Offline baseline estimate derived from contractspecv0 ABI and calculate_extension_cost (100 stroops/ledger/entry). Live storage scales with runtime state modifications; use 'sdkt storage analyze' for live on-chain storage.".to_string();

    StorageCostEstimate {
        wasm_path: wasm_path.to_string(),
        ledgers,
        cost_per_entry_stroops,
        cost_per_entry_xlm,
        classes: StorageClassesEstimate {
            instance,
            persistent,
            temporary,
        },
        total,
        spec_metrics,
        approximation_ceiling,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdkt_wasm::spec::TypeMember;
    use sdkt_wasm::{ContractFunction, ContractParameter, ContractType};

    fn make_test_spec(
        function_names: &[&str],
        custom_types: Vec<(&str, &str)>,
        event_names: &[&str],
    ) -> ContractSpec {
        let functions = function_names
            .iter()
            .map(|name| ContractFunction {
                name: (*name).to_string(),
                doc: String::new(),
                parameters: vec![ContractParameter {
                    name: "arg".to_string(),
                    doc: String::new(),
                    type_: ContractType {
                        name: "u32".to_string(),
                        kind: "primitive".to_string(),
                        doc: String::new(),
                        members: vec![],
                    },
                }],
                outputs: vec![],
            })
            .collect();

        let custom_types = custom_types
            .into_iter()
            .map(|(name, kind)| ContractType {
                name: name.to_string(),
                kind: kind.to_string(),
                doc: String::new(),
                members: vec![TypeMember {
                    name: "field".to_string(),
                    doc: String::new(),
                }],
            })
            .collect();

        let events = event_names
            .iter()
            .map(|name| sdkt_wasm::ContractEvent {
                name: (*name).to_string(),
                doc: String::new(),
            })
            .collect();

        ContractSpec {
            env_meta: None,
            functions,
            custom_types,
            events,
        }
    }

    #[test]
    fn test_format_stroops_to_xlm() {
        assert_eq!(format_stroops_to_xlm(0), "0");
        assert_eq!(format_stroops_to_xlm(10_000_000), "1");
        assert_eq!(format_stroops_to_xlm(25_000_000), "2.5");
        assert_eq!(format_stroops_to_xlm(1_728_000), "0.1728");
        assert_eq!(format_stroops_to_xlm(3_456_000), "0.3456");
        assert_eq!(format_stroops_to_xlm(100), "0.00001");
        assert_eq!(format_stroops_to_xlm(1), "0.0000001");
    }

    #[test]
    fn test_estimate_empty_spec() {
        let spec = make_test_spec(&[], vec![], &[]);
        let est = estimate_storage_from_spec(&spec, "test.wasm", DEFAULT_ESTIMATE_LEDGERS);

        assert_eq!(est.wasm_path, "test.wasm");
        assert_eq!(est.ledgers, 17_280);
        assert_eq!(est.cost_per_entry_stroops, 1_728_000);
        assert_eq!(est.cost_per_entry_xlm, "0.1728");

        // Instance: 1 guaranteed singleton
        assert_eq!(est.classes.instance.entry_count, 1);
        assert_eq!(est.classes.instance.cost_stroops, 1_728_000);
        assert_eq!(est.classes.instance.cost_xlm, "0.1728");
        assert_eq!(
            est.classes.instance.derivation,
            "guaranteed_instance_singleton"
        );

        // Persistent: 0 (no UDTs)
        assert_eq!(est.classes.persistent.entry_count, 0);
        assert_eq!(est.classes.persistent.cost_stroops, 0);
        assert_eq!(est.classes.persistent.cost_xlm, "0");
        assert_eq!(est.classes.persistent.derivation, "zero_udts_declared");

        // Temporary: 0
        assert_eq!(est.classes.temporary.entry_count, 0);
        assert_eq!(est.classes.temporary.cost_stroops, 0);
        assert_eq!(est.classes.temporary.cost_xlm, "0");

        // Total
        assert_eq!(est.total.baseline_entries, 1);
        assert_eq!(est.total.cost_stroops, 1_728_000);
        assert_eq!(est.total.cost_xlm, "0.1728");

        // Spec metrics
        assert_eq!(est.spec_metrics.functions_count, 0);
        assert_eq!(est.spec_metrics.custom_types_count, 0);
        assert_eq!(est.spec_metrics.events_count, 0);
    }

    #[test]
    fn test_estimate_spec_with_udts() {
        let spec = make_test_spec(
            &["transfer", "balance"],
            vec![("Balance", "struct"), ("DataKey", "enum")],
            &["TransferEvent"],
        );
        let est = estimate_storage_from_spec(&spec, "token.wasm", DEFAULT_ESTIMATE_LEDGERS);

        // Instance: 1, Persistent: 2, Temporary: 0 -> Total: 3
        assert_eq!(est.classes.instance.entry_count, 1);
        assert_eq!(est.classes.persistent.entry_count, 2);
        assert_eq!(est.classes.persistent.derivation, "implied_udt_baseline");
        assert_eq!(est.classes.temporary.entry_count, 0);

        assert_eq!(est.total.baseline_entries, 3);
        assert_eq!(est.total.cost_stroops, 3 * 1_728_000);
        assert_eq!(est.total.cost_xlm, format_stroops_to_xlm(3 * 1_728_000));

        assert_eq!(est.spec_metrics.functions_count, 2);
        assert_eq!(est.spec_metrics.custom_types_count, 2);
        assert_eq!(est.spec_metrics.events_count, 1);
    }

    #[test]
    fn test_estimate_excludes_error_enums_from_persistent_storage() {
        let spec = make_test_spec(
            &["foo"],
            vec![("Error", "error_enum"), ("Record", "struct")],
            &[],
        );
        let est = estimate_storage_from_spec(&spec, "app.wasm", 100);

        // Cost per entry for 100 ledgers = 100 * 100 = 10,000 stroops
        assert_eq!(est.cost_per_entry_stroops, 10_000);
        assert_eq!(est.cost_per_entry_xlm, "0.001");

        // Error enum is excluded from persistent data UDT count -> 1 struct only
        assert_eq!(est.classes.persistent.entry_count, 1);
        assert_eq!(est.classes.persistent.cost_stroops, 10_000);
        assert_eq!(est.classes.persistent.cost_xlm, "0.001");

        // Total: 1 instance + 1 persistent = 2
        assert_eq!(est.total.baseline_entries, 2);
        assert_eq!(est.total.cost_stroops, 20_000);
        assert_eq!(est.total.cost_xlm, "0.002");
    }

    #[test]
    fn test_pretty_display_output() {
        let spec = make_test_spec(&["hello"], vec![("State", "struct")], &[]);
        let est = estimate_storage_from_spec(&spec, "hello.wasm", 17_280);
        let text = est.to_string();

        assert!(text.contains("Storage Cost Estimate for hello.wasm"));
        assert!(text.contains("Ledger Horizon: 17280 ledgers (~1 day(s) at 5s/ledger)"));
        assert!(text.contains("Contract Spec:  1 function(s), 1 custom type(s), 0 event(s)"));
        assert!(text.contains("Instance:"));
        assert!(text.contains("1728000 stroops (0.1728 XLM)"));
        assert!(text.contains("Persistent:"));
        assert!(text.contains("Temporary:"));
        assert!(text.contains("Baseline Entries: 2"));
        assert!(text.contains("Total Cost:       3456000 stroops (0.3456 XLM)"));
        assert!(text.contains("Approximation Ceiling & Limitations:"));
    }

    #[test]
    fn test_json_serialization_roundtrip() {
        let spec = make_test_spec(&["test"], vec![("Data", "struct")], &[]);
        let est = estimate_storage_from_spec(&spec, "contract.wasm", 17_280);
        let json_str = serde_json::to_string(&est).expect("serialization succeeds");
        let parsed: StorageCostEstimate =
            serde_json::from_str(&json_str).expect("deserialization succeeds");
        assert_eq!(est, parsed);
    }
}
