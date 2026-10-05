//! Network-aware diagnostics — what network am I actually talking to?
//!
//! `sdkt network diagnose` answers the questions a release verification needs
//! answered *before* trusting any on-chain evidence:
//!
//! 1. **Identity** — does the endpoint's passphrase equal the passphrase the
//!    local configuration expects? (The classic foot-gun: signing against the
//!    wrong network with a plausible-looking endpoint.)
//! 2. **Protocol** — which protocol version do `getNetwork`, `getLedger`, and
//!    the network's Horizon API each report, and do they agree?
//! 3. **Environment** — which network is this (testnet/mainnet/futurenet/
//!    custom), what does the RPC node self-report, and is Horizon available?
//! 4. **Resource limits** — the authoritative values a ledger actually closed
//!    with (`base_fee_in_stroops`, `base_reserve_in_stroops`,
//!    `max_tx_set_size`), read from Horizon's ledger resource **only when that
//!    source answers**.
//!
//! Design rules:
//!
//! - **No hardcoded limits.** Every limit value carries the source that
//!   produced it. When Horizon does not answer, the result is
//!   `status: "unavailable"` with a reason — never a hardcoded constant and
//!   never a silent PASS.
//! - **Read-only.** One JSON-RPC probe (`getHealth`, `getNetwork`,
//!   `getLatestLedger`) plus at most two Horizon GETs. Nothing is signed,
//!   submitted, or deployed; the mutating mainnet guard does not apply.
//! - **Partial data is reported, not hidden.** `unknown`/`mismatch` are real
//!   outcomes with explicit statuses, so a consumer never mistakes absence of
//!   data for agreement.

use crate::commands::network::resolve_target_network;
use crate::NetworkArgs;
use sdkt_core::network_key;
use sdkt_rpc::{account::horizon_url_for_endpoint, SorobanRpcClient};
use serde::Serialize;

/// Schema version of the machine-readable network-diagnosis report.
pub const DIAGNOSIS_SCHEMA_VERSION: u32 = 1;

/// Identity comparison status between configured and endpoint passphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    /// The endpoint's passphrase equals the configured one.
    Match,
    /// The endpoint reports a different passphrase — verification against
    /// this target is meaningless until resolved.
    Mismatch,
    /// The endpoint did not report a passphrase; nothing was compared.
    Unknown,
}

/// Protocol agreement across the sources that reported a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolStatus {
    /// Every source that answered reports the same protocol version.
    Consistent,
    /// Two or more sources answered with different versions — the sources
    /// themselves disagree.
    Inconsistent,
    /// No source answered with a protocol version.
    Unknown,
}

/// Compare configured vs endpoint passphrase (pure; unit-tested).
pub fn classify_identity(configured: &str, endpoint: Option<&str>) -> IdentityStatus {
    match endpoint {
        Some(ep) if ep == configured => IdentityStatus::Match,
        Some(_) => IdentityStatus::Mismatch,
        None => IdentityStatus::Unknown,
    }
}

/// Compare protocol versions across RPC `getNetwork`, RPC `getLatestLedger`
/// and Horizon (pure; unit-tested). Missing sources are skipped; agreement
/// requires every present source to be equal, and at least one to exist.
pub fn classify_protocol(
    rpc_network: Option<u32>,
    rpc_ledger: Option<u32>,
    horizon: Option<u32>,
) -> ProtocolStatus {
    let present: Vec<u32> = [rpc_network, rpc_ledger, horizon]
        .into_iter()
        .flatten()
        .collect();
    if present.is_empty() {
        ProtocolStatus::Unknown
    } else if present.windows(2).all(|w| w[0] == w[1]) {
        ProtocolStatus::Consistent
    } else {
        ProtocolStatus::Inconsistent
    }
}

/// Authoritative resource limits read from a Horizon ledger resource.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceLimits {
    /// `available` when every value below came from a live Horizon ledger
    /// response; `unavailable` when the source did not answer (values absent).
    pub status: String,
    /// Human-readable provenance, e.g. `horizon ledger 5032903`. Absent when
    /// unavailable — never a fabricated source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Observed values, present only when `status` is `available`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<ResourceLimitValues>,
    /// Why limits could not be observed (network unreachable, source
    /// unsupported, malformed response). Present when unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The numeric limits, verbatim from the source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceLimitValues {
    /// Base fee in stroops (`base_fee_in_stroops`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_fee_in_stroops: Option<u32>,
    /// Base reserve in stroops (`base_reserve_in_stroops`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_reserve_in_stroops: Option<u32>,
    /// Max transactions per ledger set (`max_tx_set_size`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tx_set_size: Option<u32>,
    /// Protocol version the ledger closed under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
}

impl ResourceLimits {
    /// Build an `available` limits struct from observed values.
    pub fn available(source: String, values: ResourceLimitValues) -> Self {
        ResourceLimits {
            status: "available".to_string(),
            source: Some(source),
            values: Some(values),
            reason: None,
        }
    }

    /// Build an `unavailable` limits struct. The reason is mandatory so the
    /// absence of data is explainable.
    pub fn unavailable(reason: String) -> Self {
        ResourceLimits {
            status: "unavailable".to_string(),
            source: None,
            values: None,
            reason: Some(reason),
        }
    }
}

/// Horizon `GET /` (root) resource, read for identity/protocol cross-check.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HorizonRoot {
    pub horizon_version: Option<String>,
    pub core_version: Option<String>,
    pub network_passphrase: Option<String>,
    pub current_protocol_version: Option<u32>,
    pub supported_protocol_version: Option<u32>,
    pub history_latest_ledger: Option<u32>,
}

/// Horizon `GET /ledgers/{sequence}` resource, read for authoritative limits.
///
/// Field names are snake_case on the wire (`base_fee_in_stroops`, …), unlike
/// the camelCase JSON-RPC payloads — deserializing them as camelCase silently
/// produced all-`None` limits and reported `unavailable` against a Horizon
/// that had answered fine.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct HorizonLedger {
    pub base_fee_in_stroops: Option<u32>,
    pub base_reserve_in_stroops: Option<u32>,
    pub max_tx_set_size: Option<u32>,
    pub protocol_version: Option<u32>,
}

use serde::Deserialize;

/// One read-only probe of an RPC endpoint + its derived Horizon API.
pub struct Diagnosis {
    /// Canonical network name (`testnet`, `mainnet`, `futurenet`, profile
    /// name, or `custom-…` derived from the passphrase).
    pub network: String,
    pub rpc_url: String,
    /// Passphrase the local configuration expects for this target.
    pub configured_passphrase: String,
    /// Passphrase the endpoint self-reports (`getNetwork`), if any.
    pub endpoint_passphrase: Option<String>,
    pub identity_status: IdentityStatus,
    /// Node health string (`healthy`/…), if answered.
    pub health_status: Option<String>,
    /// RPC sequence + protocol version from `getLatestLedger`.
    pub latest_ledger: Option<u32>,
    pub rpc_ledger_protocol: Option<u32>,
    /// Protocol version from `getNetwork`.
    pub rpc_network_protocol: Option<u32>,
    pub protocol_status: ProtocolStatus,
    /// Horizon availability + versions (root resource), if answered.
    pub horizon_available: bool,
    pub horizon_endpoint: String,
    pub horizon_version: Option<String>,
    pub core_version: Option<String>,
    pub horizon_network_passphrase: Option<String>,
    pub horizon_protocol: Option<u32>,
    pub resource_limits: ResourceLimits,
    /// Transport-level error, when the RPC endpoint could not be reached.
    pub rpc_error: Option<String>,
    /// Why Horizon did not answer, when it did not.
    pub horizon_error: Option<String>,
}

/// Serialized report (JSON schema).
#[derive(Debug, Serialize)]
pub struct NetworkDiagnosisReport {
    pub schema_version: u32,
    pub network: String,
    pub rpc_url: String,
    pub status: String,
    pub identity: IdentitySection,
    pub health: HealthSection,
    pub protocol: ProtocolSection,
    pub environment: EnvironmentSection,
    pub horizon: HorizonSection,
    pub resource_limits: ResourceLimits,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IdentitySection {
    pub configured_passphrase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_passphrase: Option<String>,
    pub status: IdentityStatus,
}

#[derive(Debug, Serialize)]
pub struct HealthSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProtocolSection {
    pub status: ProtocolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_get_network: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_get_ledger: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizon: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentSection {
    /// Where the network name came from: `flag`, `profile`, `config` or
    /// `passphrase` (classification of the local configuration).
    pub network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_ledger: Option<u32>,
    /// The local tool's own version — the assumption being verified.
    pub sdkt_version: String,
}

#[derive(Debug, Serialize)]
pub struct HorizonSection {
    pub available: bool,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizon_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Diagnosis {
    /// Overall status: `unreachable` (no RPC answer), `degraded` (identity
    /// mismatch, protocol inconsistency, or unhealthy node), else `ok`.
    pub fn status(&self) -> &'static str {
        if self.rpc_error.is_some() {
            return "unreachable";
        }
        if self.identity_status == IdentityStatus::Mismatch
            || self.protocol_status == ProtocolStatus::Inconsistent
            || self
                .health_status
                .as_deref()
                .is_some_and(|s| !s.eq_ignore_ascii_case("healthy"))
        {
            return "degraded";
        }
        "ok"
    }

    pub fn into_report(self) -> NetworkDiagnosisReport {
        let status = self.status().to_string();
        let sdkt_version = env!("CARGO_PKG_VERSION").to_string();
        NetworkDiagnosisReport {
            schema_version: DIAGNOSIS_SCHEMA_VERSION,
            network: self.network.clone(),
            rpc_url: self.rpc_url.clone(),
            status,
            identity: IdentitySection {
                configured_passphrase: self.configured_passphrase,
                endpoint_passphrase: self.endpoint_passphrase,
                status: self.identity_status,
            },
            health: HealthSection {
                status: self.health_status,
            },
            protocol: ProtocolSection {
                status: self.protocol_status,
                rpc_get_network: self.rpc_network_protocol,
                rpc_get_ledger: self.rpc_ledger_protocol,
                horizon: self.horizon_protocol,
            },
            environment: EnvironmentSection {
                network: self.network,
                latest_ledger: self.latest_ledger,
                sdkt_version,
            },
            horizon: HorizonSection {
                available: self.horizon_available,
                endpoint: self.horizon_endpoint,
                horizon_version: self.horizon_version,
                core_version: self.core_version,
                network_passphrase: self.horizon_network_passphrase,
                error: self.horizon_error,
            },
            resource_limits: self.resource_limits,
            error: self.rpc_error,
        }
    }
}

/// Probe an RPC target. Never mutates anything: only `getHealth`,
/// `getNetwork`, `getLatestLedger`, plus read-only Horizon GETs.
pub async fn diagnose(target_name: &str, target: &sdkt_core::NetworkConfig) -> Diagnosis {
    let client = SorobanRpcClient::from_config(target);
    let configured_passphrase = target.passphrase.clone();
    let network = network_key(Some(target_name), &configured_passphrase);
    let horizon_endpoint = horizon_url_for_endpoint(&target.rpc_url);

    // RPC probe: identity + protocol + health, in one round each.
    let mut endpoint_passphrase = None;
    let mut rpc_network_protocol = None;
    let mut latest_ledger = None;
    let mut rpc_ledger_protocol = None;
    let mut health_status = None;
    let mut rpc_error = None;

    match client.get_network().await {
        Ok(info) => {
            endpoint_passphrase = Some(info.passphrase);
            rpc_network_protocol = Some(info.protocol_version);
            latest_ledger = info.latest_ledger;
        }
        Err(e) => rpc_error = Some(format!("getNetwork failed: {e}")),
    }
    if let Ok(ledger) = client.get_ledger().await {
        latest_ledger = Some(ledger.sequence);
        rpc_ledger_protocol = Some(ledger.protocol_version);
    }
    if let Ok(h) = client.get_health().await {
        health_status = Some(h.status);
    }
    if rpc_error.is_none() && endpoint_passphrase.is_none() && latest_ledger.is_none() {
        rpc_error = Some("the RPC endpoint did not answer any probe".to_string());
    }

    let identity_status = classify_identity(&configured_passphrase, endpoint_passphrase.as_deref());

    // Horizon: identity cross-check + authoritative ledger limits. Failures
    // degrade to `unavailable` with a reason; they never fabricate values.
    let mut horizon_available = false;
    let mut horizon_version = None;
    let mut core_version = None;
    let mut horizon_passphrase = None;
    let mut horizon_protocol = None;
    let mut horizon_error = None;
    let mut resource_limits = ResourceLimits::unavailable(
        "Horizon did not answer; resource limits cannot be observed from this target".to_string(),
    );

    match fetch_horizon_root(&client, &horizon_endpoint).await {
        Ok(root) => {
            horizon_available = true;
            horizon_version = root.horizon_version;
            core_version = root.core_version;
            horizon_passphrase = root.network_passphrase;
            horizon_protocol = root.current_protocol_version;
            if let Some(seq) = root.history_latest_ledger {
                match fetch_horizon_ledger(&client, &horizon_endpoint, seq).await {
                    Ok(Some(ledger)) => {
                        resource_limits = ResourceLimits::available(
                            format!("horizon ledger {seq}"),
                            ResourceLimitValues {
                                base_fee_in_stroops: ledger.base_fee_in_stroops,
                                base_reserve_in_stroops: ledger.base_reserve_in_stroops,
                                max_tx_set_size: ledger.max_tx_set_size,
                                protocol_version: ledger.protocol_version,
                            },
                        );
                    }
                    Ok(None) => {
                        resource_limits = ResourceLimits::unavailable(format!(
                            "Horizon ledger {seq} resource did not include limit fields"
                        ));
                    }
                    Err(e) => {
                        resource_limits = ResourceLimits::unavailable(format!(
                            "Horizon ledger resource unreachable: {e}"
                        ));
                    }
                }
            } else {
                resource_limits = ResourceLimits::unavailable(
                    "Horizon root did not report a latest ledger to query limits from".to_string(),
                );
            }
        }
        Err(e) => {
            horizon_error = Some(e);
        }
    }

    // Horizon's network_passphrase is a second identity observation; it is
    // reported in the horizon section so a consumer can cross-check it, while
    // the RPC getNetwork answer remains the authoritative identity source.
    let protocol_status =
        classify_protocol(rpc_network_protocol, rpc_ledger_protocol, horizon_protocol);

    Diagnosis {
        network,
        rpc_url: target.rpc_url.clone(),
        configured_passphrase,
        endpoint_passphrase,
        identity_status,
        health_status,
        latest_ledger,
        rpc_ledger_protocol,
        rpc_network_protocol,
        protocol_status,
        horizon_available,
        horizon_endpoint,
        horizon_version,
        core_version,
        horizon_network_passphrase: horizon_passphrase,
        horizon_protocol,
        resource_limits,
        rpc_error,
        horizon_error,
    }
}

/// `GET {horizon}/` — the server-info resource (read-only).
async fn fetch_horizon_root(
    client: &SorobanRpcClient,
    horizon_base: &str,
) -> Result<HorizonRoot, String> {
    let url = format!("{}/", horizon_base.trim_end_matches('/'));
    let res = client
        .http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Horizon request error: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("Horizon returned status {}", res.status()));
    }
    let root: HorizonRoot = res
        .json()
        .await
        .map_err(|e| format!("Horizon root parse error: {e}"))?;
    Ok(root)
}

/// `GET {horizon}/ledgers/{sequence}` — one ledger's authoritative limits.
///
/// Returns `Ok(None)` when the resource answers but carries no limit fields
/// (so the caller reports `unavailable`, not fabricated zeros).
async fn fetch_horizon_ledger(
    client: &SorobanRpcClient,
    horizon_base: &str,
    sequence: u32,
) -> Result<Option<HorizonLedger>, String> {
    let url = format!(
        "{}/ledgers/{}",
        horizon_base.trim_end_matches('/'),
        sequence
    );
    let res = client
        .http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Horizon request error: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("Horizon returned status {}", res.status()));
    }
    let ledger: HorizonLedger = res
        .json()
        .await
        .map_err(|e| format!("Horizon ledger parse error: {e}"))?;
    if ledger.base_fee_in_stroops.is_none()
        && ledger.base_reserve_in_stroops.is_none()
        && ledger.max_tx_set_size.is_none()
    {
        return Ok(None);
    }
    Ok(Some(ledger))
}

/// Resolve the target exactly like `verify` / `release-assurance` do, then
/// run the read-only probe and print (or serialize) the report.
pub async fn run_network_diagnose(
    network: Option<String>,
    net: &NetworkArgs,
    format: &sdkt_core::OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = match resolve_target_network(network.as_deref(), net) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    let diagnosis = diagnose(&target.network_name, &target.config).await;
    let report = diagnosis.into_report();

    if *format == sdkt_core::OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| format!("serializing report: {e}"))?
        );
    } else {
        print_diagnosis_pretty(&report);
    }

    // Exit contract: 0 = ok, 1 = degraded or unreachable. Consumers gate on
    // the exit code without parsing; `--format json` carries the detail.
    if report.status != "ok" {
        std::process::exit(1);
    }
    Ok(())
}

fn print_diagnosis_pretty(r: &NetworkDiagnosisReport) {
    println!("Network Diagnosis");
    println!("=================");
    println!("Network        : {}", r.network);
    println!("RPC URL        : {}", r.rpc_url);
    println!("Status         : {}", r.status);
    println!(
        "Node health    : {}",
        r.health.status.as_deref().unwrap_or("unknown")
    );
    println!(
        "Passphrase     : {} ({:?})",
        r.identity.configured_passphrase, r.identity.status
    );
    match r.identity.endpoint_passphrase.as_deref() {
        Some(ep) if *ep != r.identity.configured_passphrase => {
            println!("Endpoint says  : {ep}  ← MISMATCH");
        }
        Some(ep) => println!("Endpoint says  : {ep}"),
        None => println!("Endpoint says  : not reported"),
    }
    println!(
        "Protocol       : rpc(getNetwork)={} rpc(getLedger)={} horizon={} ({:?})",
        opt(r.protocol.rpc_get_network),
        opt(r.protocol.rpc_get_ledger),
        opt(r.protocol.horizon),
        r.protocol.status
    );
    if let Some(seq) = r.environment.latest_ledger {
        println!("Latest ledger  : {seq}");
    }
    println!("sdkt           : {}", r.environment.sdkt_version);
    if r.horizon.available {
        println!(
            "Horizon        : {} ({} / {})",
            r.horizon.endpoint,
            r.horizon.horizon_version.as_deref().unwrap_or("?"),
            r.horizon.core_version.as_deref().unwrap_or("?")
        );
    } else {
        println!(
            "Horizon        : unavailable ({})",
            r.horizon.error.as_deref().unwrap_or("not reachable")
        );
    }
    match &r.resource_limits {
        ResourceLimits {
            status,
            source: Some(src),
            values: Some(v),
            ..
        } => {
            println!(
                "Resource limits: {status} — fee={} reserve={} tx_set={} (source: {src})",
                v.base_fee_in_stroops
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into()),
                v.base_reserve_in_stroops
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into()),
                v.max_tx_set_size
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into()),
            );
        }
        ResourceLimits { status, reason, .. } => {
            println!(
                "Resource limits: {status} — {}",
                reason.as_deref().unwrap_or("no reason reported")
            );
        }
    }
    if let Some(e) = &r.horizon.error {
        println!("Horizon error  : {e}");
    }
}

fn opt(v: Option<u32>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_match_mismatch_unknown() {
        assert_eq!(
            classify_identity(
                "Test SDF Network ; September 2015",
                Some("Test SDF Network ; September 2015")
            ),
            IdentityStatus::Match
        );
        assert_eq!(
            classify_identity(
                "Test SDF Network ; September 2015",
                Some("Public Global Stellar Network ; September 2015")
            ),
            IdentityStatus::Mismatch
        );
        assert_eq!(
            classify_identity("Test SDF Network ; September 2015", None),
            IdentityStatus::Unknown
        );
    }

    #[test]
    fn protocol_agreement_and_disagreement() {
        assert_eq!(
            classify_protocol(Some(29), Some(29), Some(29)),
            ProtocolStatus::Consistent
        );
        assert_eq!(
            classify_protocol(Some(29), Some(29), None),
            ProtocolStatus::Consistent
        );
        assert_eq!(
            classify_protocol(Some(29), Some(28), None),
            ProtocolStatus::Inconsistent
        );
        assert_eq!(
            classify_protocol(Some(29), None, Some(28)),
            ProtocolStatus::Inconsistent
        );
        assert_eq!(classify_protocol(None, None, None), ProtocolStatus::Unknown);
    }

    #[test]
    fn limits_unavailable_never_fabricates_values() {
        let l = ResourceLimits::unavailable("Horizon unreachable".into());
        assert_eq!(l.status, "unavailable");
        assert!(l.values.is_none());
        assert!(l.source.is_none());
        assert!(l.reason.unwrap().contains("unreachable"));
    }

    #[test]
    fn limits_available_carries_source() {
        let l = ResourceLimits::available(
            "horizon ledger 123".into(),
            ResourceLimitValues {
                base_fee_in_stroops: Some(100),
                base_reserve_in_stroops: Some(5_000_000),
                max_tx_set_size: Some(200),
                protocol_version: Some(29),
            },
        );
        assert_eq!(l.status, "available");
        assert_eq!(l.source.as_deref(), Some("horizon ledger 123"));
        let v = l.values.unwrap();
        assert_eq!(v.base_fee_in_stroops, Some(100));
        assert_eq!(v.max_tx_set_size, Some(200));
    }

    #[test]
    fn report_status_degrades_on_identity_mismatch() {
        let d = base_diagnosis();
        assert_eq!(d.status(), "ok");
        let mut mismatch = base_diagnosis();
        mismatch.identity_status = IdentityStatus::Mismatch;
        assert_eq!(mismatch.status(), "degraded");
        let mut proto = base_diagnosis();
        proto.protocol_status = ProtocolStatus::Inconsistent;
        assert_eq!(proto.status(), "degraded");
        let mut unhealthy = base_diagnosis();
        unhealthy.health_status = Some("degraded".into());
        assert_eq!(unhealthy.status(), "degraded");
        let mut dead = base_diagnosis();
        dead.rpc_error = Some("connection refused".into());
        assert_eq!(dead.status(), "unreachable");
    }

    #[test]
    fn report_json_is_versioned_and_serializable() {
        let report = base_diagnosis().into_report();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema_version"], DIAGNOSIS_SCHEMA_VERSION);
        assert_eq!(json["identity"]["status"], "match");
        assert_eq!(json["resource_limits"]["status"], "unavailable");
        // No fabricated limit numbers in the unavailable case.
        assert!(json["resource_limits"].get("values").is_none());
        assert!(json["environment"]["sdkt_version"].is_string());
    }

    fn base_diagnosis() -> Diagnosis {
        Diagnosis {
            network: "testnet".to_string(),
            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
            configured_passphrase: "Test SDF Network ; September 2015".to_string(),
            endpoint_passphrase: Some("Test SDF Network ; September 2015".to_string()),
            identity_status: IdentityStatus::Match,
            health_status: Some("healthy".to_string()),
            latest_ledger: Some(5032887),
            rpc_ledger_protocol: Some(29),
            rpc_network_protocol: Some(29),
            protocol_status: ProtocolStatus::Consistent,
            horizon_available: true,
            horizon_endpoint: "https://horizon-testnet.stellar.org".to_string(),
            horizon_version: Some("29.0.0".to_string()),
            core_version: Some("stellar-core 29.0.0".to_string()),
            horizon_network_passphrase: Some("Test SDF Network ; September 2015".to_string()),
            horizon_protocol: Some(29),
            resource_limits: ResourceLimits::unavailable("no ledger queried".into()),
            rpc_error: None,
            horizon_error: None,
        }
    }
}
