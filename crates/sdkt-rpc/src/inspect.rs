use crate::client::SorobanRpcClient;
use crate::error::RpcError;
use crate::wasm::get_wasm_bytecode;
use sdkt_wasm::{parse_contract_spec, ContractSpec};
use sdkt_xdr::{
    decode_contract_id, encode_ledger_key, extract_contract_data_value,
    extract_contract_executable, parse_external_executable_hash, ContractExecutableRef,
    ExternalExecutableRef, LedgerKeyParams,
};
use serde::Deserialize;
use serde::Serialize;
use stellar_xdr::{ContractDataDurability, ScAddress, ScVal};

/// Normalize a user-supplied contract identifier into a 32-byte hex string
/// suitable for [`encode_ledger_key`].
///
/// Accepts both the canonical StrKey form (`C...`) and a raw 32-byte hex string,
/// so callers (CLI `inspect`, `events --abi-contract`, `storage --abi-contract`,
/// and the storage TTL analyzer) can pass whichever form they already hold
/// without pre-converting.
///
/// StrKey decoding is delegated to [`sdkt_xdr::decode_contract_id`], which maps
/// `C...` -> `Hash` -> hex. A value that is already valid 32-byte hex is passed
/// through unchanged.
pub(crate) fn contract_id_to_hex(contract_id: &str) -> Result<String, RpcError> {
    // Fast path: already a 32-byte hex string.
    if let Ok(bytes) = hex::decode(contract_id) {
        if bytes.len() == 32 {
            return Ok(contract_id.to_string());
        }
    }
    // Otherwise treat it as a StrKey `C...` and decode to the underlying hash.
    let hash = decode_contract_id(contract_id)
        .map_err(|e| RpcError::Rpc(format!("Invalid contract ID '{contract_id}': {e}")))?;
    Ok(hex::encode(hash.0))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContractAbiSummary {
    /// This reuses the existing `sdkt_wasm::parse_contract_spec` parser — it does NOT
    /// introduce a new ABI decoder. It only lists the declared symbol names so the
    /// on-chain inspection report can summarize a deployed contract's interface.
    pub functions: Vec<String>,
    pub events: Vec<String>,
    pub types: Vec<String>,
}

impl ContractAbiSummary {
    /// Project a parsed [`ContractSpec`] into the name-only summary.
    pub fn from_spec(spec: &ContractSpec) -> Self {
        Self {
            functions: spec.functions.iter().map(|f| f.name.clone()).collect(),
            events: spec.events.iter().map(|e| e.name.clone()).collect(),
            types: spec.custom_types.iter().map(|t| t.name.clone()).collect(),
        }
    }
}

/// The result of a contract inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractInspection {
    pub contract_id: String,
    pub wasm_hash: String,
    pub wasm_size: Option<usize>,
    /// Parsed on-chain ABI summary (functions / events / types). `None` when the
    /// on-chain WASM code cannot be fetched or has no `contractspecv0` section.
    pub abi: Option<ContractAbiSummary>,
    pub storage_summary: StorageSummary,
    pub ttl_info: Option<TtlInfoSummary>,
    pub storage_keys: Vec<StorageKeyInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TtlInfoSummary {
    pub minimum_ttl: u32,
    pub maximum_ttl: u32,
    pub average_ttl: u32,
    pub expiring_entries_count: usize,
    pub estimated_rent_cost: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StorageSummary {
    pub instance_entries: usize,
    pub persistent_entries: usize,
    pub temporary_entries: usize,
}

/// Metadata about a storage key discovered in the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageKeyInfo {
    pub key: String,
    pub key_type: String,
    pub permissions: String,
}

/// Inspects a deployed contract to extract its WASM hash, on-chain WASM size,
/// and parsed ABI — reusing the existing `get_wasm_bytecode` + `parse_contract_spec`
/// primitives. Storage/TTL/storage-key fields are left for the caller's layer
/// (which has access to `StorageAnalyzer`) to populate, matching the existing
/// architecture where storage analysis lives outside `sdkt-rpc`.
///
/// ## Protocol 28 (CAP-85) executables
///
/// A `ContractInstance`'s executable is a union, not necessarily an inline Wasm
/// hash. `Wasm` behaves exactly as before. `StellarAsset` is protocol-defined
/// code with no Wasm artifact, and `ExternalRef` (CAP-85) holds no code at
/// all — it names an entry owned by another contract. An external reference is
/// resolved to its current Wasm hash via a single read-only owner lookup
/// ([`resolve_external_executable`]); a `StellarAsset` has no hash at all, so
/// the inspection fails with an explicit error naming the executable kind
/// instead of reporting a misleading value.
///
/// Failures to fetch/parse the on-chain WASM code degrade gracefully: the
/// inspection still returns the `contract_id` + `wasm_hash` it already recovered,
/// with `wasm_size`/`abi` left as `None`. Only the initial contract-data lookup
/// failing (contract not on chain) is fatal.
pub async fn inspect_contract(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<ContractInspection, RpcError> {
    let contract_id_hex = contract_id_to_hex(contract_id)?;
    let encoded_key = encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.clone()))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode ledger key: {e}")))?;

    let response = client
        .get_contract_storage(contract_id, &[encoded_key])
        .await?;

    if response.entries.is_empty() {
        return Err(RpcError::ContractNotFound);
    }

    let first_entry = &response.entries[0];

    // Wasm executables resolve exactly as before. Non-Wasm variants get their
    // own explicit, actionable message; the legacy decode-error wording is kept
    // verbatim for undecodable entries so existing consumers are unaffected.
    let wasm_hash = match resolve_inspection_wasm_hash(client, &first_entry.xdr).await {
        Ok(hash) => hash,
        Err(ExecutableResolutionError::Decode(e)) => {
            return Err(RpcError::Rpc(format!("Failed to extract WASM hash: {e}")))
        }
        Err(e) => return Err(RpcError::Rpc(e.to_string())),
    };

    // Enrich with on-chain WASM size + parsed ABI. Both steps are best-effort:
    // if the code entry is missing or unparseable, we keep what we have instead
    // of failing the whole inspection.
    let mut wasm_size = None;
    let mut abi = None;
    if let Ok(bytes) = get_wasm_bytecode(client, &wasm_hash).await {
        wasm_size = Some(bytes.len());
        if let Ok(spec) = parse_contract_spec(&bytes) {
            abi = Some(ContractAbiSummary::from_spec(&spec));
        }
    }

    Ok(ContractInspection {
        contract_id: contract_id.to_string(),
        wasm_hash,
        wasm_size,
        abi,
        storage_summary: StorageSummary::default(),
        ttl_info: None,
        storage_keys: Vec::new(),
    })
}

/// Outcome of a read-only deployed-executable probe.
///
/// Unlike [`inspect_contract`], this never fetches code or ABI and never
/// converts a non-Wasm executable into an error: it reports exactly what the
/// contract instance ledger entry says, so a caller (deployment verification)
/// can distinguish `MATCH`-eligible data from an honest `UNKNOWN`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeployedExecutable {
    /// True when a contract-instance entry exists at this target. False means
    /// the contract does not exist here — not an error, a verdict input.
    pub instance_found: bool,
    /// The executable as decoded from the instance entry, when decodable.
    pub executable: Option<sdkt_xdr::ContractExecutableRef>,
    /// The Wasm hash, when one exists authoritatively: an inline `Wasm`
    /// executable's own hash, or a `ExternalRef` resolved through the existing
    /// read-only owner lookup. `None` for `StellarAsset` (no artifact exists)
    /// or when resolution failed.
    pub wasm_hash: Option<String>,
    /// Why the instance entry could not be decoded, when applicable.
    pub decode_error: Option<String>,
    /// Why an `ExternalRef` could not be resolved to a hash, when applicable.
    pub resolve_error: Option<String>,
    /// Transport/RPC error; `Err` means the probe could not run at all.
    pub error: Option<String>,
}

/// Probe the deployed executable of `contract_id` with a single
/// `getLedgerEntries` read of its instance singleton key (plus, for CAP-85
/// `external_ref` executables only, the existing owner-entry lookup).
///
/// Read-only: no bytecode download, no ABI parse, no signing or submission.
pub async fn probe_deployed_executable(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<DeployedExecutable, RpcError> {
    let contract_id_hex = contract_id_to_hex(contract_id)?;
    let encoded_key = encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex))
        .map_err(|e| RpcError::Rpc(format!("Failed to encode ledger key: {e}")))?;

    let response = client.get_contract_storage("", &[encoded_key]).await?;
    if response.entries.is_empty() {
        return Ok(DeployedExecutable {
            instance_found: false,
            executable: None,
            wasm_hash: None,
            decode_error: None,
            resolve_error: None,
            error: None,
        });
    }

    let entry = &response.entries[0].xdr;
    let executable = match extract_contract_executable(entry) {
        Ok(exec) => Some(exec),
        Err(e) => {
            return Ok(DeployedExecutable {
                instance_found: true,
                executable: None,
                wasm_hash: None,
                decode_error: Some(format!("{e}")),
                resolve_error: None,
                error: None,
            })
        }
    };

    let mut wasm_hash = None;
    let mut resolve_error = None;
    if let Some(sdkt_xdr::ContractExecutableRef::Wasm(hash)) = &executable {
        wasm_hash = Some(hex::encode(hash.0));
    } else if let Some(r) = executable.as_ref().and_then(|e| e.executable_tag_key()) {
        // CAP-85: resolve through the same read-only owner lookup `inspect`
        // uses, so an unresolvable reference is reported as such rather than
        // as a missing hash.
        match resolve_external_executable(client, r).await {
            Ok(hash) => wasm_hash = Some(hash),
            Err(e) => resolve_error = Some(format!("{e}")),
        }
    }

    Ok(DeployedExecutable {
        instance_found: true,
        executable,
        wasm_hash,
        decode_error: None,
        resolve_error,
        error: None,
    })
}

/// Resolve the Wasm hash a contract instance's inspection should report.
///
/// `Wasm` executables are returned unchanged, preserving the previous behavior
/// byte-for-byte. For the other union variants the instance holds no inline
/// hash, so resolution is delegated to [`resolve_external_executable`] when
/// possible; anything without a resolvable hash yields a typed
/// [`ExecutableResolutionError`] instead of a fabricated value.
async fn resolve_inspection_wasm_hash(
    client: &SorobanRpcClient,
    entry_xdr: &str,
) -> Result<String, ExecutableResolutionError> {
    let executable =
        extract_contract_executable(entry_xdr).map_err(ExecutableResolutionError::Decode)?;
    match executable {
        ContractExecutableRef::Wasm(hash) => Ok(hex::encode(hash.0)),
        ContractExecutableRef::StellarAsset => Err(ExecutableResolutionError::Unsupported {
            kind: executable.kind(),
        }),
        ContractExecutableRef::ExternalRef(reference) => {
            resolve_external_executable(client, &reference).await
        }
    }
}

/// Why an inspection or resolution could not produce a Wasm hash.
///
/// Public because it is the error type of [`resolve_external_executable`]; the
/// `Display` output is the actionable message surfaced to CLI users.
#[derive(Debug)]
pub enum ExecutableResolutionError {
    /// The ledger entry itself could not be decoded.
    Decode(sdkt_xdr::DecodeError),
    /// The executable is a valid non-Wasm variant with no inline hash.
    Unsupported {
        /// `stellar_asset` — protocol-defined code with no Wasm artifact.
        kind: &'static str,
    },
    /// A CAP-85 external reference could not be resolved to a Wasm hash.
    External {
        /// Owner contract holding the executable entry.
        owner: ScAddress,
        /// The tag keying the owner's entry.
        tag: String,
        /// Why resolution failed.
        source: String,
    },
}

impl std::fmt::Display for ExecutableResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutableResolutionError::Decode(e) => write!(f, "{e}"),
            ExecutableResolutionError::Unsupported { kind } => write!(
                f,
                "contract executable is `{kind}` (Protocol 28): it is not a Wasm contract \
                 and has no Wasm hash to report"
            ),
            ExecutableResolutionError::External { owner, tag, source } => write!(
                f,
                "could not resolve the CAP-85 external executable reference (owner {owner}, \
                 tag \"{tag}\"): {source}"
            ),
        }
    }
}

/// Resolve a CAP-85 `ExternalRef` to the Wasm hash it currently names.
///
/// CAP-0085 stores the referenced hash in a **persistent contract-data entry of
/// the owner contract, keyed by the tag** (`SCV_EXECUTABLE_TAG`). This performs
/// exactly that single read-only `getLedgerEntries` lookup; the owner contract
/// is never invoked, and nothing is signed or submitted.
pub async fn resolve_external_executable(
    client: &SorobanRpcClient,
    reference: &ExternalExecutableRef,
) -> Result<String, ExecutableResolutionError> {
    let owner = reference.owner.to_string();
    let tag = reference.tag.clone();
    let key = executable_tag_key(&reference.owner, &reference.tag).map_err(|source| {
        ExecutableResolutionError::External {
            owner: reference.owner.clone(),
            tag: tag.clone(),
            source,
        }
    })?;

    let response = client
        .get_contract_storage(&owner, &[key])
        .await
        .map_err(|e| ExecutableResolutionError::External {
            owner: reference.owner.clone(),
            tag: tag.clone(),
            source: e.to_string(),
        })?;

    let entry = response
        .entries
        .first()
        .ok_or_else(|| ExecutableResolutionError::External {
            owner: reference.owner.clone(),
            tag: tag.clone(),
            source: format!(
                "owner {owner} has no executable tag entry named \"{tag}\" (it may never have \
                 been created, or the entry may have expired)"
            ),
        })?;

    let value = extract_contract_data_value(&entry.xdr).map_err(|e| {
        ExecutableResolutionError::External {
            owner: reference.owner.clone(),
            tag: tag.clone(),
            source: format!("could not decode the owner's executable tag entry: {e}"),
        }
    })?;

    parse_external_executable_hash(&value.val).map_err(|e| ExecutableResolutionError::External {
        owner: reference.owner.clone(),
        tag: tag.clone(),
        source: e.to_string(),
    })
}

/// Build the ledger key of a CAP-85 executable-tag entry.
///
/// The owner must be a contract address (only a contract can hold such an
/// entry), and the tag travels as raw bytes because `SCString` is an opaque key
/// that need not be valid UTF-8.
fn executable_tag_key(owner: &ScAddress, tag: &str) -> Result<String, String> {
    if !matches!(owner, ScAddress::Contract(_)) {
        return Err(format!(
            "executable owner {owner} is not a contract address; only a contract can hold an \
             executable tag entry"
        ));
    }
    let tag_bytes = stellar_xdr::StringM::try_from(tag.as_bytes().to_vec())
        .map_err(|_| "executable tag exceeds the maximum ScString length".to_string())?;
    encode_ledger_key(&LedgerKeyParams::ContractDataEntry {
        contract: owner.to_string(),
        key: ScVal::ExecutableTag(stellar_xdr::ScString(tag_bytes)),
        durability: ContractDataDurability::Persistent,
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_summary_from_spec_lists_names() {
        // A spec with one function, one event, one type.
        let spec = ContractSpec {
            env_meta: None,
            functions: vec![sdkt_wasm::ContractFunction {
                name: "transfer".into(),
                doc: String::new(),
                parameters: vec![],
                outputs: vec![],
            }],
            custom_types: vec![sdkt_wasm::ContractType {
                name: "Asset".into(),
                kind: "Struct".into(),
                doc: String::new(),
                members: vec![],
                type_args: vec![],
                bytes_n: None,
            }],
            events: vec![sdkt_wasm::ContractEvent {
                name: "transfer".into(),
                doc: String::new(),
                params: vec![],
                prefix_topics: vec![],
                data_format: "single_value".into(),
            }],
        };
        let summary = ContractAbiSummary::from_spec(&spec);
        assert_eq!(summary.functions, vec!["transfer".to_string()]);
        assert_eq!(summary.events, vec!["transfer".to_string()]);
        assert_eq!(summary.types, vec!["Asset".to_string()]);
    }

    #[test]
    fn abi_summary_default_is_empty() {
        let s = ContractAbiSummary::default();
        assert!(s.functions.is_empty());
        assert!(s.events.is_empty());
        assert!(s.types.is_empty());
    }

    #[test]
    fn contract_id_to_hex_accepts_strkey_c_address() {
        // Real testnet contract StrKey `C...` used by the / live validation.
        let c = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
        let hex = contract_id_to_hex(c).expect("StrKey C... should decode");
        // 32-byte hash -> 64 hex chars.
        assert_eq!(hex.len(), 64);
        // Re-decoding the hex must be a no-op (already hex, passed through).
        assert_eq!(contract_id_to_hex(&hex).unwrap(), hex);
        // The decode must match the canonical hash of this contract.
        assert_eq!(
            hex,
            "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c"
        );
    }

    #[test]
    fn contract_id_to_hex_rejects_garbage() {
        assert!(contract_id_to_hex("not-a-contract-id").is_err());
        // Valid-length hex but wrong type of strkey (account G...) must fail.
        assert!(
            contract_id_to_hex("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF").is_err()
        );
    }
}
