//! Versioned network profile: live-observed network configuration with
//! provenance, snapshotting, and explicit coverage status.
//!
//! ## What this module is
//!
//! A [`NetworkProfile`] binds:
//!
//! - **network identity** — the 32-byte network ID, derived as
//!   `SHA-256(passphrase)` (the Stellar network ID rule), never a placeholder;
//! - **protocol version** and **ledger sequence** as reported by the RPC
//!   endpoint (`getNetwork` / `getLatestLedger`);
//! - **configuration parameters** decoded from live `ConfigSetting` ledger
//!   entries (`getLedgerEntries`), each carrying its own provenance;
//! - **snapshot metadata** — source endpoint, retrieval time, and a content
//!   hash that covers every field a replay depends on.
//!
//! ## What this module is not
//!
//! - It is **not** a claim that local execution reproduces the network. The
//!   pinned host (`soroban-env-host 28.0.2`) supports protocol 28; a profile
//!   for a higher protocol is recorded and marked [`ProfileStatus::HostUnsupported`].
//! - It is **not** an oracle. Parameters the RPC does not expose are recorded
//!   as [`ParamSource::Unavailable`], never silently defaulted to zero.
//! - It is **not** a Mainnet/Testnet equivalence claim: the network id is part
//!   of the hash, so two profiles from different networks never compare equal.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The protocol version the pinned host can execute.
///
/// `soroban-env-host 28.0.2` is built from `soroban-env-common 28.0.2`, whose
/// `meta::INTERFACE_VERSION.protocol` is 28 (`src/meta.rs`:
/// `ledger_protocol_version: 28` for the non-`next` build). The host rejects a
/// ledger protocol above that in `Host::check_ledger_protocol_supported`
/// ("ledger protocol version too new for host"), so protocol 29 execution is
/// **not** available with this dependency.
pub const HOST_SUPPORTED_PROTOCOL: u32 = 28;

/// Maximum snapshot age, in ledgers, before a profile is considered stale
/// against a freshly observed ledger. Mainnet closes roughly every 5s, so this
/// is a few minutes of drift tolerance — deliberately small because network
/// configuration can change by upgrade, and a "current" claim must mean it.
pub const MAX_SNAPSHOT_AGE_LEDGERS: u64 = 100;

/// Where a parameter value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamSource {
    /// Read directly from a live `ConfigSetting` ledger entry via RPC.
    LiveConfigSetting,
    /// Read from the RPC network/ledger endpoints (`getNetwork`,
    /// `getLatestLedger`).
    LiveRpcMetadata,
    /// Derived deterministically from other observed values (e.g. the network
    /// ID is derived from the passphrase).
    Derived,
    /// Loaded from a previously written snapshot rather than the network.
    Snapshot,
    /// Not exposed by the RPC surface this tool uses. The value is unknown;
    /// it is **not** zero.
    Unavailable,
}

/// Provenance for one observed quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: ParamSource,
    /// Ledger sequence the value was observed at, when the source has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ledger: Option<u32>,
}

impl Provenance {
    pub fn live_config(ledger: u32) -> Self {
        Self {
            source: ParamSource::LiveConfigSetting,
            observed_at_ledger: Some(ledger),
        }
    }

    pub fn unavailable() -> Self {
        Self {
            source: ParamSource::Unavailable,
            observed_at_ledger: None,
        }
    }
}

/// A single network configuration parameter with explicit provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observed<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    pub provenance: Provenance,
}

impl<T> Observed<T> {
    pub fn live(value: T, ledger: u32) -> Self {
        Self {
            value: Some(value),
            provenance: Provenance::live_config(ledger),
        }
    }

    pub fn unavailable() -> Self {
        Self {
            value: None,
            provenance: Provenance::unavailable(),
        }
    }

    pub fn is_available(&self) -> bool {
        self.value.is_some()
    }
}

/// One cost-parameter entry, keyed by the `ContractCostType` name so the
/// mapping is explicit and survives enum reordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostParamEntrySnapshot {
    /// `ContractCostType` variant name (e.g. `"WasmInsnExec"`).
    pub cost_type: String,
    /// Position in the source `ContractCostParams` vector.
    pub index: u32,
    pub const_term: i64,
    pub linear_term: i64,
}

/// The CPU and memory cost tables observed from the network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostParamsSnapshot {
    pub cpu: Vec<CostParamEntrySnapshot>,
    pub mem: Vec<CostParamEntrySnapshot>,
}

impl CostParamsSnapshot {
    /// Entry count per dimension. The two must agree for a well-formed
    /// network configuration (`ContractCostParams` is one entry per cost type
    /// in both dimensions).
    pub fn entry_count(&self) -> usize {
        self.cpu.len().min(self.mem.len())
    }

    /// True when the two dimensions disagree in length — a malformed
    /// configuration, not a partial one.
    pub fn is_malformed(&self) -> bool {
        self.cpu.len() != self.mem.len()
    }
}

/// The set of configuration values a profile carries. Every field is
/// `Option`-shaped through [`Observed`] so absence is representable and
/// visible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkConfigSnapshot {
    pub cpu_limit: Observed<u64>,
    pub mem_limit: Observed<u64>,
    pub ledger_max_instructions: Observed<u64>,
    pub fee_rate_per_instructions_increment: Observed<u64>,
    pub max_contract_size_bytes: Observed<u32>,
    pub tx_max_size_bytes: Observed<u32>,
    pub tx_max_contract_events_size_bytes: Observed<u32>,
    pub fee_contract_events_1kb: Observed<i64>,
    pub cost_params: Observed<CostParamsSnapshot>,
}

impl Default for NetworkConfigSnapshot {
    fn default() -> Self {
        Self {
            cpu_limit: Observed::unavailable(),
            mem_limit: Observed::unavailable(),
            ledger_max_instructions: Observed::unavailable(),
            fee_rate_per_instructions_increment: Observed::unavailable(),
            max_contract_size_bytes: Observed::unavailable(),
            tx_max_size_bytes: Observed::unavailable(),
            tx_max_contract_events_size_bytes: Observed::unavailable(),
            fee_contract_events_1kb: Observed::unavailable(),
            cost_params: Observed::unavailable(),
        }
    }
}

/// How far the profile is from being usable for faithful execution.
///
/// Ordered from most to least capable; a profile carries exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    /// Identity, protocol, ledger, and every required configuration parameter
    /// were observed, and the pinned host can execute this protocol.
    Complete,
    /// Some configuration could not be observed (RPC does not expose it) or a
    /// cost dimension is missing entries. Execution is possible but the model
    /// is knowingly partial — callers must not claim network parity.
    Incomplete,
    /// The profile's protocol version is above
    /// [`HOST_SUPPORTED_PROTOCOL`]. The pinned host refuses this protocol, so
    /// no local execution can reproduce it with this dependency set.
    HostUnsupported,
}

impl ProfileStatus {
    /// A profile may only be reported as executable under the pinned host when
    /// nothing else is missing.
    pub fn is_complete_execution_ready(self) -> bool {
        matches!(self, ProfileStatus::Complete)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ProfileStatus::Complete => "COMPLETE",
            ProfileStatus::Incomplete => "INCOMPLETE",
            ProfileStatus::HostUnsupported => "HOST_UNSUPPORTED",
        }
    }
}

/// A versioned network profile: observed identity, protocol, ledger, and
/// configuration, with provenance and snapshot metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkProfile {
    /// Profile schema version. Bumped when the snapshot layout changes, so an
    /// old snapshot is rejected rather than misread.
    pub schema_version: u32,
    /// Human-readable network name (`"mainnet"`, `"testnet"`, `"futurenet"`,
    /// or `"custom"`). Diagnostic only — never used for identity.
    pub network_name: String,
    /// The canonical network passphrase, as reported by `getNetwork`.
    pub passphrase: String,
    /// `SHA-256(passphrase)` — the Stellar network ID.
    pub network_id: [u8; 32],
    /// Protocol version the network reports.
    pub protocol_version: u32,
    /// Ledger sequence the observations were made at.
    pub ledger_sequence: u32,
    /// RPC endpoint the profile was captured from.
    pub source_endpoint: String,
    /// Unix seconds the snapshot was written (0 for a synthetic/offline one).
    pub captured_at_unix: u64,
    pub config: NetworkConfigSnapshot,
    /// SHA-256 over the canonical identity fields (computed on demand by
    /// [`NetworkProfile::content_hash`]; not stored, so it can never drift
    /// from the data).
    #[serde(skip)]
    pub content_hash_cache: Option<[u8; 32]>,
}

/// Errors raised while validating a profile or snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// Snapshot schema version is not the one this build understands.
    SchemaMismatch { found: u32, expected: u32 },
    /// `network_id` is not `SHA-256(passphrase)`.
    NetworkIdMismatch { expected: [u8; 32], found: [u8; 32] },
    /// The profile was captured at a ledger too far behind `current`.
    Stale {
        captured: u32,
        current: u32,
        max_age: u64,
    },
    /// CPU and memory cost tables have different lengths.
    MalformedCostParams { cpu: usize, mem: usize },
    /// Cost-table index/name mapping is inconsistent.
    CostTypeMapping(String),
    /// The profile claims complete execution for a protocol the host rejects.
    StatusContradiction { protocol: u32, host_max: u32 },
    /// A required configuration value is absent.
    MissingParameter(&'static str),
    /// The passphrase is not a recognisable network and no explicit name.
    UnknownNetwork(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::SchemaMismatch { found, expected } => {
                write!(f, "snapshot schema_version {found} != {expected}")
            }
            ProfileError::NetworkIdMismatch { .. } => {
                write!(f, "network_id does not match SHA-256(passphrase)")
            }
            ProfileError::Stale {
                captured,
                current,
                max_age,
            } => write!(
                f,
                "snapshot captured at ledger {captured} is {current} behind (max age {max_age})"
            ),
            ProfileError::MalformedCostParams { cpu, mem } => {
                write!(f, "cost params length mismatch: cpu={cpu} mem={mem}")
            }
            ProfileError::CostTypeMapping(m) => write!(f, "cost type mapping: {m}"),
            ProfileError::StatusContradiction { protocol, host_max } => write!(
                f,
                "profile protocol {protocol} exceeds host max {host_max}, yet is marked complete"
            ),
            ProfileError::MissingParameter(p) => write!(f, "missing parameter: {p}"),
            ProfileError::UnknownNetwork(p) => write!(f, "unknown network passphrase: {p}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// The current snapshot schema version.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Derive the Stellar network ID from a passphrase: `SHA-256(passphrase)`.
///
/// This is the rule Stellar uses for signing (`sdkt-xdr`'s
/// `Network::network_id`) and for contract-ID derivation. A profile must use
/// the derived value, never a placeholder constant.
pub fn network_id_from_passphrase(passphrase: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(passphrase.as_bytes());
    h.finalize().into()
}

/// Recognise a known Stellar network name from its passphrase.
pub fn network_name_for_passphrase(passphrase: &str) -> Option<&'static str> {
    match passphrase {
        "Public Global Stellar Network ; September 2015" => Some("mainnet"),
        "Test SDF Network ; September 2015" => Some("testnet"),
        "Test SDF Future Network ; October 2022" => Some("futurenet"),
        _ => None,
    }
}

impl NetworkProfile {
    /// Build a profile from observed values, deriving the network id from the
    /// passphrase and computing the status.
    ///
    /// `config` entries supplied without a value stay `Unavailable`; the
    /// resulting status is `Incomplete`.
    pub fn new(
        passphrase: String,
        protocol_version: u32,
        ledger_sequence: u32,
        source_endpoint: String,
        captured_at_unix: u64,
        config: NetworkConfigSnapshot,
    ) -> Self {
        let network_name = network_name_for_passphrase(&passphrase)
            .unwrap_or("custom")
            .to_string();
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            network_name,
            network_id: network_id_from_passphrase(&passphrase),
            passphrase,
            protocol_version,
            ledger_sequence,
            source_endpoint,
            captured_at_unix,
            config,
            content_hash_cache: None,
        }
    }

    /// Every required parameter is present, and the cost tables are
    /// well-formed and non-empty.
    pub fn has_complete_configuration(&self) -> bool {
        let c = &self.config;
        let basic = c.cpu_limit.is_available()
            && c.mem_limit.is_available()
            && c.ledger_max_instructions.is_available()
            && c.max_contract_size_bytes.is_available()
            && c.tx_max_size_bytes.is_available()
            && c.cost_params.is_available();
        let params_ok = c
            .cost_params
            .value
            .as_ref()
            .map(|p| !p.is_malformed() && !p.cpu.is_empty())
            .unwrap_or(false);
        basic && params_ok
    }

    /// Classify the profile.
    ///
    /// `HostUnsupported` takes precedence: a protocol the pinned host refuses
    /// can never be a faithful execution target here, whatever else is known.
    pub fn status(&self) -> ProfileStatus {
        if self.protocol_version > HOST_SUPPORTED_PROTOCOL {
            return ProfileStatus::HostUnsupported;
        }
        if !self.has_complete_configuration() {
            return ProfileStatus::Incomplete;
        }
        ProfileStatus::Complete
    }

    /// Validate structural integrity and provenance.
    ///
    /// `current_ledger` is the ledger sequence to check staleness against;
    /// pass `None` for an offline load where currency is not being asserted
    /// (a snapshot is then never reported stale).
    pub fn validate(&self, current_ledger: Option<u32>) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::SchemaMismatch {
                found: self.schema_version,
                expected: PROFILE_SCHEMA_VERSION,
            });
        }
        let expected = network_id_from_passphrase(&self.passphrase);
        if expected != self.network_id {
            return Err(ProfileError::NetworkIdMismatch {
                expected,
                found: self.network_id,
            });
        }
        if let Some(p) = self.config.cost_params.value.as_ref() {
            if p.is_malformed() {
                return Err(ProfileError::MalformedCostParams {
                    cpu: p.cpu.len(),
                    mem: p.mem.len(),
                });
            }
            // Every entry's declared index must match its position and the
            // names must be identical across the two dimensions.
            for (i, (c, m)) in p.cpu.iter().zip(p.mem.iter()).enumerate() {
                if c.index as usize != i || m.index as usize != i {
                    return Err(ProfileError::CostTypeMapping(format!(
                        "entry {i} declares index cpu={} mem={}",
                        c.index, m.index
                    )));
                }
                if c.cost_type != m.cost_type {
                    return Err(ProfileError::CostTypeMapping(format!(
                        "entry {i}: cpu type `{}` != mem type `{}`",
                        c.cost_type, m.cost_type
                    )));
                }
            }
        }
        if let Some(current) = current_ledger {
            let age = current.saturating_sub(self.ledger_sequence) as u64;
            if current > self.ledger_sequence && age > MAX_SNAPSHOT_AGE_LEDGERS {
                return Err(ProfileError::Stale {
                    captured: self.ledger_sequence,
                    current,
                    max_age: MAX_SNAPSHOT_AGE_LEDGERS,
                });
            }
            if self.ledger_sequence > current {
                // A snapshot from the future is not a valid observation of
                // this ledger either.
                return Err(ProfileError::Stale {
                    captured: self.ledger_sequence,
                    current,
                    max_age: MAX_SNAPSHOT_AGE_LEDGERS,
                });
            }
        }
        Ok(())
    }

    /// Canonical hash over every field a replay depends on. Two profiles with
    /// the same hash describe the same network state and configuration.
    ///
    /// `source_endpoint`, `captured_at_unix`, and `network_name` are excluded:
    /// they are capture metadata, not network state (the same endpoint content
    /// fetched twice must hash equal).
    pub fn content_hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"sdkt-fuzz-network-profile-v1");
        h.update(self.schema_version.to_be_bytes());
        h.update(self.network_id);
        h.update(self.protocol_version.to_be_bytes());
        h.update(self.ledger_sequence.to_be_bytes());
        h.update(self.passphrase.as_bytes());
        h.update([0u8]);

        let cfg = &self.config;
        fn feed_u64(h: &mut Sha256, o: &Observed<u64>) {
            match o.value {
                Some(v) => {
                    h.update([1u8]);
                    h.update(v.to_be_bytes());
                }
                None => h.update([0u8]),
            }
        }
        fn feed_i64(h: &mut Sha256, o: &Observed<i64>) {
            match o.value {
                Some(v) => {
                    h.update([1u8]);
                    h.update(v.to_be_bytes());
                }
                None => h.update([0u8]),
            }
        }
        fn feed_u32(h: &mut Sha256, o: &Observed<u32>) {
            match o.value {
                Some(v) => {
                    h.update([1u8]);
                    h.update(v.to_be_bytes());
                }
                None => h.update([0u8]),
            }
        }
        feed_u64(&mut h, &cfg.cpu_limit);
        feed_u64(&mut h, &cfg.mem_limit);
        feed_u64(&mut h, &cfg.ledger_max_instructions);
        feed_u64(&mut h, &cfg.fee_rate_per_instructions_increment);
        feed_u32(&mut h, &cfg.max_contract_size_bytes);
        feed_u32(&mut h, &cfg.tx_max_size_bytes);
        feed_u32(&mut h, &cfg.tx_max_contract_events_size_bytes);
        feed_i64(&mut h, &cfg.fee_contract_events_1kb);

        match cfg.cost_params.value.as_ref() {
            Some(p) => {
                h.update([1u8]);
                h.update((p.cpu.len() as u64).to_be_bytes());
                for (c, m) in p.cpu.iter().zip(p.mem.iter()) {
                    h.update(c.index.to_be_bytes());
                    h.update(c.cost_type.as_bytes());
                    h.update(c.const_term.to_be_bytes());
                    h.update(c.linear_term.to_be_bytes());
                    h.update(m.const_term.to_be_bytes());
                    h.update(m.linear_term.to_be_bytes());
                }
            }
            None => h.update([0u8]),
        }
        h.finalize().into()
    }

    /// Hex rendering of [`NetworkProfile::content_hash`].
    pub fn content_hash_hex(&self) -> String {
        self.content_hash()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Ledger sequence the CPU/memory limits were observed at, when known.
    pub fn config_ledger(&self) -> Option<u32> {
        self.config.cpu_limit.provenance.observed_at_ledger
    }

    /// Cost types with a live entry, keyed by variant name.
    pub fn cost_type_table(&self) -> BTreeMap<&str, (i64, i64)> {
        let mut out = BTreeMap::new();
        if let Some(p) = self.config.cost_params.value.as_ref() {
            for e in &p.cpu {
                out.insert(e.cost_type.as_str(), (e.const_term, e.linear_term));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAINNET: &str = "Public Global Stellar Network ; September 2015";
    const TESTNET: &str = "Test SDF Network ; September 2015";

    fn cost_params(n: usize) -> CostParamsSnapshot {
        let names = [
            "WasmInsnExec",
            "MemAlloc",
            "MemCpy",
            "MemCmp",
            "DispatchHostFunction",
        ];
        let cpu = (0..n)
            .map(|i| CostParamEntrySnapshot {
                cost_type: names.get(i).unwrap_or(&"Synthetic").to_string(),
                index: i as u32,
                const_term: 4 + i as i64,
                linear_term: 0,
            })
            .collect::<Vec<_>>();
        let mem = cpu.clone();
        CostParamsSnapshot { cpu, mem }
    }

    fn complete_config() -> NetworkConfigSnapshot {
        NetworkConfigSnapshot {
            cpu_limit: Observed::live(2_500_000, 1000),
            mem_limit: Observed::live(2_000_000, 1000),
            ledger_max_instructions: Observed::live(580_000_000, 1000),
            fee_rate_per_instructions_increment: Observed::live(7, 1000),
            max_contract_size_bytes: Observed::live(131_072, 1000),
            tx_max_size_bytes: Observed::live(132_096, 1000),
            tx_max_contract_events_size_bytes: Observed::live(16_384, 1000),
            fee_contract_events_1kb: Observed::live(200, 1000),
            cost_params: Observed::live(cost_params(5), 1000),
        }
    }

    fn profile(passphrase: &str, protocol: u32) -> NetworkProfile {
        NetworkProfile::new(
            passphrase.to_string(),
            protocol,
            1000,
            "https://example.invalid".to_string(),
            0,
            complete_config(),
        )
    }

    #[test]
    fn network_id_is_sha256_of_passphrase() {
        // Known values: the testnet/mainnet IDs the rest of the workspace uses.
        let testnet = network_id_from_passphrase(TESTNET);
        assert_eq!(
            hex(&testnet),
            "cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472"
        );
        let mainnet = network_id_from_passphrase(MAINNET);
        assert_eq!(
            hex(&mainnet),
            "7ac33997544e3175d266bd022439b22cdb16508c01163f26e5cb2a3e1045a979"
        );
    }

    #[test]
    fn profile_derives_network_id_not_placeholder() {
        let p = profile(MAINNET, 28);
        assert_eq!(p.network_id, network_id_from_passphrase(MAINNET));
        // The old placeholder constant must never appear.
        assert_ne!(p.network_id, [5u8; 32]);
        assert_eq!(p.network_name, "mainnet");
    }

    #[test]
    fn placeholder_network_id_is_rejected_by_validation() {
        let mut p = profile(MAINNET, 28);
        p.network_id = [5u8; 32];
        let err = p.validate(None).unwrap_err();
        assert!(
            matches!(err, ProfileError::NetworkIdMismatch { .. }),
            "{err}"
        );
    }

    #[test]
    fn mainnet_protocol_29_is_host_unsupported_not_complete() {
        let mut p = profile(MAINNET, 29);
        p.config.cpu_limit = Observed::live(400_000_000, 1000);
        p.config.mem_limit = Observed::live(41_943_040, 1000);
        assert_eq!(p.status(), ProfileStatus::HostUnsupported);
        assert!(!p.status().is_complete_execution_ready());
        assert!(p.has_complete_configuration(), "config itself is complete");
    }

    #[test]
    fn protocol_28_with_complete_config_is_complete() {
        let p = profile(TESTNET, 28);
        assert_eq!(p.status(), ProfileStatus::Complete);
        assert!(p.status().is_complete_execution_ready());
    }

    #[test]
    fn missing_parameter_yields_incomplete_not_zero() {
        let mut p = profile(TESTNET, 28);
        p.config.cpu_limit = Observed::unavailable();
        assert_eq!(p.status(), ProfileStatus::Incomplete);
        // Absence must be visible, never a silent zero.
        assert_eq!(p.config.cpu_limit.value, None);
        assert_eq!(
            p.config.cpu_limit.provenance.source,
            ParamSource::Unavailable
        );
        assert!(!p.has_complete_configuration());
    }

    #[test]
    fn malformed_cost_params_are_rejected() {
        let mut p = profile(TESTNET, 28);
        let mut cp = cost_params(4);
        cp.mem.truncate(3);
        p.config.cost_params = Observed::live(cp, 1000);
        let err = p.validate(None).unwrap_err();
        assert!(
            matches!(err, ProfileError::MalformedCostParams { .. }),
            "{err}"
        );
        assert_eq!(p.status(), ProfileStatus::Incomplete);
    }

    #[test]
    fn cost_type_index_mismatch_is_rejected() {
        let mut p = profile(TESTNET, 28);
        let mut cp = cost_params(4);
        cp.cpu[2].index = 7;
        p.config.cost_params = Observed::live(cp, 1000);
        let err = p.validate(None).unwrap_err();
        assert!(matches!(err, ProfileError::CostTypeMapping(_)), "{err}");
    }

    #[test]
    fn cpu_mem_name_mismatch_is_rejected() {
        let mut p = profile(TESTNET, 28);
        let mut cp = cost_params(4);
        cp.mem[1].cost_type = "NotMemAlloc".to_string();
        p.config.cost_params = Observed::live(cp, 1000);
        let err = p.validate(None).unwrap_err();
        assert!(matches!(err, ProfileError::CostTypeMapping(_)), "{err}");
    }

    #[test]
    fn stale_snapshot_is_rejected_against_current_ledger() {
        let p = profile(TESTNET, 28); // captured at 1000
        assert!(p
            .validate(Some(1000 + MAX_SNAPSHOT_AGE_LEDGERS as u32))
            .is_ok());
        let err = p
            .validate(Some(1000 + MAX_SNAPSHOT_AGE_LEDGERS as u32 + 1))
            .unwrap_err();
        assert!(matches!(err, ProfileError::Stale { .. }), "{err}");
    }

    #[test]
    fn snapshot_from_the_future_is_rejected() {
        let p = profile(TESTNET, 28); // captured at 1000
        let err = p.validate(Some(999)).unwrap_err();
        assert!(matches!(err, ProfileError::Stale { .. }), "{err}");
    }

    #[test]
    fn offline_load_does_not_assert_currency() {
        let p = profile(TESTNET, 28);
        assert!(p.validate(None).is_ok(), "offline load is valid");
    }

    #[test]
    fn unknown_passphrase_is_named_custom() {
        let p = profile("My Private Network ; 2026", 28);
        assert_eq!(p.network_name, "custom");
        assert_eq!(
            p.network_id,
            network_id_from_passphrase("My Private Network ; 2026")
        );
    }

    #[test]
    fn different_networks_hash_differently() {
        let a = profile(MAINNET, 28);
        let b = profile(TESTNET, 28);
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn different_protocol_or_ledger_hash_differently() {
        let a = profile(TESTNET, 28);
        let mut b = profile(TESTNET, 28);
        b.protocol_version = 27;
        assert_ne!(a.content_hash(), b.content_hash());

        let mut c = profile(TESTNET, 28);
        c.ledger_sequence = 1001;
        assert_ne!(a.content_hash(), c.content_hash());
    }

    #[test]
    fn endpoint_and_time_do_not_change_content_hash() {
        let a = profile(TESTNET, 28);
        let mut b = profile(TESTNET, 28);
        b.source_endpoint = "https://other.example".to_string();
        b.captured_at_unix = 12345;
        assert_eq!(
            a.content_hash(),
            b.content_hash(),
            "capture metadata is not network state"
        );
    }

    #[test]
    fn config_change_changes_content_hash() {
        let a = profile(TESTNET, 28);
        let mut b = profile(TESTNET, 28);
        b.config.cpu_limit = Observed::live(1, 1000);
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn snapshot_round_trips_and_revalidates() {
        let p = profile(MAINNET, 29);
        let json = serde_json::to_string(&p).unwrap();
        let back: NetworkProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content_hash(), p.content_hash());
        assert_eq!(back.status(), ProfileStatus::HostUnsupported);
        back.validate(None).unwrap();
    }

    #[test]
    fn schema_mismatch_is_rejected() {
        let mut p = profile(TESTNET, 28);
        p.schema_version = PROFILE_SCHEMA_VERSION + 1;
        let err = p.validate(None).unwrap_err();
        assert!(matches!(err, ProfileError::SchemaMismatch { .. }), "{err}");
    }

    fn hex(b: &[u8; 32]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
