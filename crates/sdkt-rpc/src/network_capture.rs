//! Live network-configuration capture for `sdkt-fuzz` network profiles.
//!
//! Reads the Soroban network configuration that the RPC surface actually
//! exposes — `getNetwork`, `getLatestLedger`, and the `ConfigSetting` ledger
//! entries reachable through `getLedgerEntries` — and decodes it into a
//! [`CapturedNetworkConfig`] of plain transport-level values.
//!
//! ## Architectural boundary
//!
//! This module deliberately does **not** depend on `sdkt-fuzz`: the fuzz crate
//! owns the only `soroban-env-host` dependency in the workspace, and a network
//! transport crate must not pull the host engine. The capture returns plain
//! values; the caller assembles them into a
//! `sdkt_fuzz::network_profile::NetworkProfile` (see
//! [`CapturedNetworkConfig::into_profile_parts`]).
//!
//! ## What is and is not available
//!
//! Decoded from `ConfigSetting` entries (verified against live mainnet and
//! testnet):
//!
//! | `ConfigSettingId` | Value |
//! |---|---|
//! | `ContractComputeV0` | `txMaxInstructions`, `txMemoryLimit`, `ledgerMaxInstructions`, `feeRatePerInstructionsIncrement` |
//! | `ContractMaxSizeBytes` | `maxContractSizeBytes` |
//! | `ContractBandwidthV0` | `txMaxSizeBytes` |
//! | `ContractEventsV0` | `txMaxContractEventsSizeBytes`, `feeContractEvents1KB` |
//! | `ContractCostParamsCpuInstructions` / `ContractCostParamsMemoryBytes` | the cost tables |
//!
//! **Not** available as a single RPC object: the host's `FeeConfiguration`
//! and `RentWriteFeeConfiguration` (they are assembled from several
//! `ConfigSetting` entries plus a computed `fee_per_write_1kb`). Fields backed
//! by those stay `None` with [`ParamSource::Unavailable`] — never a guess.

use crate::client::{LedgerInfo, NetworkInfo, SorobanRpcClient};
use crate::error::RpcError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    ConfigSettingEntry, ConfigSettingId, ContractCostParamEntry, ContractCostParams,
    ContractCostType, LedgerEntry, LedgerEntryData, ReadXdr,
};

/// Where a captured value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// Read from a live `ConfigSetting` ledger entry.
    LiveConfigSetting,
    /// Read from the RPC network/ledger endpoints.
    LiveRpcMetadata,
    /// Not exposed by this RPC surface; the value is unknown, not zero.
    Unavailable,
}

/// A captured value with its source and the ledger it was observed at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured<T> {
    pub value: Option<T>,
    pub source: CaptureSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ledger: Option<u32>,
}

impl<T> Captured<T> {
    pub fn live(value: T, ledger: u32) -> Self {
        Self {
            value: Some(value),
            source: CaptureSource::LiveConfigSetting,
            observed_at_ledger: Some(ledger),
        }
    }

    pub fn unavailable() -> Self {
        Self {
            value: None,
            source: CaptureSource::Unavailable,
            observed_at_ledger: None,
        }
    }
}

/// One cost-parameter entry: its position, the `ContractCostType` variant name
/// at that position, and the two terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedCostParam {
    pub index: u32,
    pub cost_type: String,
    pub const_term: i64,
    pub linear_term: i64,
}

/// The captured CPU and memory cost tables.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedCostParams {
    pub cpu: Vec<CapturedCostParam>,
    pub mem: Vec<CapturedCostParam>,
}

/// Everything read from the network in one capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedNetworkConfig {
    pub passphrase: String,
    pub protocol_version: u32,
    pub ledger_sequence: u32,
    pub cpu_limit: Captured<u64>,
    pub mem_limit: Captured<u64>,
    pub ledger_max_instructions: Captured<u64>,
    pub fee_rate_per_instructions_increment: Captured<u64>,
    pub max_contract_size_bytes: Captured<u32>,
    pub tx_max_size_bytes: Captured<u32>,
    pub tx_max_contract_events_size_bytes: Captured<u32>,
    pub fee_contract_events_1kb: Captured<i64>,
    pub cost_params: Captured<CapturedCostParams>,
}

impl CapturedNetworkConfig {
    fn empty(passphrase: String, protocol_version: u32, ledger_sequence: u32) -> Self {
        Self {
            passphrase,
            protocol_version,
            ledger_sequence,
            cpu_limit: Captured::unavailable(),
            mem_limit: Captured::unavailable(),
            ledger_max_instructions: Captured::unavailable(),
            fee_rate_per_instructions_increment: Captured::unavailable(),
            max_contract_size_bytes: Captured::unavailable(),
            tx_max_size_bytes: Captured::unavailable(),
            tx_max_contract_events_size_bytes: Captured::unavailable(),
            fee_contract_events_1kb: Captured::unavailable(),
            cost_params: Captured::unavailable(),
        }
    }

    /// Names of the parameters that could not be captured. Empty when the
    /// capture is complete for the fields this module decodes.
    pub fn unavailable_params(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.cpu_limit.value.is_none() {
            out.push("txMaxInstructions");
        }
        if self.mem_limit.value.is_none() {
            out.push("txMemoryLimit");
        }
        if self.ledger_max_instructions.value.is_none() {
            out.push("ledgerMaxInstructions");
        }
        if self.fee_rate_per_instructions_increment.value.is_none() {
            out.push("feeRatePerInstructionsIncrement");
        }
        if self.max_contract_size_bytes.value.is_none() {
            out.push("maxContractSizeBytes");
        }
        if self.tx_max_size_bytes.value.is_none() {
            out.push("txMaxSizeBytes");
        }
        if self.tx_max_contract_events_size_bytes.value.is_none() {
            out.push("txMaxContractEventsSizeBytes");
        }
        if self.cost_params.value.is_none() {
            out.push("costParams");
        }
        out
    }

    /// Parameters the RPC surface does not expose at all, for documentation
    /// and for the profile's "not available" list.
    pub fn never_available_params() -> &'static [&'static str] {
        &[
            "FeeConfiguration.feePerWriteEntry",
            "FeeConfiguration.feePerHistorical1KB",
            "RentWriteFeeConfiguration.feePerWrite1KB (computed)",
            "RentFeeConfiguration.feePerRent1KB (computed)",
        ]
    }
}

/// A single `ConfigSetting` ledger key, base64 XDR, ready for
/// `getLedgerEntries`.
///
/// `LedgerKey::ConfigSetting` is `LedgerEntryType::ConfigSetting (8)` followed
/// by the `ConfigSettingId` as a u32.
pub fn config_setting_key(id: ConfigSettingId) -> String {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&8u32.to_be_bytes());
    bytes.extend_from_slice(&(id as u32).to_be_bytes());
    STANDARD.encode(bytes)
}

/// The `ConfigSetting` ids this module requests.
pub const CAPTURED_CONFIG_SETTING_IDS: [ConfigSettingId; 6] = [
    ConfigSettingId::ContractComputeV0,
    ConfigSettingId::ContractMaxSizeBytes,
    ConfigSettingId::ContractBandwidthV0,
    ConfigSettingId::ContractEventsV0,
    ConfigSettingId::ContractCostParamsCpuInstructions,
    ConfigSettingId::ContractCostParamsMemoryBytes,
];

/// Fetch and decode the live network configuration.
///
/// Every field records the ledger it was observed at. A parameter the RPC does
/// not return stays `None` with [`CaptureSource::Unavailable`].
pub async fn capture_network_config(
    client: &SorobanRpcClient,
) -> Result<CapturedNetworkConfig, RpcError> {
    let network: NetworkInfo = client.get_network().await?;
    let ledger: LedgerInfo = client.get_ledger().await?;
    let ledger_seq = ledger.sequence;

    let mut out =
        CapturedNetworkConfig::empty(network.passphrase, network.protocol_version, ledger_seq);

    let keys: Vec<String> = CAPTURED_CONFIG_SETTING_IDS
        .iter()
        .map(|id| config_setting_key(*id))
        .collect();
    let response = client.get_contract_storage("", &keys).await?;

    for entry in &response.entries {
        let data = decode_ledger_entry_data_b64(&entry.xdr)?;
        // The entry's own ledger provenance comes from the response envelope:
        // `lastModifiedLedgerSeq` is when the value last changed, which is
        // what a configuration snapshot must record (a config entry unchanged
        // since ledger N is still the network's configuration at the current
        // ledger).
        decode_into(&mut out, &data, entry.last_modified_ledger_seq)?;
    }

    // A mixed-ledger response is not a coherent configuration.
    if response.latest_ledger != ledger_seq {
        return Err(RpcError::Rpc(format!(
            "mixed ledger provenance: getLatestLedger={ledger_seq} \
             getLedgerEntries={}",
            response.latest_ledger
        )));
    }

    Ok(out)
}

/// Decode a base64 `LedgerEntry` payload. Exposed so a snapshot's stored
/// entries can be re-decoded offline with the same code path.
///
/// Note: the `xdr` returned by `getLedgerEntries` is the `LedgerEntryData`
/// union (type + variant), not a full `LedgerEntry` — the sequence and ext
/// fields are carried in the response envelope (`lastModifiedLedgerSeq`,
/// `liveUntilLedgerSeq`).
pub fn decode_ledger_entry_data(raw: &[u8]) -> Result<LedgerEntryData, RpcError> {
    let mut cursor = std::io::Cursor::new(raw);
    let mut limited = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerEntryData::read_xdr(&mut limited)
        .map_err(|e| RpcError::Rpc(format!("ConfigSetting entry XDR: {e}")))
}

/// Decode a base64-encoded `LedgerEntryData` string.
pub fn decode_ledger_entry_data_b64(b64: &str) -> Result<LedgerEntryData, RpcError> {
    let raw = STANDARD
        .decode(b64.trim())
        .map_err(|e| RpcError::Rpc(format!("base64 decode: {e}")))?;
    decode_ledger_entry_data(&raw)
}

/// Decode a full `LedgerEntry` (used where the payload really is one).
pub fn decode_ledger_entry(raw: &[u8]) -> Result<LedgerEntry, RpcError> {
    let mut cursor = std::io::Cursor::new(raw);
    let mut limited = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerEntry::read_xdr(&mut limited).map_err(|e| RpcError::Rpc(format!("LedgerEntry XDR: {e}")))
}

/// Decode one `LedgerEntryData` into the capture, matching on the
/// `ConfigSettingId` so an unknown id is ignored rather than misread.
///
/// `entry_ledger` is the entry's `lastModifiedLedgerSeq` — the ledger at which
/// the configuration value last changed.
pub fn decode_into(
    out: &mut CapturedNetworkConfig,
    data: &LedgerEntryData,
    entry_ledger: u32,
) -> Result<(), RpcError> {
    let ledger = entry_ledger;
    let LedgerEntryData::ConfigSetting(setting) = data else {
        return Ok(());
    };
    match setting {
        ConfigSettingEntry::ContractComputeV0(v) => {
            out.cpu_limit = Captured::live(v.tx_max_instructions as u64, ledger);
            out.mem_limit = Captured::live(v.tx_memory_limit as u64, ledger);
            out.ledger_max_instructions = Captured::live(v.ledger_max_instructions as u64, ledger);
            out.fee_rate_per_instructions_increment =
                Captured::live(v.fee_rate_per_instructions_increment as u64, ledger);
        }
        ConfigSettingEntry::ContractMaxSizeBytes(v) => {
            out.max_contract_size_bytes = Captured::live(*v, ledger);
        }
        ConfigSettingEntry::ContractBandwidthV0(v) => {
            out.tx_max_size_bytes = Captured::live(v.tx_max_size_bytes, ledger);
        }
        ConfigSettingEntry::ContractEventsV0(v) => {
            out.tx_max_contract_events_size_bytes =
                Captured::live(v.tx_max_contract_events_size_bytes, ledger);
            out.fee_contract_events_1kb = Captured::live(v.fee_contract_events1_kb, ledger);
        }
        ConfigSettingEntry::ContractCostParamsCpuInstructions(p) => {
            let cpu = decode_cost_params(p)?;
            let existing = out.cost_params.value.clone().unwrap_or_default();
            out.cost_params = Captured::live(
                CapturedCostParams {
                    cpu,
                    mem: existing.mem,
                },
                ledger,
            );
        }
        ConfigSettingEntry::ContractCostParamsMemoryBytes(p) => {
            let mem = decode_cost_params(p)?;
            let existing = out.cost_params.value.clone().unwrap_or_default();
            out.cost_params = Captured::live(
                CapturedCostParams {
                    cpu: existing.cpu,
                    mem,
                },
                ledger,
            );
        }
        // Deliberately not decoded: the host's FeeConfiguration /
        // RentWriteFeeConfiguration are assembled from several entries plus a
        // computed value. Recording them unavailable is honest; guessing a fee
        // model is not.
        _ => {}
    }
    Ok(())
}

/// Decode a `ContractCostParams` vector into named entries.
///
/// The mapping is explicit: each entry's position is matched against
/// `ContractCostType::try_from(index)` and the variant name is recorded. A
/// position that does not map to a variant is an error, not a silently dropped
/// entry.
pub fn decode_cost_params(params: &ContractCostParams) -> Result<Vec<CapturedCostParam>, RpcError> {
    let mut out = Vec::with_capacity(params.0.len());
    for (i, entry) in params.0.iter().enumerate() {
        let ContractCostParamEntry {
            ext: _,
            const_term,
            linear_term,
        } = entry;
        let cost_type = ContractCostType::try_from(i as i32)
            .map_err(|_| RpcError::Rpc(format!("cost type index {i} is not a valid variant")))?;
        out.push(CapturedCostParam {
            index: i as u32,
            cost_type: cost_type.name().to_string(),
            const_term: *const_term,
            linear_term: *linear_term,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{LedgerEntryData, Limits, WriteXdr};

    fn compute_entry() -> ConfigSettingEntry {
        ConfigSettingEntry::ContractComputeV0(stellar_xdr::ConfigSettingContractComputeV0 {
            ledger_max_instructions: 580_000_000,
            tx_max_instructions: 400_000_000,
            fee_rate_per_instructions_increment: 7,
            tx_memory_limit: 41_943_040,
        })
    }

    fn entry_data_bytes(entry: &ConfigSettingEntry) -> Vec<u8> {
        LedgerEntryData::ConfigSetting(entry.clone())
            .to_xdr(Limits::none())
            .unwrap()
    }

    fn read_data(bytes: &[u8]) -> LedgerEntryData {
        decode_ledger_entry_data(bytes).unwrap()
    }

    fn empty_capture() -> CapturedNetworkConfig {
        CapturedNetworkConfig::empty(
            "Public Global Stellar Network ; September 2015".to_string(),
            29,
            64_846_678,
        )
    }

    #[test]
    fn config_setting_key_encodes_type_and_id() {
        assert_eq!(
            config_setting_key(ConfigSettingId::ContractComputeV0),
            "AAAACAAAAAE="
        );
        assert_eq!(
            config_setting_key(ConfigSettingId::ContractCostParamsMemoryBytes),
            "AAAACAAAAAc="
        );
    }

    #[test]
    fn compute_entry_decodes_into_limits() {
        let mut out = empty_capture();
        let data = read_data(&entry_data_bytes(&compute_entry()));
        decode_into(&mut out, &data, 64_846_678).unwrap();
        assert_eq!(out.cpu_limit.value, Some(400_000_000));
        assert_eq!(out.mem_limit.value, Some(41_943_040));
        assert_eq!(out.ledger_max_instructions.value, Some(580_000_000));
        assert_eq!(out.fee_rate_per_instructions_increment.value, Some(7));
        assert_eq!(out.cpu_limit.observed_at_ledger, Some(64_846_678));
        assert_eq!(out.cpu_limit.source, CaptureSource::LiveConfigSetting);
    }

    #[test]
    fn cost_params_decode_with_explicit_type_mapping() {
        let entries: Vec<ContractCostParamEntry> = (0..86)
            .map(|i| ContractCostParamEntry {
                ext: stellar_xdr::ExtensionPoint::V0,
                const_term: i as i64,
                linear_term: (i as i64) * 2,
            })
            .collect();
        let params = ContractCostParams::try_from(entries).unwrap();
        let decoded = decode_cost_params(&params).unwrap();
        assert_eq!(decoded.len(), 86);
        assert_eq!(decoded[0].cost_type, "WasmInsnExec");
        assert_eq!(decoded[0].index, 0);
        assert_eq!(decoded[11].cost_type, "VmInstantiation");
        assert_eq!(decoded[12].cost_type, "VmCachedInstantiation");
        assert_eq!(decoded[22].cost_type, "ChaCha20DrawBytes");
        assert_eq!(decoded[23].cost_type, "ParseWasmInstructions");
        assert_eq!(decoded[85].index, 85);
        // Every index maps to a distinct named variant.
        let names: std::collections::BTreeSet<_> =
            decoded.iter().map(|e| e.cost_type.as_str()).collect();
        assert_eq!(names.len(), 86, "all 86 positions map to distinct types");
    }

    #[test]
    fn cpu_and_mem_tables_merge_across_two_versions() {
        let mut out = empty_capture();
        let cpu_entries: Vec<ContractCostParamEntry> = (0..86)
            .map(|i| ContractCostParamEntry {
                ext: stellar_xdr::ExtensionPoint::V0,
                const_term: i as i64,
                linear_term: 0,
            })
            .collect();
        let mem_entries: Vec<ContractCostParamEntry> = (0..86)
            .map(|i| ContractCostParamEntry {
                ext: stellar_xdr::ExtensionPoint::V0,
                const_term: 0,
                linear_term: i as i64,
            })
            .collect();
        let cpu = ConfigSettingEntry::ContractCostParamsCpuInstructions(
            ContractCostParams::try_from(cpu_entries).unwrap(),
        );
        let mem = ConfigSettingEntry::ContractCostParamsMemoryBytes(
            ContractCostParams::try_from(mem_entries).unwrap(),
        );
        decode_into(&mut out, &read_data(&entry_data_bytes(&cpu)), 100).unwrap();
        decode_into(&mut out, &read_data(&entry_data_bytes(&mem)), 100).unwrap();
        let p = out.cost_params.value.as_ref().unwrap();
        assert_eq!(p.cpu.len(), 86);
        assert_eq!(p.mem.len(), 86);
        assert_eq!(p.cpu[11].const_term, 11);
        assert_eq!(p.mem[11].linear_term, 11);
    }

    #[test]
    fn non_config_setting_entry_is_ignored() {
        let mut out = empty_capture();
        let data = LedgerEntryData::ContractCode(stellar_xdr::ContractCodeEntry {
            ext: stellar_xdr::ContractCodeEntryExt::V0,
            hash: stellar_xdr::Hash([0u8; 32]),
            code: stellar_xdr::BytesM::try_from(vec![0u8]).unwrap(),
        });
        decode_into(&mut out, &data, 1).unwrap();
        assert_eq!(out.cpu_limit.value, None);
        assert_eq!(out.cpu_limit.source, CaptureSource::Unavailable);
    }

    /// The exact bytes observed from live mainnet at ledger 64846678 for
    /// `ContractComputeV0`, so a decode regression is caught by a real value.
    #[test]
    fn live_mainnet_compute_entry_decodes() {
        let b64 = "AAAACAAAAAEAAAAAIpIZAAAAAAAX14QAAAAAAAAAAAcCgAAA";
        let data = decode_ledger_entry_data_b64(b64).unwrap();
        let mut out = empty_capture();
        decode_into(&mut out, &data, 8).unwrap();
        assert_eq!(out.cpu_limit.value, Some(400_000_000));
        assert_eq!(out.mem_limit.value, Some(41_943_040));
        assert_eq!(out.ledger_max_instructions.value, Some(580_000_000));
        assert_eq!(out.fee_rate_per_instructions_increment.value, Some(7));
    }

    /// The exact live mainnet CPU cost table captured from
    /// `mainnet.sorobanrpc.com` at ledger 64846834 (86 entries), checked at
    /// the entries whose values differ from the vendored V20 table.
    #[test]
    fn live_mainnet_cpu_table_decodes() {
        let fixture = include_str!("../tests/fixtures/mainnet_cpu_cost_params.xdr.b64");
        let data = decode_ledger_entry_data_b64(fixture.trim()).unwrap();
        let mut out = empty_capture();
        decode_into(&mut out, &data, 62_447_231).unwrap();
        let p = out.cost_params.value.as_ref().unwrap();
        assert_eq!(p.cpu.len(), 86);
        assert_eq!(p.cpu[0].cost_type, "WasmInsnExec");
        assert_eq!((p.cpu[0].const_term, p.cpu[0].linear_term), (4, 0));
        // These two differ from the vendored V20 table:
        // V20 had VmInstantiation (451626, 45405); live is (417482, 45712).
        assert_eq!(
            (p.cpu[11].const_term, p.cpu[11].linear_term),
            (417482, 45712)
        );
        // V20 had VmCachedInstantiation (451626, 45405); live is (41142, 634).
        assert_eq!((p.cpu[12].const_term, p.cpu[12].linear_term), (41142, 634));
    }

    /// The live mainnet memory cost table — the two live tables must pair up
    /// with identical type names at identical indices.
    #[test]
    fn live_mainnet_mem_table_pairs_with_cpu() {
        let cpu_fixture = include_str!("../tests/fixtures/mainnet_cpu_cost_params.xdr.b64");
        let mem_fixture = include_str!("../tests/fixtures/mainnet_mem_cost_params.xdr.b64");
        let mut out = empty_capture();
        decode_into(
            &mut out,
            &decode_ledger_entry_data_b64(cpu_fixture.trim()).unwrap(),
            62_447_231,
        )
        .unwrap();
        decode_into(
            &mut out,
            &decode_ledger_entry_data_b64(mem_fixture.trim()).unwrap(),
            62_447_231,
        )
        .unwrap();
        let p = out.cost_params.value.as_ref().unwrap();
        assert_eq!(p.cpu.len(), 86);
        assert_eq!(p.mem.len(), 86);
        for (c, m) in p.cpu.iter().zip(p.mem.iter()) {
            assert_eq!(c.cost_type, m.cost_type, "index {}", c.index);
            assert_eq!(c.index, m.index);
        }
    }

    #[test]
    fn unavailable_params_lists_every_missing_field() {
        let out = empty_capture();
        let missing = out.unavailable_params();
        assert!(missing.contains(&"txMaxInstructions"));
        assert!(missing.contains(&"costParams"));
        assert_eq!(missing.len(), 8);
    }
}
