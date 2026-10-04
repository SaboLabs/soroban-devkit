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
    /// The request named an artifact this capability has no argument for.
    /// Refused rather than emitted positionally, which would build a command
    /// line the CLI rejects with a usage error.
    UnmappableArgument {
        /// Capability that cannot express the request.
        capability_id: &'static str,
        /// Role that has no matching flag in the registry.
        argument: &'static str,
    },
}

impl PlanError {
    /// Machine-stable code for the result document.
    pub fn code(&self) -> &'static str {
        match self {
            PlanError::Parse(e) => e.code(),
            PlanError::UnknownCapability(_) => "unknown_capability",
            PlanError::Blocked { .. } => "blocked_unsafe",
            PlanError::RefusedNetwork(_) => "refused_network",
            PlanError::UnmappableArgument { .. } => "unmappable_argument",
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
            PlanError::UnmappableArgument {
                capability_id,
                argument,
            } => format!(
                "{capability_id} has no argument for the {argument}; refusing to build a \
                 command line the CLI would reject. Ask for one artifact at a time."
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
    // a positional file, `verify --wasm <f>` a flag, `diff --old-wasm /
    // --new-wasm` and `release-assurance --wasm / --previous-wasm` two flags
    // under different names. The flag for each role is therefore looked up in
    // the capability's argument lists rather than fixed to one spelling — the
    // previous spelling-only mapping emitted `release-assurance <candidate>
    // --wasm <baseline>`, which clap rejected with an exit-2 usage error.
    // Resolve each artifact's flag from the registry, then emit the pair in the
    // order the registry declares them: `diff` lists `--old-wasm` first, so the
    // baseline leads; `release-assurance` lists the required `--wasm` first, so
    // the candidate leads. Ordering therefore follows the capability rather
    // than per-command code in this planner.
    let mut artifacts: Vec<(usize, Vec<String>)> = Vec::new();
    if let Some(w) = &args.previous_wasm {
        match flag_for_any(cap, BASELINE_FLAGS) {
            Some(flag) => {
                artifacts.push((arg_position(cap, flag), vec![flag.to_string(), w.clone()]))
            }
            // No flag for a baseline artifact means this capability does not
            // take one. Emitting the path anyway would put it in positional
            // position and reproduce the malformed-command bug above, so the
            // request is refused instead.
            None => {
                return Err(PlanError::UnmappableArgument {
                    capability_id: cap.id,
                    argument: "baseline artifact",
                })
            }
        }
    }
    if let Some(w) = &args.wasm {
        match flag_for_any(cap, CANDIDATE_FLAGS) {
            Some(flag) => {
                artifacts.push((arg_position(cap, flag), vec![flag.to_string(), w.clone()]))
            }
            // Positional artifact (e.g. `wasm inspect <file.wasm>`).
            None => artifacts.push((arg_position(cap, "<file.wasm>"), vec![w.clone()])),
        }
    }
    artifacts.sort_by_key(|(position, _)| *position);
    for (_, words) in artifacts {
        argv.extend(words);
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

/// Flag spellings that can carry the *baseline* (previously deployed)
/// artifact, in the order they are preferred. Which one applies is decided by
/// the registry, not by the request text.
const BASELINE_FLAGS: &[&str] = &["--previous-wasm", "--old-wasm"];

/// Flag spellings that can carry the *candidate* (newly built) artifact.
const CANDIDATE_FLAGS: &[&str] = &["--wasm", "--new-wasm"];

/// True when the registry lists this flag for the capability.
fn flag_for(cap: &Capability, flag: &str) -> bool {
    let in_required = cap.required_args.iter().any(|a| a.contains(flag));
    let in_optional = cap.optional_args.iter().any(|a| a.contains(flag));
    in_required || in_optional
}

/// First flag in `candidates` that the registry declares for this capability.
///
/// Matching is exact on the flag name so a longer flag cannot be mistaken for
/// a shorter one: `--wasm` must not satisfy a lookup for `--previous-wasm`,
/// which contains it as a substring.
fn flag_for_any<'a>(cap: &Capability, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .find(|flag| declares_arg(cap, flag))
        .copied()
}

/// Position of an argument in the registry's declaration order, used only to
/// emit a capability's artifact flags in that same order.
fn arg_position(cap: &Capability, flag: &str) -> usize {
    cap.required_args
        .iter()
        .chain(cap.optional_args)
        .position(|a| a.split_whitespace().next().unwrap_or_default() == flag)
        .unwrap_or(usize::MAX)
}

/// Whether the registry declares `flag` as an argument of its own.
fn declares_arg(cap: &Capability, flag: &str) -> bool {
    // An entry looks like `--wasm <f>` or `--rpc-url`; compare the leading
    // flag token only, so `<f>` placeholders and argument names never match.
    let declared = |a: &&str| a.split_whitespace().next().unwrap_or_default() == flag;
    cap.required_args.iter().any(declared) || cap.optional_args.iter().any(declared)
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
    fn release_assurance_maps_both_artifacts_to_their_own_flags() {
        // Regression: the candidate was emitted as a positional argument and
        // the baseline took `--wasm`, so sdkt rejected the line with exit 2.
        let p = plan_ok("release assurance for candidate.wasm previous baseline.wasm");
        assert_eq!(p.capability_id, "release_assurance");
        let head: Vec<&str> = p.argv.iter().take(4).map(String::as_str).collect();
        assert_eq!(
            head,
            vec![
                "release-assurance",
                "--wasm",
                "candidate.wasm",
                "--previous-wasm"
            ],
            "argv was {:?}",
            p.argv
        );
        assert_eq!(p.argv.get(4).map(String::as_str), Some("baseline.wasm"));
        // The invariant is about values, not flags: no artifact may sit in the
        // positional slot right after the subcommand — that is exactly the
        // shape clap rejected.
        let positional = &p.argv[1];
        assert!(
            positional.starts_with("--"),
            "nothing may follow the subcommand positionally, found {positional:?} in {:?}",
            p.argv
        );
        for (flag, value) in [
            ("--wasm", "candidate.wasm"),
            ("--previous-wasm", "baseline.wasm"),
        ] {
            let i = p.argv.iter().position(|a| a == flag).expect("flag present");
            assert_eq!(p.argv[i + 1], value, "{flag} must be followed by {value}");
        }
    }

    #[test]
    fn release_assurance_role_order_does_not_depend_on_phrasing() {
        // "NEW previous OLD" and "OLD then NEW" must yield the same roles.
        let a = plan_ok("release assurance for candidate.wasm previous baseline.wasm");
        let b = plan_ok("release assurance for baseline.wasm candidate.wasm");
        let pick = |p: &Plan, flag: &str| {
            let i = p.argv.iter().position(|a| a == flag).expect("flag present");
            p.argv[i + 1].clone()
        };
        assert_eq!(pick(&a, "--wasm"), "candidate.wasm");
        assert_eq!(pick(&a, "--previous-wasm"), "baseline.wasm");
        assert_eq!(pick(&b, "--wasm"), "candidate.wasm");
        assert_eq!(pick(&b, "--previous-wasm"), "baseline.wasm");
    }

    #[test]
    fn diff_keeps_its_own_flag_spelling_and_order() {
        // The registry-driven mapping must not disturb the capability that
        // spells its pair differently.
        let p = plan_ok("check upgrade safety from old.wasm to new.wasm");
        assert_eq!(p.capability_id, "diff.upgrade_safety");
        assert_eq!(
            &p.argv[..6],
            &[
                "diff",
                "--upgrade-safety",
                "--old-wasm",
                "old.wasm",
                "--new-wasm",
                "new.wasm"
            ],
            "{:?}",
            p.argv
        );
    }

    #[test]
    fn baseline_argument_is_refused_when_the_capability_has_none() {
        // `verify` takes one artifact. A second path must not be emitted
        // positionally — that is the shape of the bug being fixed.
        let e = plan_err(&format!(
            "verify contract {CID} against candidate.wasm previous baseline.wasm"
        ));
        match e {
            PlanError::UnmappableArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "verify");
                assert!(argument.contains("baseline"));
            }
            other => panic!("expected UnmappableArgument, got {other:?}"),
        }
    }

    #[test]
    fn flag_lookup_is_exact_not_substring() {
        // `--previous-wasm` contains `--wasm`; a substring lookup would let the
        // candidate claim the baseline's flag.
        let ra = sdkt_core::registry::find("release_assurance").expect("in registry");
        assert!(declares_arg(ra, "--wasm"));
        assert!(declares_arg(ra, "--previous-wasm"));
        assert!(!declares_arg(ra, "--old-wasm"));
        let d = sdkt_core::registry::find("diff").expect("in registry");
        assert!(declares_arg(d, "--old-wasm"));
        assert!(!declares_arg(d, "--previous-wasm"));
        assert!(!declares_arg(d, "--wasm"));
    }

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
