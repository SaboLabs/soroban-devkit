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
//! - JSON output is only requested when the registry says it exists;
//! - every argument the request named is either mapped into argv or the
//!   request is refused whole — nothing is dropped silently, so a partial
//!   request can never run and report success as if it were the complete one.

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
    /// Whether JSON output was requested (registry said it exists).
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

/// Flag spellings that can carry the *baseline* (previously deployed)
/// artifact, in the order they are preferred. Which one applies is decided by
/// the registry, not by the request text.
const BASELINE_FLAGS: &[&str] = &["--previous-wasm", "--old-wasm"];

/// Flag spellings that can carry the *candidate* (newly built) artifact.
const CANDIDATE_FLAGS: &[&str] = &["--wasm", "--new-wasm"];

/// Flags whose *presence* the planner itself interprets, mapped into argv
/// with values from [`crate::intent::IntentArgs`]. A request naming one of
/// these is not "unsupported" merely because the capability spells its
/// artifact flags differently; but capabilities that cannot use the value at
/// all (e.g. `inspect --rpc-url` for an offline path) still hit the registry
/// declaration check.
const PLANNER_MANAGED_FLAGS: &[&str] = &[
    "--rpc-url",
    "--network-passphrase",
    "--format",
    "--wasm",
    "--previous-wasm",
    "--old-wasm",
    "--new-wasm",
    "--envelope",
    "--args",
    "--base-fees",
    "--rpc",
];

/// Number of artifact paths the capability can accept, derived from the
/// registry argument lists rather than a hand-maintained capability table.
fn artifact_capacity(cap: &Capability) -> usize {
    if BASELINE_FLAGS.iter().any(|f| declares_arg(cap, f)) {
        2
    } else if CANDIDATE_FLAGS.iter().any(|f| declares_arg(cap, f))
        || declares_arg(cap, "<file.wasm>")
    {
        1
    } else {
        0
    }
}

/// Build the argv for a capability.
///
/// Fails with [`PlanError`] rather than emitting a partial command line, so
/// a refused network or a dropped argument can never leak into an executed
/// argv.
fn build_argv(cap: &Capability, args: &intent::IntentArgs) -> Result<Vec<String>, PlanError> {
    // ---- Nothing the user asked for may vanish silently. ----
    // Every artifact path must fit the capability's declared capacity; the
    // parser already refused unmarked two-artifact role guesses, and a third
    // path here is refused instead of dropped.
    if !args.artifact_paths.is_empty() && args.artifact_paths.len() > artifact_capacity(cap) {
        return Err(PlanError::Parse(ParseError::UnsupportedArgument {
            capability_id: cap.id,
            arguments: format!(
                "artifact path(s) {} — {} accepts at most {}",
                args.artifact_paths.join(", "),
                cap.id,
                artifact_capacity(cap)
            ),
        }));
    }
    // Every explicit flag must be one the planner maps *and* the registry
    // declares for this capability; anything else refuses the whole request.
    for (flag, _) in &args.explicit_flags {
        let managed = PLANNER_MANAGED_FLAGS.contains(&flag.as_str());
        if (managed && flag_for(cap, flag)) || declares_arg(cap, flag) {
            continue;
        }
        return Err(PlanError::Parse(ParseError::UnsupportedArgument {
            capability_id: cap.id,
            arguments: flag.clone(),
        }));
    }

    // ---- Network policy: refuse a named production network before
    // building anything; pin an explicit endpoint when one applies. ----
    if cap.network != NetworkRequirement::None {
        match args.network.as_deref() {
            // Absent or testnet: the endpoint is pinned explicitly below.
            None | Some("testnet") => {}
            // Any other named network (mainnet, futurenet, custom) is refused
            // outright rather than guessed at.
            Some(other) => {
                return Err(PlanError::RefusedNetwork(other.to_string()));
            }
        }
    }
    // An RPC endpoint the request supplied is honoured only when the
    // capability is network-touching and declares `--rpc-url`; otherwise it
    // is an argument with nowhere to go — refuse, never drop.
    if args.rpc_url.is_some()
        && (cap.network == NetworkRequirement::None || !flag_for(cap, "--rpc-url"))
    {
        return Err(PlanError::Parse(ParseError::UnsupportedArgument {
            capability_id: cap.id,
            arguments: format!("--rpc-url {}", args.rpc_url.as_deref().unwrap_or("")),
        }));
    }
    if args.network_passphrase.is_some()
        && (cap.network == NetworkRequirement::None || !flag_for(cap, "--network-passphrase"))
    {
        return Err(PlanError::Parse(ParseError::UnsupportedArgument {
            capability_id: cap.id,
            arguments: "--network-passphrase".to_string(),
        }));
    }
    // A request that names its own endpoint must also name its passphrase:
    // pairing a foreign URL with the testnet passphrase would query the
    // wrong network under a plausible-looking flag set.
    if cap.network != NetworkRequirement::None
        && flag_for(cap, "--rpc-url")
        && args.rpc_url.is_some()
        && args.rpc_url.as_deref() != Some(TESTNET_RPC)
        && args.network_passphrase.is_none()
    {
        return Err(PlanError::Parse(ParseError::MissingArgument {
            capability_id: cap.id,
            argument: "network passphrase (--network-passphrase) for a non-testnet RPC url",
        }));
    }

    let mut argv: Vec<String> = cap.command.iter().map(|s| (*s).to_string()).collect();

    // Flags the registry declares for this capability but the planner does
    // not map itself (`--audit <path>`, `--max-size-bytes <N>`, `--key-xdr`):
    // passed through verbatim, with the value the request supplied. A
    // declared flag with no value is refused rather than emitted bare, which
    // the CLI would answer with a usage error.
    for (flag, value) in &args.explicit_flags {
        if PLANNER_MANAGED_FLAGS.contains(&flag.as_str()) {
            continue;
        }
        if !declares_arg(cap, flag) {
            continue; // refused by the gate above; unreachable here
        }
        match value {
            Some(v) => {
                argv.push(flag.clone());
                argv.push(v.clone());
            }
            None => {
                return Err(PlanError::Parse(ParseError::MissingArgument {
                    capability_id: cap.id,
                    argument: Box::leak(format!("{flag} <value>").into_boxed_str()),
                }))
            }
        }
    }

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
    // the capability's argument lists rather than fixed to one spelling.
    // Emit the pair in the order the registry declares them: `diff` lists
    // `--old-wasm` first, so the baseline leads; `release-assurance` lists
    // the required `--wasm` first, so the candidate leads.
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
    // Transaction envelope: the registry declares it as `--envelope <xdr>`
    // for `tx simulate`; emit exactly that flag, never a positional clap
    // would reject.
    if let Some(e) = &args.envelope {
        if !flag_for(cap, "--envelope") {
            return Err(PlanError::UnmappableArgument {
                capability_id: cap.id,
                argument: "transaction envelope",
            });
        }
        argv.push("--envelope".to_string());
        argv.push(e.clone());
    }
    // Typed call arguments: passed through exactly as the CLI takes them.
    if !args.typed_args.is_empty() {
        if !flag_for(cap, "--args") {
            return Err(PlanError::UnmappableArgument {
                capability_id: cap.id,
                argument: "typed call arguments",
            });
        }
        for t in &args.typed_args {
            argv.push("--args".to_string());
            argv.push(t.clone());
        }
    }
    // Offline fee samples: the user-supplied value passes through verbatim.
    if let Some(bf) = &args.base_fees {
        if !flag_for(cap, "--base-fees") {
            return Err(PlanError::UnmappableArgument {
                capability_id: cap.id,
                argument: "--base-fees",
            });
        }
        argv.push("--base-fees".to_string());
        argv.push(bf.clone());
    }
    // Live RPC fee statistics. The switch is emitted when the request
    // spelled `--rpc`, and for `fee estimate` when the request named an RPC
    // endpoint with no `--base-fees`: "estimate the fee using <url>" has
    // exactly one CLI meaning (fetch fee stats from that RPC), so the flag
    // maps the request's own words — it invents no value.
    let live_fee = cap.id == "fee.estimate"
        && args.rpc_url.is_some()
        && args.base_fees.is_none()
        && !args.use_rpc;
    if args.use_rpc || live_fee {
        if !flag_for(cap, "--rpc") {
            return Err(PlanError::UnmappableArgument {
                capability_id: cap.id,
                argument: "--rpc",
            });
        }
        argv.push("--rpc".to_string());
    }

    // Pin an explicit testnet endpoint only for capabilities that actually
    // accept `--rpc-url` (the registry is the source of that fact). Some
    // commands, e.g. `network check`, resolve a saved profile instead and
    // reject the flag. `fee estimate --base-fees` is fully offline and gets
    // no endpoint. A request-supplied URL (validated above) wins over the
    // pinned default.
    if cap.network != NetworkRequirement::None && flag_for(cap, "--rpc-url") {
        let offline_fee = cap.id == "fee.estimate" && args.base_fees.is_some() && !args.use_rpc;
        if !offline_fee {
            let rpc = args
                .rpc_url
                .clone()
                .unwrap_or_else(|| TESTNET_RPC.to_string());
            argv.push("--rpc-url".to_string());
            argv.push(rpc);
            let phrase = args.network_passphrase.clone().unwrap_or_else(|| {
                // Default passphrase is only correct for the default
                // endpoint; a foreign URL without one was refused above.
                TESTNET_PASSPHRASE.to_string()
            });
            argv.push("--network-passphrase".to_string());
            argv.push(phrase);
        }
    }

    // A `--format` the parser could not capture a value for (`--format`
    // followed by prose) must refuse rather than default to json: the user
    // asked for a format and dropping the word silently would change the
    // request.
    if args.has_flag("--format") && args.flag_value("--format").is_none() {
        return Err(PlanError::Parse(ParseError::UnsupportedArgument {
            capability_id: cap.id,
            arguments: "--format <json|pretty>".to_string(),
        }));
    }
    // JSON output: request it when the registry says it exists, unless the
    // request explicitly asked for pretty. A `--format` value the registry
    // cannot back is refused here rather than dropped, so `--format yaml`
    // never runs as a silent `--format json`.
    let asked = args.flag_value("--format");
    if let Some(v) = asked {
        let ok = (v == "json" && cap.formats.json) || (v == "pretty" && cap.formats.pretty);
        if !ok {
            return Err(PlanError::Parse(ParseError::UnsupportedArgument {
                capability_id: cap.id,
                arguments: format!("--format {v}"),
            }));
        }
    }
    if cap.formats.json && asked != Some("pretty") {
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
        // "NEW previous OLD" and "candidate NEW baseline OLD" must agree.
        let a = plan_ok("release assurance for candidate.wasm previous baseline.wasm");
        let b = plan_ok("release assurance for candidate new.wasm previous old.wasm");
        let pick = |p: &Plan, flag: &str| {
            let i = p.argv.iter().position(|a| a == flag).expect("flag present");
            p.argv[i + 1].clone()
        };
        assert_eq!(pick(&a, "--wasm"), "candidate.wasm");
        assert_eq!(pick(&a, "--previous-wasm"), "baseline.wasm");
        assert_eq!(pick(&b, "--wasm"), "new.wasm");
        assert_eq!(pick(&b, "--previous-wasm"), "old.wasm");
    }

    #[test]
    fn release_assurance_refuses_unmarked_pair() {
        // P2-1 regression: listing order is not role information. The old
        // planner guessed "first path is the baseline", which inverts when
        // the user lists candidate first. Nothing may be built or executed.
        let e = plan_err("release assurance for newbuild.wasm snapshot.wasm");
        assert_eq!(e.code(), "ambiguous_artifact_roles", "{e:?}");
        let e = plan_err("release assurance for snapshot.wasm newbuild.wasm");
        assert_eq!(e.code(), "ambiguous_artifact_roles", "{e:?}");
    }

    #[test]
    fn release_assurance_flag_spelled_artifacts_map_by_flag() {
        let p = plan_ok("release assurance --wasm cand.wasm --previous-wasm base.wasm");
        let i = p.argv.iter().position(|a| a == "--wasm").unwrap();
        assert_eq!(p.argv[i + 1], "cand.wasm");
        let j = p.argv.iter().position(|a| a == "--previous-wasm").unwrap();
        assert_eq!(p.argv[j + 1], "base.wasm");
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
    fn diff_refuses_unmarked_pair() {
        // Same invariant as release assurance: direction must be explicit.
        let e = plan_err("diff two artifacts a.wasm and b.wasm");
        assert_eq!(e.code(), "ambiguous_artifact_roles", "{e:?}");
    }

    #[test]
    fn baseline_argument_is_refused_when_the_capability_has_none() {
        // `verify` takes one artifact. A second path must not be emitted
        // positionally — that is the shape of the bug being fixed.
        let e = plan_err(&format!(
            "verify contract {CID} against candidate.wasm previous baseline.wasm"
        ));
        match e {
            PlanError::Parse(ParseError::UnsupportedArgument {
                capability_id,
                arguments,
            }) => {
                assert_eq!(capability_id, "verify");
                assert!(arguments.contains("artifact"), "{arguments}");
            }
            other => panic!("expected UnsupportedArgument, got {other:?}"),
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

    // ------------------------------------------------------------ P2-2 ---

    #[test]
    fn unsupported_flag_refuses_rather_than_dropping() {
        // Regression: `inspect` cannot express a size-policy flag. The old
        // planner ignored the token and ran a request the user did not make.
        let e = plan_err(&format!("inspect contract {CID} --max-growth-pct 50"));
        assert_eq!(e.code(), "unsupported_argument", "{e:?}");
        match e {
            PlanError::Parse(ParseError::UnsupportedArgument {
                capability_id,
                arguments,
            }) => {
                assert_eq!(capability_id, "inspect");
                assert_eq!(arguments, "--max-growth-pct");
            }
            other => panic!("expected UnsupportedArgument, got {other:?}"),
        }
    }

    #[test]
    fn supported_policy_flag_still_maps_to_argv() {
        // The same flag IS declared for release-assurance: it must plan.
        // The agent passes registry-declared flags through when the registry
        // declares them; the value ride-along keeps argv honest.
        let p = plan_ok(
            "release assurance for candidate new.wasm previous old.wasm --audit src/lib.rs --max-size-bytes 131072",
        );
        let shown = p.display();
        assert!(shown.contains("--audit src/lib.rs"), "{shown}");
        assert!(shown.contains("--max-size-bytes 131072"), "{shown}");
    }

    #[test]
    fn extra_artifact_refuses_rather_than_dropping() {
        // wasm inspect takes exactly one artifact.
        let e = plan_err("inspect wasm a.wasm b.wasm");
        assert_eq!(e.code(), "unsupported_argument", "{e:?}");
        // Release assurance with three artifacts: two roles, one leftover.
        let e = plan_err("release assurance for candidate a.wasm previous b.wasm extra c.wasm");
        assert_eq!(e.code(), "unsupported_argument", "{e:?}");
    }

    #[test]
    fn request_supplied_rpc_url_passes_through() {
        let p = plan_ok(&format!(
            "inspect contract {CID} --rpc-url https://soroban-testnet.stellar.org"
        ));
        let i = p.argv.iter().position(|a| a == "--rpc-url").unwrap();
        assert_eq!(p.argv[i + 1], "https://soroban-testnet.stellar.org");
        let j = p
            .argv
            .iter()
            .position(|a| a == "--network-passphrase")
            .unwrap();
        assert_eq!(p.argv[j + 1], TESTNET_PASSPHRASE);
    }

    #[test]
    fn non_testnet_rpc_url_requires_an_explicit_passphrase() {
        // Never pair a foreign endpoint with the testnet passphrase.
        let e = plan_err(&format!(
            "inspect contract {CID} --rpc-url https://my.rpc.example"
        ));
        assert_eq!(e.code(), "missing_argument", "{e:?}");
        // Quoted value: a passphrase with spaces is delimited by quotes.
        let p = plan_ok(&format!(
            "inspect contract {CID} --rpc-url https://my.rpc.example --network-passphrase \"Custom Net ; 2026\""
        ));
        let i = p.argv.iter().position(|a| a == "--rpc-url").unwrap();
        assert_eq!(p.argv[i + 1], "https://my.rpc.example");
        let j = p
            .argv
            .iter()
            .position(|a| a == "--network-passphrase")
            .unwrap();
        assert_eq!(p.argv[j + 1], "Custom Net ; 2026");
        // An *unquoted* multi-word passphrase is never guessed: the value
        // cannot be delimited, so the request clarifies instead of running.
        let e = plan_err(&format!(
            "inspect contract {CID} --rpc-url https://my.rpc.example --network-passphrase Custom Net ; 2026"
        ));
        assert_eq!(e.code(), "missing_argument", "{e:?}");
    }

    #[test]
    fn explicit_format_json_is_not_doubled() {
        let p = plan_ok("inspect wasm a.wasm --format json");
        let n = p.argv.iter().filter(|a| a.as_str() == "--format").count();
        assert_eq!(n, 1, "{:?}", p.argv);
        let i = p.argv.iter().position(|a| a == "--format").unwrap();
        assert_eq!(p.argv[i + 1], "json");
    }

    #[test]
    fn unsupported_format_value_is_refused_not_dropped() {
        // `--format yaml` would otherwise vanish and the agent would emit
        // `--format json` on its own — a different request than the user made.
        let e = plan_err("inspect wasm a.wasm --format yaml");
        assert_eq!(e.code(), "unsupported_argument", "{e:?}");
    }

    #[test]
    fn pretty_format_suppresses_the_json_default() {
        let p = plan_ok("inspect wasm a.wasm --format pretty");
        assert!(!p.argv.contains(&"--format".to_string()), "{:?}", p.argv);
    }

    #[test]
    fn rpc_url_on_a_capability_without_the_flag_refuses() {
        // network.check resolves a saved profile and rejects --rpc-url; a
        // request naming both must clarify, not run half the ask.
        let e = plan_err(
            "check network status profile wl-testnet --rpc-url https://soroban-testnet.stellar.org",
        );
        assert_eq!(e.code(), "unsupported_argument", "{e:?}");
    }

    // ------------------------------------------------------------ P2-3 ---

    #[test]
    fn network_list_plans_registry_argv() {
        let p = plan_ok("list networks");
        assert_eq!(p.capability_id, "network.list");
        assert_eq!(p.argv, vec!["network", "list", "--format", "json"]);
    }

    #[test]
    fn project_status_plans_with_pinned_testnet() {
        let p = plan_ok("show project status");
        assert_eq!(p.capability_id, "project.status");
        let shown = p.display();
        assert!(shown.starts_with("sdkt project status"), "{shown}");
        assert!(
            shown.contains("--rpc-url https://soroban-testnet.stellar.org"),
            "{shown}"
        );
    }

    #[test]
    fn fee_estimate_rpc_plans_live_source() {
        let p = plan_ok("estimate the fee using https://soroban-testnet.stellar.org --rpc");
        assert_eq!(p.capability_id, "fee.estimate");
        assert!(p.argv.contains(&"--rpc".to_string()), "{:?}", p.argv);
        let i = p.argv.iter().position(|a| a == "--rpc-url").unwrap();
        assert_eq!(p.argv[i + 1], "https://soroban-testnet.stellar.org");
    }

    #[test]
    fn fee_estimate_base_fees_is_offline() {
        let p = plan_ok("estimate the fee --base-fees 100,120,110");
        let i = p.argv.iter().position(|a| a == "--base-fees").unwrap();
        assert_eq!(p.argv[i + 1], "100,120,110");
        assert!(!p.argv.contains(&"--rpc-url".to_string()), "{:?}", p.argv);
    }

    #[test]
    fn fee_estimate_without_source_clarifies() {
        let e = plan_err("estimate the fee");
        assert_eq!(e.code(), "missing_argument", "{e:?}");
    }

    #[test]
    fn tx_simulate_plans_envelope() {
        let p = plan_ok("simulate transaction --envelope tx.xdr");
        assert_eq!(p.capability_id, "tx.simulate");
        let i = p.argv.iter().position(|a| a == "--envelope").unwrap();
        assert_eq!(p.argv[i + 1], "tx.xdr");
        assert!(p
            .display()
            .contains("--rpc-url https://soroban-testnet.stellar.org"));
    }

    #[test]
    fn tx_simulate_without_envelope_clarifies() {
        let e = plan_err("simulate this transaction");
        assert_eq!(e.code(), "missing_argument", "{e:?}");
    }

    #[test]
    fn call_plans_contract_function_and_typed_args() {
        let p = plan_ok(&format!(
            "call function balance on contract {CID} --args u32:7"
        ));
        assert_eq!(p.capability_id, "call");
        assert_eq!(p.argv[0], "call");
        assert!(p.argv.contains(&CID.to_string()), "{:?}", p.argv);
        assert!(p.argv.contains(&"balance".to_string()), "{:?}", p.argv);
        let i = p.argv.iter().position(|a| a == "--args").unwrap();
        assert_eq!(p.argv[i + 1], "u32:7");
    }
}
