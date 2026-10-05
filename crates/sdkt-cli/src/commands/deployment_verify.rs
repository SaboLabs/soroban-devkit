//! Deployment verification — is the deployed contract the artifact I built?
//!
//! Phase A foundation for release assurance. One command, `sdkt deployment
//! verify`, answers exactly one question with evidence: does the WASM I have
//! locally match the WASM deployed at an explicit contract ID on an explicit
//! network target?
//!
//! ```text
//! local WASM → artifact identity (offline SHA-256)
//!            → deployed executable (raw ledger probe, no inspection shortcuts)
//!            → MATCH / DRIFT / UNKNOWN / NOT_FOUND
//! ```
//!
//! Design rules this module is built to honour:
//!
//! - **Explicit targets.** The contract ID and the network target are both
//!   required arguments; nothing is guessed. Network resolution goes through
//!   `resolve_target_network`, the same precedence every other network-aware
//!   command uses.
//! - **Read-only.** Only `getLedgerEntries` reads are issued. No envelope is
//!   built, nothing is signed, submitted, or deployed. The mainnet-safety
//!   guard for mutating commands deliberately does not apply — like `verify`
//!   and `release-assurance`, this command cannot mutate.
//! - **Reachable ≠ verified.** A successful HTTP call proves connectivity
//!   only. The verdict requires the deployed executable itself to be
//!   readable; if it cannot be obtained authoritatively the result is
//!   `UNKNOWN` with a reason, never `MATCH`.
//! - **Three non-WASM deployed shapes are distinguished, not conflated:**
//!   `NOT_FOUND` (no contract instance entry at all), and `UNKNOWN`
//!   (`stellar_asset` protocol code that has no WASM artifact, CAP-85
//!   `external_ref` executables the probe cannot resolve, or an entry that
//!   cannot be decoded).
//!
//! The report is a versioned, machine-readable JSON document
//! ([`DEPLOYMENT_SCHEMA_VERSION`]) so a future release-assurance revision can
//! consume it as a section input without re-deriving any of it.

use crate::commands::network::resolve_target_network;
use crate::NetworkArgs;
use sdkt_rpc::probe_deployed_executable;
use sdkt_xdr::ContractExecutableRef;
use serde::Serialize;

/// Schema version of the machine-readable deployment-verification report.
/// Bump on breaking field changes only; additive fields keep the version.
pub const DEPLOYMENT_SCHEMA_VERSION: u32 = 1;

/// How a local artifact compares against a deployed contract's executable.
///
/// `Display` yields the stable UPPER_SNAKE token used in JSON output and CI
/// gating: `MATCH`, `DRIFT`, `UNKNOWN`, `NOT_FOUND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentVerdict {
    /// The deployed WASM hash equals the local artifact's hash.
    Match,
    /// The deployed WASM hash differs from the local artifact's hash. Both
    /// sides are authoritative; the artifacts simply are not the same code.
    Drift,
    /// The deployed executable could not be compared authoritatively: the
    /// contract is protocol-defined (`stellar_asset`), CAP-85 `external_ref`
    /// (not resolved by this read-only probe), undecodable, or the code entry
    /// could not be read. `reason` explains which.
    Unknown,
    /// The contract instance does not exist at this target (no ledger entry).
    NotFound,
}

impl std::fmt::Display for DeploymentVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let token = match self {
            DeploymentVerdict::Match => "MATCH",
            DeploymentVerdict::Drift => "DRIFT",
            DeploymentVerdict::Unknown => "UNKNOWN",
            DeploymentVerdict::NotFound => "NOT_FOUND",
        };
        f.write_str(token)
    }
}

/// The deployed executable kind, as reported by the ledger probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableKind {
    /// A plain WASM executable (hash available when the read succeeded).
    Wasm,
    /// Stellar Asset Contract: protocol-defined code, no WASM artifact exists.
    StellarAsset,
    /// CAP-85 external executable reference.
    ExternalRef,
    /// The instance entry could not be decoded, so its shape is unknown.
    Undecodable,
    /// The probe was not run / produced no executable information.
    None,
}

impl std::fmt::Display for ExecutableKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let token = match self {
            ExecutableKind::Wasm => "wasm",
            ExecutableKind::StellarAsset => "stellar_asset",
            ExecutableKind::ExternalRef => "external_ref",
            ExecutableKind::Undecodable => "undecodable",
            ExecutableKind::None => "none",
        };
        f.write_str(token)
    }
}

/// Machine-readable deployment-verification report (schema versioned).
#[derive(Debug, Serialize)]
pub struct DeploymentVerificationReport {
    pub schema_version: u32,
    /// The contract ID exactly as supplied.
    pub contract_id: String,
    /// Canonical network name resolved for the verification (e.g. `testnet`,
    /// `mainnet`, `futurenet`, or the saved profile name).
    pub network: String,
    /// The RPC endpoint queried. Recorded so the same verdict can be audited
    /// against the same source later.
    pub rpc_url: String,
    /// UPPER_SNAKE verdict token: MATCH | DRIFT | UNKNOWN | NOT_FOUND.
    pub verdict: String,
    /// SHA-256 hex of the local WASM, when a local artifact was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_wasm_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_wasm_size_bytes: Option<usize>,
    /// The deployed executable kind, when the instance was reachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployed_executable: Option<ExecutableKind>,
    /// SHA-256 hex of the deployed WASM, only when one exists AND was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployed_wasm_hash: Option<String>,
    /// Why the verdict is what it is. Required for `UNKNOWN`/`NOT_FOUND`, so
    /// a consumer never has to guess what could not be determined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// True only for `MATCH`. Everything else — including a reachable RPC —
    /// is not a deployment verification.
    pub verified: bool,
}

/// Pure comparison used by the orchestrator and unit-tested directly.
///
/// Returns `(deployed_executable, deployed_wasm_hash, verdict, reason)`.
/// Keeping this free of network types makes the four verdicts testable
/// without an RPC at all.
pub fn compare_deployment(
    executable: ExecutableKind,
    deployed_hash: Option<String>,
    local: Option<(String, usize)>,
    instance_found: bool,
) -> (
    ExecutableKind,
    Option<String>,
    DeploymentVerdict,
    Option<String>,
) {
    if !instance_found {
        return (
            ExecutableKind::None,
            None,
            DeploymentVerdict::NotFound,
            Some("no contract instance ledger entry exists at this network target".to_string()),
        );
    }
    match executable {
        ExecutableKind::Wasm => match (deployed_hash.clone(), local) {
            (_, None) => (
                executable,
                deployed_hash,
                DeploymentVerdict::Unknown,
                Some(
                    "no local WASM supplied; the deployed hash was read but there is nothing to \
                     compare it against — pass --wasm <file> for a verdict"
                        .to_string(),
                ),
            ),
            (None, Some(_)) => (
                executable,
                None,
                DeploymentVerdict::Unknown,
                Some(
                    "the deployed WASM code entry could not be read from this endpoint, so the \
                     hash is not authoritatively available"
                        .to_string(),
                ),
            ),
            (Some(dh), Some((lh, _))) if dh == lh => {
                (executable, deployed_hash, DeploymentVerdict::Match, None)
            }
            (Some(dh), Some((lh, _))) => (
                executable,
                deployed_hash,
                DeploymentVerdict::Drift,
                Some(format!("deployed WASM {dh} differs from local WASM {lh}")),
            ),
        },
        ExecutableKind::StellarAsset => (
            executable,
            None,
            DeploymentVerdict::Unknown,
            Some(
                "the contract executable is `stellar_asset` (protocol 28): protocol-defined code \
                 with no WASM artifact, so there is no hash to compare"
                    .to_string(),
            ),
        ),
        ExecutableKind::ExternalRef => match (deployed_hash.clone(), local) {
            // A resolved CAP-85 external ref carries an authoritative hash
            // (the same owner-entry lookup `inspect` uses) and compares like
            // inline Wasm; an unresolved one has no hash and cannot compare.
            (Some(dh), Some((lh, _))) if dh == lh => {
                (executable, deployed_hash, DeploymentVerdict::Match, None)
            }
            (Some(dh), Some((lh, _))) => (
                executable,
                deployed_hash,
                DeploymentVerdict::Drift,
                Some(format!("deployed WASM {dh} differs from local WASM {lh}")),
            ),
            (Some(dh), None) => (
                executable,
                deployed_hash,
                DeploymentVerdict::Unknown,
                Some(format!(
                    "resolved external executable hash is {dh} but no local WASM was supplied"
                )),
            ),
            (None, _) => (
                executable,
                None,
                DeploymentVerdict::Unknown,
                Some(
                    "the CAP-85 external executable could not be resolved to a WASM hash \
                     through the owner-entry lookup"
                        .to_string(),
                ),
            ),
        },
        ExecutableKind::Undecodable => (
            executable,
            None,
            DeploymentVerdict::Unknown,
            Some("the contract instance ledger entry could not be decoded".to_string()),
        ),
        ExecutableKind::None => (
            executable,
            None,
            DeploymentVerdict::Unknown,
            Some("no deployed executable information was produced by the probe".to_string()),
        ),
    }
}

/// Orchestrate the read-only deployment verification and print the report.
///
/// Local hashing happens before any network call, so a bad/missing artifact
/// fails fast without touching the endpoint. The deployed executable is
/// obtained by [`probe_deployed_executable`], which never downloads code and
/// never converts a non-Wasm executable into an error.
pub async fn run_deployment_verify(
    contract: &str,
    wasm: Option<&str>,
    network: Option<String>,
    net: &NetworkArgs,
    format: &sdkt_core::OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    // Offline artifact identity first: a missing or unparseable file is an
    // input error, not a network one.
    let local = match wasm {
        Some(path) => {
            let bytes =
                std::fs::read(path).map_err(|e| format!("Error reading WASM file {path}: {e}"))?;
            let meta = sdkt_wasm::parse_metadata(&bytes)
                .map_err(|e| format!("{path} is not valid WASM: {e}"))?;
            Some((meta.hash, meta.size_bytes))
        }
        None => None,
    };

    // Read-only boundary: same resolution as `verify` / `release-assurance`.
    let target = match resolve_target_network(network.as_deref(), net) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    let network_name = target.network_name;

    let probe = match probe_deployed_executable(&target.client, contract).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Map the wire executable union onto the report's stable kinds. A
    // resolved CAP-85 external ref carries an authoritative hash (same owner
    // lookup `inspect` uses), so it stays comparable; an unresolved one has
    // no hash and compares as UNKNOWN below.
    let executable = match &probe.executable {
        Some(ContractExecutableRef::Wasm(_)) => ExecutableKind::Wasm,
        Some(ContractExecutableRef::StellarAsset) => ExecutableKind::StellarAsset,
        Some(ContractExecutableRef::ExternalRef(_)) => ExecutableKind::ExternalRef,
        None => ExecutableKind::Undecodable,
    };
    let (executable, deployed_hash, verdict, reason) = compare_deployment(
        executable,
        probe.wasm_hash,
        local.clone(),
        probe.instance_found,
    );

    let report = DeploymentVerificationReport {
        schema_version: DEPLOYMENT_SCHEMA_VERSION,
        contract_id: contract.to_string(),
        network: network_name,
        rpc_url: target.config.rpc_url.clone(),
        verdict: verdict.to_string(),
        local_wasm_hash: local.as_ref().map(|(h, _)| h.clone()),
        local_wasm_size_bytes: local.as_ref().map(|(_, s)| *s),
        deployed_executable: Some(executable),
        deployed_wasm_hash: deployed_hash,
        reason,
        verified: verdict == DeploymentVerdict::Match,
    };

    if *format == sdkt_core::OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| format!("serializing report: {e}"))?
        );
    } else {
        print_deployment_pretty(&report);
    }

    // Exit contract: only MATCH is a pass. DRIFT, UNKNOWN and NOT_FOUND all
    // fail a caller/CI step; transport errors exit non-zero above.
    if !report.verified {
        std::process::exit(1);
    }
    Ok(())
}

fn print_deployment_pretty(r: &DeploymentVerificationReport) {
    println!("Deployment Verification Report");
    println!("==============================");
    println!("Contract ID : {}", r.contract_id);
    println!("Network     : {}", r.network);
    println!("Verdict     : {}", r.verdict);
    if let (Some(lh), Some(sz)) = (&r.local_wasm_hash, r.local_wasm_size_bytes) {
        println!("Local WASM  : {lh}   ({sz} bytes)");
    }
    if let (Some(kind), Some(dh)) = (&r.deployed_executable, &r.deployed_wasm_hash) {
        println!("Deployed    : {kind} {dh}");
    } else {
        println!(
            "Deployed    : {}",
            r.deployed_executable
                .map(|k| k.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
    }
    if let Some(reason) = &r.reason {
        println!();
        println!("{reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_hashes_are_match() {
        let (_, _, verdict, reason) = compare_deployment(
            ExecutableKind::Wasm,
            Some("abc123".into()),
            Some(("abc123".into(), 42)),
            true,
        );
        assert_eq!(verdict, DeploymentVerdict::Match);
        assert_eq!(verdict.to_string(), "MATCH");
        assert!(reason.is_none());
    }

    #[test]
    fn different_hashes_are_drift() {
        let (_, hash, verdict, reason) = compare_deployment(
            ExecutableKind::Wasm,
            Some("abc123".into()),
            Some(("def456".into(), 42)),
            true,
        );
        assert_eq!(verdict, DeploymentVerdict::Drift);
        assert_eq!(hash.as_deref(), Some("abc123"));
        let reason = reason.expect("drift carries a reason");
        assert!(
            reason.contains("abc123") && reason.contains("def456"),
            "{reason}"
        );
    }

    #[test]
    fn missing_instance_is_not_found_not_unknown() {
        let (exec, hash, verdict, reason) =
            compare_deployment(ExecutableKind::None, None, Some(("abc".into(), 1)), false);
        assert_eq!(verdict, DeploymentVerdict::NotFound);
        assert_eq!(exec, ExecutableKind::None);
        assert!(hash.is_none());
        assert!(reason.expect("reason").contains("no contract instance"));
    }

    #[test]
    fn stellar_asset_is_unknown_not_drift() {
        for kind in [
            ExecutableKind::StellarAsset,
            ExecutableKind::ExternalRef,
            ExecutableKind::Undecodable,
        ] {
            let (_, _, verdict, reason) =
                compare_deployment(kind, None, Some(("abc".into(), 1)), true);
            assert_eq!(verdict, DeploymentVerdict::Unknown, "{kind:?}");
            assert!(reason.is_some());
        }
    }

    #[test]
    fn unreadable_deployed_code_is_unknown() {
        let (_, _, verdict, reason) =
            compare_deployment(ExecutableKind::Wasm, None, Some(("abc".into(), 1)), true);
        assert_eq!(verdict, DeploymentVerdict::Unknown);
        assert!(
            reason.expect("reason").contains("could not be read"),
            "RPC reachable must not become a verification"
        );
    }

    #[test]
    fn missing_local_artifact_is_unknown() {
        let (_, hash, verdict, _) =
            compare_deployment(ExecutableKind::Wasm, Some("abc".into()), None, true);
        assert_eq!(verdict, DeploymentVerdict::Unknown);
        // The deployed side is still reported; only the comparison is unknown.
        assert_eq!(hash.as_deref(), Some("abc"));
    }

    #[test]
    fn verdict_tokens_are_stable() {
        assert_eq!(DeploymentVerdict::Match.to_string(), "MATCH");
        assert_eq!(DeploymentVerdict::Drift.to_string(), "DRIFT");
        assert_eq!(DeploymentVerdict::Unknown.to_string(), "UNKNOWN");
        assert_eq!(DeploymentVerdict::NotFound.to_string(), "NOT_FOUND");
    }
}
