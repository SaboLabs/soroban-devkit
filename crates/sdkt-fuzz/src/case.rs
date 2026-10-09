//! Fuzz case input model.
//!
//! A [`FuzzCase`] is a fully self-contained description of one execution:
//! which function to call, with which `ScVal` arguments, against which
//! baseline state. Everything needed to reproduce it lives here — nothing
//! is inferred from generator state (Phase 2) or campaign context.

use soroban_env_host::xdr::{
    AccountId, ContractDataDurability, ContractDataEntry, ContractExecutable, ContractId,
    ContractIdPreimage, ContractIdPreimageFromAddress, ExtensionPoint, Hash, HashIdPreimage,
    HashIdPreimageContractId, LedgerEntry, LedgerEntryData, LedgerEntryExt, Limits, PublicKey,
    ScAddress, ScContractInstance, ScVal, Uint256, VecM, WriteXdr,
};

/// A single call into the contract under test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionCall {
    /// Exported function name.
    pub function: String,
    /// Ordered `ScVal` arguments.
    pub args: Vec<ScVal>,
}

impl FunctionCall {
    pub fn new(function: impl Into<String>, args: Vec<ScVal>) -> Self {
        Self {
            function: function.into(),
            args,
        }
    }
}

/// Instance-storage seed: `(key, value)` pairs placed into the contract
/// instance's storage map before execution. The executor owns the instance
/// entry itself; only its storage contents are seedable here.
pub type InstanceStorage = Vec<(ScVal, ScVal)>;

/// One fuzz case: contract identity + call + baseline state.
///
/// Invariants:
/// - `baseline_entries` must be `LedgerEntryData::ContractData` entries
///   belonging to *this* case's contract; the executor scopes their keys.
/// - `contract_id` is derived, not free-form: see [`FuzzCase::from_deployer`].
#[derive(Clone, Debug)]
pub struct FuzzCase {
    /// Deterministic case identifier. Phase 1 accepts any caller-chosen
    /// value; Phase 2 will derive it from seed + index. It is carried
    /// through to observations for traceability only — it does not affect
    /// execution.
    pub case_id: String,
    /// Deployed contract address (the `ScAddress::Contract` payload).
    pub contract_id: ContractId,
    /// The call to execute.
    pub call: FunctionCall,
    /// Baseline contract-data entries restored before execution.
    pub baseline_entries: Vec<LedgerEntry>,
    /// Baseline instance-storage pairs seeded into the executor-owned
    /// instance entry (state carry-forward for sequences). Ignored when
    /// [`FuzzCase::external_instance_entry`] is set.
    pub instance_storage: InstanceStorage,
    /// An externally-verified contract-instance entry to use instead of a
    /// synthesized one. When set, the executor uses this entry verbatim
    /// (after validating its contract address and key shape). This is the
    /// state-matched path.
    pub external_instance_entry: Option<LedgerEntry>,
}

impl FuzzCase {
    /// Build a case whose contract id is deterministically derived from
    /// `(network_id, deployer, salt)` — the same derivation the host uses
    /// for an address-preimage `CreateContract`, so the id matches what a
    /// real deployment against `network_id` would produce.
    pub fn from_deployer(
        case_id: impl Into<String>,
        network_id: [u8; 32],
        deployer: [u8; 32],
        salt: [u8; 32],
        call: FunctionCall,
        baseline_entries: Vec<LedgerEntry>,
    ) -> Self {
        let contract_id = contract_id_from_address(network_id, deployer, salt);
        Self {
            case_id: case_id.into(),
            contract_id,
            call,
            baseline_entries,
            instance_storage: Vec::new(),
            external_instance_entry: None,
        }
    }

    /// Seed instance storage for this case.
    pub fn with_instance_storage(mut self, storage: InstanceStorage) -> Self {
        self.instance_storage = storage;
        self
    }

    /// Seed the case with an externally-verified contract-instance entry.
    ///
    /// This is the state-matched path: the instance entry comes from the
    /// network (e.g. a `simulateTransaction` response's `stateChanges.before`
    /// or a `getLedgerEntries` read) rather than being synthesized by the
    /// executor. The executor will use this entry as-is — including its
    /// executable and storage map — instead of building one from the campaign
    /// WASM hash.
    ///
    /// The entry must be a `ContractData` entry whose key is
    /// `LedgerKeyContractInstance` and whose contract matches this case's
    /// address; anything else is rejected at execution time, so a bad seed
    /// cannot silently change what is executed.
    pub fn with_external_instance_entry(mut self, entry: LedgerEntry) -> Self {
        self.external_instance_entry = Some(entry);
        self
    }

    /// The externally-provided instance entry, when set.
    pub fn external_instance_entry(&self) -> Option<&LedgerEntry> {
        self.external_instance_entry.as_ref()
    }

    /// The contract address this case executes against.
    pub fn contract_address(&self) -> ScAddress {
        ScAddress::Contract(self.contract_id.clone())
    }
}

/// Deterministic contract id for an address-preimage deployment.
fn contract_id_from_address(
    network_id: [u8; 32],
    deployer: [u8; 32],
    salt: [u8; 32],
) -> ContractId {
    let preimage = ContractIdPreimage::Address(ContractIdPreimageFromAddress {
        address: ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
            deployer,
        )))),
        salt: Uint256(salt),
    });
    let hash_preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(network_id),
        contract_id_preimage: preimage,
    });
    let bytes = hash_preimage
        .to_xdr(Limits::none())
        .expect("HashIdPreimage XDR encoding is infallible");
    use sha2::{Digest, Sha256};
    ContractId(Hash(Sha256::digest(&bytes).into()))
}

/// Build the contract-instance ledger entry the executor seeds for a case.
///
/// `soroban-env-host` resolves a contract's executable from its instance
/// entry, so every case gets one pointing at the campaign WASM hash, with
/// the case's instance-storage seed applied.
pub(crate) fn instance_entry(
    contract: &ScAddress,
    wasm_hash: &Hash,
    storage: &[(ScVal, ScVal)],
) -> LedgerEntry {
    // Host requires instance storage maps sorted by key; sort is stable
    // (ScVal: Ord) so the map content is deterministic for identical seeds.
    let mut entries: Vec<soroban_env_host::xdr::ScMapEntry> = storage
        .iter()
        .map(|(k, v)| soroban_env_host::xdr::ScMapEntry {
            key: k.clone(),
            val: v.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries.dedup_by(|a, b| a.key == b.key);
    let map = if entries.is_empty() {
        None
    } else {
        Some(soroban_env_host::xdr::ScMap(
            VecM::try_from(entries).expect("instance storage map within XDR limits"),
        ))
    };
    LedgerEntry {
        last_modified_ledger_seq: 0,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: contract.clone(),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(wasm_hash.clone()),
                storage: map,
            }),
        }),
        ext: LedgerEntryExt::V0,
    }
}
