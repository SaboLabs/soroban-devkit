//! Planner: [`Intent`] + registry → a validated argv, or a refusal.
//!
//! This is the safety boundary. Nothing reaches the executor without passing
//! through [`plan`], which enforces:
//!
//! - the capability exists in the registry (no second command list);
//! - it is read-only and does not require confirmation;
//! - its required arguments were supplied;
//! - a network-dependent capability gets an **explicit** endpoint, and never
//!   a silently chosen mainnet;
//! - JSON output is only requested when the registry says it exists.
//!
//! A refusal is a value, not an error path: [`Plan::Refused`] carries the
//! reason so the caller can report it without having run anything.

use crate::intent::{self, Intent, ParseError};
use sdkt_core::registry::{Capability, NetworkRequirement, Safety};

/// Testnet endpoint. Used only when the request did not name a network, and
/// only because it is the documented default the CLI itself would pick.
pub const TESTNET_RPC: &str = "https://soroban-testnet.stellar.org";
/// Public testnet passphrase, matching `sdkt_core::network_safety`.
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// A planned, safe-to-run command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Registry capability the request resolved to.
    pub capability_id: &'static str,
    /// Full argv, including the `--format json` decision.
    pub argv: Vec<String>,
    /// Network actually used, when one applies.
    pub network: Option<String>,
    /// Safety class copied from the registry, for the result document.
    pub safety: Safety,
    /// Whether JSON output was requested (registry said it is supported).
    pub wants_json: bool,
    /// Keywords that selected the intent, for the explanation.
    pub matched_keywords: Vec<String>,
}

impl Plan {
    /// Full command line, including the program name, for display.
    pub fn display(&self) -> String {
        format!("sdkt {}", self.argv.join(" "))
    }
}

/// Why a request will not become a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The request could not be parsed.
    Parse(ParseError),
    /// The registry has no such capability.
    UnknownCapability(&'static str),
    /// The capability is not read-only, or needs confirmation.
    Blocked {
        /// Capability that was refused.
        capability_id: &'static str,
        /// Registry safety class.
        safety: Safety,
        /// Whether the registry demands human confirmation.
        requires_confirmation: bool,
    },
    /// A network was named that this read-only agent will not pick.
    RefusedNetwork(String),
}

impl PlanError {
    /// Machine-stable code for the result document.
    pub fn code(&self) -> &'static str {
        match self {
            PlanError::Parse(e) => e.code(),
            PlanError::UnknownCapability(_) => "unknown_capability",
            PlanError::Blocked { .. } => "blocked_unsafe",
            PlanError::RefusedNetwork(_) => "refused_network",
        }
    }

    /// Human-readable reason.
    pub fn message(&self) -> String {
        match self {
            PlanError::Parse(e) => e.message(),
            PlanError::UnknownCapability(id) => {
                format!("{id} is not in the capability registry")
            }
            PlanError::Blocked {
                capability_id,
                safety,
                requires_confirmation,
            } => {
                let mut why = format!("{capability_id} is classified {safety:?}");
                if *requires_confirmation {
                    why.push_str(" and requires confirmation");
                }
                why.push_str("; the v0.1 agent only runs read-only capabilities");
                why
            }
            PlanError::RefusedNetwork(net) => format!(
                "refusing to target {net}: the v0.1 agent runs read-only checks and will not \
                 select a production network implicitly; name a saved network profile instead"
            ),
        }
    }
}

/// Outcome of planning: either a runnable plan or a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// Safe to execute.
    Run(Plan),
    /// Refused; nothing was executed.
    Refuse(PlanError),
}

/// Parse and plan in one step.
pub fn plan(request: &str) -> Planned {
    match intent::parse(request) {
        Ok(i) => plan_intent(&i),
        // A named mutation is a safety refusal, not a comprehension failure:
        // lift it to `Blocked` so callers see one consistent status.
        Err(intent::ParseError::MutationRequested { capability_id }) => {
            let cap = sdkt_core::registry::find(capability_id);
            Planned::Refuse(PlanError::Blocked {
                capability_id,
                safety: cap.map(|c| c.safety).unwrap_or(Safety::Mutating),
                requires_confirmation: cap.is_some_and(|c| c.requires_confirmation),
            })
        }
        Err(e) => Planned::Refuse(PlanError::Parse(e)),
    }
}

/// Plan an already-parsed intent.
pub fn plan_intent(intent: &Intent) -> Planned {
    let Some(cap) = sdkt_core::registry::find(intent.capability_id) else {
        return Planned::Refuse(PlanError::UnknownCapability(intent.capability_id));
    };

    // ---- Safety gate: before anything is built or run. ----
    if cap.safety != Safety::ReadOnly || cap.requires_confirmation {
        return Planned::Refuse(PlanError::Blocked {
            capability_id: cap.id,
            safety: cap.safety,
            requires_confirmation: cap.requires_confirmation,
        });
    }

    let argv = match build_argv(cap, &intent.args) {
        Ok(argv) => argv,
        Err(e) => return Planned::Refuse(e),
    };

    Planned::Run(Plan {
        capability_id: cap.id,
        argv,
        network: intent.args.network.clone(),
        safety: cap.safety,
        wants_json: cap.formats.json,
        matched_keywords: intent.matched_keywords.clone(),
    })
}

/// Build the argv for a capability.
///
/// Fails with [`PlanError`] rather than emitting a partial command line, so a
/// refused network can never leak into an executed argv.
fn build_argv(cap: &Capability, args: &intent::IntentArgs) -> Result<Vec<String>, PlanError> {
    // ---- Network policy first: refuse before building anything. ----
    if cap.network != NetworkRequirement::None {
        match args.network.as_deref() {
            // Absent or testnet: pin the explicit testnet endpoint +
            // passphrase. This cannot inherit a global default and cannot
            // silently become mainnet.
            None | Some("testnet") => {}
            // Any other named network (mainnet, futurenet, custom) is refused
            // outright rather than guessed at.
            Some(other) => {
                return Err(PlanError::RefusedNetwork(other.to_string()));
            }
        }
    }

    let mut argv: Vec<String> = cap.command.iter().map(|s| (*s).to_string()).collect();

    if let Some(c) = &args.contract {
        if flag_for(cap, "--contract") {
            argv.push("--contract".to_string());
        }
        argv.push(c.clone());
    }
    // Artifact paths are positional or flagged per command, and the registry's
    // own argument lists are the source of that fact: `wasm inspect <f>` takes
    // a positional file, `verify --wasm <f>` a flag, `diff --old-wasm/--new-wasm`
    // two flags. Guessing one shape for all three produced an exit-2 usage
    // error, so the flag is emitted only when the capability declares it.
    if let Some(w) = &args.previous_wasm {
        if flag_for(cap, "--old-wasm") {
            argv.push("--old-wasm".to_string());
        }
        argv.push(w.clone());
    }
    if let Some(w) = &args.wasm {
        for flag in ["--new-wasm", "--wasm"] {
            if flag_for(cap, flag) {
                argv.push(flag.to_string());
                break;
            }
        }
        argv.push(w.clone());
    }
    if let Some(s) = &args.source_path {
        argv.push(s.clone());
    }
    if let Some(f) = &args.function {
        argv.push(f.clone());
    }
    if let Some(p) = &args.profile {
        argv.push(p.clone());
    }

    // Pin an explicit testnet endpoint only for capabilities that actually
    // accept `--rpc-url` (the registry is the source of that fact). Some
    // commands, e.g. `network check`, resolve a saved profile instead and
    // reject the flag.
    if cap.network != NetworkRequirement::None && flag_for(cap, "--rpc-url") {
        argv.push("--rpc-url".to_string());
        argv.push(TESTNET_RPC.to_string());
        argv.push("--network-passphrase".to_string());
        argv.push(TESTNET_PASSPHRASE.to_string());
    }

    if cap.formats.json {
        argv.push("--format".to_string());
        argv.push("json".to_string());
    }

    Ok(argv)
}

/// True when the registry lists this flag for the capability.
fn flag_for(cap: &Capability, flag: &str) -> bool {
    let in_required = cap.required_args.iter().any(|a| a.contains(flag));
    let in_optional = cap.optional_args.iter().any(|a| a.contains(flag));
    in_required || in_optional
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_ok(request: &str) -> Plan {
        match plan(request) {
            Planned::Run(p) => p,
            Planned::Refuse(e) => panic!("expected a plan for {request:?}, got {e:?}"),
        }
    }

    fn plan_err(request: &str) -> PlanError {
        match plan(request) {
            Planned::Run(p) => panic!("expected refusal for {request:?}, got {p:?}"),
            Planned::Refuse(e) => e,
        }
    }

    const CID: &str = "CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV";

    #[test]
    fn health_request_plans_a_valid_argv() {
        let p = plan_ok(&format!("check health of contract {CID} on testnet"));
        assert_eq!(p.capability_id, "health");
        let shown = p.display();
        assert!(shown.starts_with("sdkt health"), "{shown}");
        assert!(shown.contains(&format!("--contract {CID}")), "{shown}");
        assert!(shown.contains("--format json"), "{shown}");
        assert!(
            shown.contains("--rpc-url https://soroban-testnet.stellar.org"),
            "{shown}"
        );
        assert!(p.wants_json);
    }

    #[test]
    fn registry_is_the_source_of_commands() {
        // The argv prefix must match the registry entry verbatim.
        for request in [
            format!("inspect contract {CID}"),
            "inspect wasm us_new.wasm".to_string(),
            "run doctor".to_string(),
        ] {
            let p = plan_ok(&request);
            let cap = sdkt_core::registry::find(p.capability_id).expect("in registry");
            let prefix: Vec<String> = cap
                .argv()
                .split_whitespace()
                .skip(1)
                .map(str::to_string)
                .collect();
            assert_eq!(
                &p.argv[..prefix.len()],
                &prefix[..],
                "argv must start with the registry command for {request:?}"
            );
        }
    }

    #[test]
    fn upgrade_safety_maps_to_diff_flags() {
        let p =
            plan_ok("check whether this wasm is safe to upgrade from us_old.wasm to us_new.wasm");
        assert_eq!(p.capability_id, "diff.upgrade_safety");
        let shown = p.display();
        assert!(shown.contains("--old-wasm us_old.wasm"), "{shown}");
        assert!(shown.contains("--new-wasm us_new.wasm"), "{shown}");
    }

    #[test]
    fn mutating_capabilities_are_blocked_before_argv() {
        for (request, id) in [
            ("deploy this contract to testnet", "deploy"),
            ("invoke the mint function", "invoke"),
            ("submit transaction AAAA", "tx.submit"),
            ("sign transaction AAAA", "tx.sign"),
            ("extend ttl of the contract", "storage.extend"),
            ("restore storage for the contract", "storage.restore"),
            ("deploy project to testnet", "project.deploy"),
            ("fund account alice", "identity.fund"),
        ] {
            let e = plan_err(request);
            match &e {
                PlanError::Blocked { capability_id, .. } => {
                    assert_eq!(*capability_id, id, "{request:?}")
                }
                other => panic!("{request:?}: expected Blocked, got {other:?}"),
            }
        }
    }

    #[test]
    fn mainnet_is_refused_rather_than_silently_chosen() {
        let e = plan_err(&format!("check health of contract {CID} on mainnet"));
        assert!(
            matches!(
                e.code(),
                "refused_network" | "blocked_unsafe" | "unrecognised_request"
            ),
            "unexpected code {}",
            e.code()
        );
        // And it must not have silently built a mainnet argv.
        if let Planned::Run(p) = plan(&format!("check health of contract {CID} on mainnet")) {
            assert!(
                !p.display().contains("mainnet"),
                "mainnet leaked into argv: {}",
                p.display()
            );
        }
    }

    #[test]
    fn ambiguous_request_never_executes() {
        let e = plan_err("check health and verify the contract");
        assert_eq!(e.code(), "ambiguous_request");
    }

    #[test]
    fn missing_argument_never_executes() {
        assert_eq!(
            plan_err("check health of the contract").code(),
            "missing_argument"
        );
        assert_eq!(
            plan_err("verify contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV")
                .code(),
            "missing_argument"
        );
    }

    #[test]
    fn offline_capabilities_get_no_network_flags() {
        let p = plan_ok("audit src/lib.rs");
        let shown = p.display();
        assert!(shown.starts_with("sdkt audit src/lib.rs"), "{shown}");
        assert!(!shown.contains("--rpc-url"), "{shown}");
    }

    #[test]
    fn json_only_requested_when_registry_offers_it() {
        let with = plan_ok("inspect wasm us_new.wasm");
        assert!(with.display().contains("--format json"));
        assert!(with.wants_json);
    }

    #[test]
    fn every_blocked_capability_is_absent_from_any_plan() {
        // Belt and braces: no planned argv may start with a mutating command.
        let mutating: Vec<String> = sdkt_core::registry::capabilities()
            .iter()
            .filter(|c| c.safety == Safety::Mutating)
            .map(|c| c.command.join(" "))
            .collect();
        for request in [
            format!("inspect contract {CID}"),
            "inspect wasm a.wasm".to_string(),
            "run doctor".to_string(),
            "audit src/lib.rs".to_string(),
        ] {
            if let Planned::Run(p) = plan(&request) {
                for m in &mutating {
                    assert!(
                        !p.display().starts_with(&format!("{m} ")) && p.display() != *m,
                        "{request:?} planned a mutating command {m:?}"
                    );
                }
            }
        }
    }
}
