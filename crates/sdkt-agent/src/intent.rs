//! Deterministic request parser: keywords in, [`Intent`] out.
//!
//! No model, no fuzzy scoring — a request maps to an intent only when its
//! keywords and arguments are unambiguous. Anything else is a
//! [`ParseError`], which the planner turns into `needs_clarification` without
//! running a command.
//!
//! The parser deliberately prefers *refusing* over *guessing*: a bare
//! "check the contract" has no contract id, so it cannot be planned, and a
//! request naming two different operations (say `health` and `verify`) is
//! ambiguous rather than arbitrarily resolved.

use sdkt_core::registry::Capability;

/// What the user appears to be asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    /// Registry capability id this request maps to.
    pub capability_id: &'static str,
    /// Arguments extracted from the request, keyed by canonical name.
    pub args: IntentArgs,
    /// The words that selected this intent, for the explanation.
    pub matched_keywords: Vec<String>,
}

/// Arguments the parser can recognise in free text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntentArgs {
    /// A `C...` contract id.
    pub contract: Option<String>,
    /// Path to a local `.wasm` file.
    pub wasm: Option<String>,
    /// A second local `.wasm`, used as the upgrade baseline.
    pub previous_wasm: Option<String>,
    /// A `.rs` path or directory for the audit.
    pub source_path: Option<String>,
    /// A contract function name (for `call`).
    pub function: Option<String>,
    /// A saved network profile name (for `network check`).
    pub profile: Option<String>,
    /// Network named in the request, normalised lowercase.
    pub network: Option<String>,
}

/// Why a request could not be turned into an intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing in the request matched a known operation.
    Unrecognised(String),
    /// Two different operations were named.
    Ambiguous {
        /// Capability ids that matched.
        candidates: Vec<&'static str>,
    },
    /// The request names an operation that v0.1 refuses to run.
    MutationRequested {
        /// The registry capability that was refused.
        capability_id: &'static str,
    },
    /// The operation was identified but a required argument is missing.
    MissingArgument {
        /// Capability id.
        capability_id: &'static str,
        /// Canonical name of the argument that is missing.
        argument: &'static str,
    },
}

impl ParseError {
    /// Short machine-stable code for the result document.
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::Unrecognised(_) => "unrecognised_request",
            ParseError::Ambiguous { .. } => "ambiguous_request",
            ParseError::MutationRequested { .. } => "mutation_requested",
            ParseError::MissingArgument { .. } => "missing_argument",
        }
    }

    /// Human-readable reason, safe to show the user.
    pub fn message(&self) -> String {
        match self {
            ParseError::Unrecognised(r) => format!(
                "could not map {r:?} to a known read-only capability; rephrase with an explicit operation"
            ),
            ParseError::Ambiguous { candidates } => format!(
                "request names more than one operation ({}) — ask for one at a time",
                candidates.join(", ")
            ),
            ParseError::MutationRequested { capability_id } => format!(
                "{capability_id} changes ledger or local state; the v0.1 agent is read-only and will not run it"
            ),
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => format!("{capability_id} needs {argument}, which the request did not provide"),
        }
    }
}

/// Operations the planner knows how to recognise, in match order.
///
/// Each entry lists the keyword sets that select it. An entry matches when
/// **any** of its sets is fully present in the request, which is what lets
/// "upgrade safety" and "safe to upgrade" both work.
const INTENT_KEYWORDS: &[(&str, &[&[&str]])] = &[
    // Mutating intents are recognised explicitly so the planner can block
    // them with a precise reason instead of calling them unrecognised.
    ("deploy", &[&["deploy"]]),
    ("invoke", &[&["invoke"], &["call", "and", "send"]]),
    (
        "tx.submit",
        &[&["submit", "transaction"], &["submit", "tx"]],
    ),
    (
        "tx.sign",
        &[&["sign", "transaction"], &["sign", "envelope"]],
    ),
    (
        "storage.extend",
        &[&["extend", "ttl"], &["extend", "storage"]],
    ),
    (
        "storage.restore",
        &[&["restore", "storage"], &["restore", "archived"]],
    ),
    ("project.deploy", &[&["deploy", "project"]]),
    ("identity.fund", &[&["fund", "account"], &["friendbot"]]),
    // Read-only intents.
    (
        "release_assurance",
        &[&["release", "assurance"], &["release", "check"]],
    ),
    (
        "diff.upgrade_safety",
        &[
            &["upgrade", "safety"],
            &["safe", "to", "upgrade"],
            &["breaking", "change"],
        ],
    ),
    ("verify", &[&["verify"]]),
    ("health", &[&["health"]]),
    (
        "storage.analyze",
        &[&["storage", "analyze"], &["storage", "layout"]],
    ),
    ("events", &[&["events"]]),
    ("audit", &[&["audit"], &["security", "scan"]]),
    (
        "network.check",
        &[&["network", "check"], &["network", "status"]],
    ),
    ("doctor", &[&["doctor"], &["environment", "check"]]),
    ("wasm.metadata", &[&["metadata"]]),
    (
        "wasm.inspect",
        &[&["inspect", "wasm"], &["wasm", "inspect"]],
    ),
    ("inspect", &[&["inspect", "contract"], &["inspect"]]),
    (
        "call",
        &[
            &["call", "function"],
            &["read", "function"],
            &["invoke", "read"],
        ],
    ),
    ("diff", &[&["diff"]]),
];

/// Parse a natural-language request into an intent.
pub fn parse(request: &str) -> Result<Intent, ParseError> {
    let lower = request.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '/' && c != '-')
        .filter(|w| !w.is_empty())
        .collect();
    let args = extract_args(request);

    // Collect every intent whose keyword set is fully present.
    let mut matched: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (id, sets) in INTENT_KEYWORDS {
        for set in *sets {
            if set.iter().all(|kw| words.contains(kw)) {
                let keywords = set.iter().map(|s| (*s).to_string()).collect();
                if !matched.iter().any(|(m, _)| m == id) {
                    matched.push((id, keywords));
                }
                break;
            }
        }
    }

    if matched.is_empty() {
        return Err(ParseError::Unrecognised(request.trim().to_string()));
    }

    // Specificity: when one rule's matched keywords are a strict subset of
    // another's, the broader rule is the same request described less
    // precisely — drop it rather than reporting a false ambiguity
    // ("inspect wasm f.wasm" is wasm.inspect, not "wasm.inspect OR inspect").
    let snapshot = matched.clone();
    matched.retain(|(id, kws)| {
        !snapshot.iter().any(|(other_id, other_kws)| {
            other_id != id
                && other_kws.len() > kws.len()
                && kws.iter().all(|k| other_kws.contains(k))
        })
    });

    // A mutating capability that was named explicitly is reported as such —
    // but only when it is the *only* thing named, otherwise the request is
    // ambiguous and we say so rather than picking one.
    let mutating: Vec<&'static str> = matched
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| is_mutating(id))
        .collect();
    let readonly: Vec<&'static str> = matched
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !is_mutating(id))
        .collect();

    // Prefer the read-only reading when a mutating word appears only as part
    // of a read-only phrase ("safe to upgrade" must not match `deploy`).
    if !readonly.is_empty() {
        if readonly.len() > 1 {
            return Err(ParseError::Ambiguous {
                candidates: readonly,
            });
        }
        let id = readonly[0];
        let keywords = matched
            .iter()
            .find(|(m, _)| *m == id)
            .map(|(_, k)| k.clone())
            .unwrap_or_default();
        validate_args(id, &args)?;
        return Ok(Intent {
            capability_id: id,
            args,
            matched_keywords: keywords,
        });
    }

    if mutating.len() == 1 {
        return Err(ParseError::MutationRequested {
            capability_id: mutating[0],
        });
    }
    Err(ParseError::Ambiguous {
        candidates: mutating,
    })
}

/// Whether the registry marks this capability as changing state.
fn is_mutating(id: &str) -> bool {
    use sdkt_core::registry::Safety;
    sdkt_core::registry::find(id).is_some_and(|c| c.safety == Safety::Mutating)
}

/// Check that the extracted arguments satisfy the capability's requirements.
fn validate_args(id: &'static str, args: &IntentArgs) -> Result<(), ParseError> {
    let needs_contract = matches!(
        id,
        "inspect" | "health" | "verify" | "wasm.metadata" | "storage.analyze" | "events" | "call"
    );
    if needs_contract && args.contract.is_none() {
        return Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "contract id (C...)",
        });
    }
    match id {
        "verify" if args.wasm.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "local wasm path",
        }),
        "diff.upgrade_safety" | "diff" => match (&args.previous_wasm, &args.wasm) {
            (Some(_), Some(_)) => Ok(()),
            (None, _) => Err(ParseError::MissingArgument {
                capability_id: id,
                argument: "baseline wasm (the 'from' artifact)",
            }),
            (_, None) => Err(ParseError::MissingArgument {
                capability_id: id,
                argument: "candidate wasm (the 'to' artifact)",
            }),
        },
        "audit" if args.source_path.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "a .rs source path",
        }),
        "release_assurance" if args.wasm.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "candidate wasm path",
        }),
        "call" if args.function.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "function name",
        }),
        // `network check <NAME>` resolves a *saved* profile and has no
        // --rpc-url flag, so it carries no endpoint the agent must pin.
        "network.check" if args.profile.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "a saved profile name",
        }),
        "network.check" => Ok(()),
        _ => Ok(()),
    }
}

/// Pull recognisable arguments out of the raw request text.
///
/// Contract ids are found by shape (`C` + 55 base32 chars), wasm paths by
/// `.wasm` suffix, source paths by `.rs` or a trailing directory-ish token.
fn extract_args(request: &str) -> IntentArgs {
    let mut args = IntentArgs::default();

    // Tokens split on whitespace, keeping punctuation for path detection.
    let raw_tokens: Vec<&str> = request.split_whitespace().collect();
    let mut wasm_paths: Vec<String> = Vec::new();
    let profile = extract_profile(&raw_tokens);

    for token in &raw_tokens {
        let cleaned = token.trim_matches(|c: char| {
            c == ',' || c == ';' || c == '"' || c == '\'' || c == '(' || c == ')' || c == '.'
        });
        let lower = cleaned.to_ascii_lowercase();

        if is_contract_id(cleaned) {
            args.contract = Some(cleaned.to_string());
            continue;
        }
        if lower.ends_with(".wasm") {
            wasm_paths.push(cleaned.to_string());
            continue;
        }
        if lower.ends_with(".rs") {
            args.source_path = Some(cleaned.to_string());
            continue;
        }
        if matches!(lower.as_str(), "testnet" | "mainnet" | "futurenet") {
            args.network = Some(lower);
        }
    }

    // Two wasm paths: the earlier one is the baseline when the request reads
    // "from A to B" (the natural phrasing for an upgrade).
    match wasm_paths.len() {
        0 => {}
        1 => args.wasm = Some(wasm_paths.remove(0)),
        _ => {
            let from = find_from_index(&raw_tokens);
            if let Some(from_idx) = from {
                // Only a "to" *after* the "from" marks the destination; an
                // earlier one belongs to phrasing like "safe to upgrade".
                let to_idx = raw_tokens
                    .iter()
                    .enumerate()
                    .skip(from_idx + 1)
                    .find(|(_, t)| t.eq_ignore_ascii_case("to") || t.eq_ignore_ascii_case("new"))
                    .map(|(i, _)| i)
                    .unwrap_or(usize::MAX);
                if from_idx < to_idx {
                    args.previous_wasm = Some(wasm_paths[0].clone());
                    args.wasm = Some(wasm_paths[1].clone());
                } else {
                    args.wasm = Some(wasm_paths[0].clone());
                    args.previous_wasm = Some(wasm_paths[1].clone());
                }
            } else {
                // Without an explicit direction, the first path is the
                // baseline — the `--old-wasm --new-wasm` order.
                args.previous_wasm = Some(wasm_paths[0].clone());
                args.wasm = Some(wasm_paths[1].clone());
            }
        }
    }

    args.profile = profile;
    args
}

/// A profile name follows the literal word `profile` ("… profile wl-testnet").
/// Anything else is refused rather than guessed, since picking the wrong
/// saved profile means querying the wrong network.
fn extract_profile(tokens: &[&str]) -> Option<String> {
    let at = tokens
        .iter()
        .position(|t| t.eq_ignore_ascii_case("profile"))?;
    let next = *tokens.get(at + 1)?;
    if next.is_empty() || next.starts_with('-') {
        return None;
    }
    Some(
        next.trim_matches(|c: char| c == ',' || c == ';' || c == '"' || c == '\'' || c == ')')
            .to_string(),
    )
}

fn find_from_index(tokens: &[&str]) -> Option<usize> {
    tokens
        .iter()
        .position(|t| t.eq_ignore_ascii_case("from") || t.eq_ignore_ascii_case("old"))
}

/// `C` followed by 55 base32 characters, as produced by StrKey.
fn is_contract_id(token: &str) -> bool {
    let t = token.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
    t.len() == 56
        && t.starts_with('C')
        && t[1..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
}

/// Whether the capability exists in the registry and is read-only.
pub fn capability_for(intent: &Intent) -> Option<&'static Capability> {
    sdkt_core::registry::find(intent.capability_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_health_with_contract() {
        let i = parse("check health of contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV on testnet").unwrap();
        assert_eq!(i.capability_id, "health");
        assert_eq!(
            i.args.contract.as_deref(),
            Some("CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV")
        );
        assert_eq!(i.args.network.as_deref(), Some("testnet"));
    }

    #[test]
    fn parses_verify_with_contract_and_wasm() {
        let i = parse("verify contract CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA against us_new.wasm on testnet")
            .unwrap();
        assert_eq!(i.capability_id, "verify");
        assert!(i.args.contract.is_some());
        assert_eq!(i.args.wasm.as_deref(), Some("us_new.wasm"));
    }

    #[test]
    fn parses_upgrade_safety_with_direction() {
        let i = parse("check whether this wasm is safe to upgrade from us_old.wasm to us_new.wasm")
            .unwrap();
        assert_eq!(i.capability_id, "diff.upgrade_safety");
        assert_eq!(i.args.previous_wasm.as_deref(), Some("us_old.wasm"));
        assert_eq!(i.args.wasm.as_deref(), Some("us_new.wasm"));
    }

    #[test]
    fn safe_to_upgrade_is_not_read_as_deploy() {
        // "upgrade" appears, but nothing asks for a deploy.
        let i = parse("is it safe to upgrade from a.wasm to b.wasm").unwrap();
        assert_eq!(i.capability_id, "diff.upgrade_safety");
    }

    #[test]
    fn parses_wasm_inspect_and_audit_and_doctor() {
        assert_eq!(
            parse("inspect wasm us_new.wasm").unwrap().capability_id,
            "wasm.inspect"
        );
        assert_eq!(parse("audit src/lib.rs").unwrap().capability_id, "audit");
        assert_eq!(parse("run doctor").unwrap().capability_id, "doctor");
        assert!(matches!(
            parse("check network status"),
            Err(ParseError::MissingArgument { capability_id, .. })
                if capability_id == "network.check"
        ));
        assert_eq!(
            parse("check network status profile wl-testnet")
                .unwrap()
                .capability_id,
            "network.check"
        );
    }

    #[test]
    fn ambiguous_request_is_refused() {
        // Two read-only operations in one breath.
        let e = parse("check health and verify the contract").unwrap_err();
        match e {
            ParseError::Ambiguous { candidates } => {
                assert!(candidates.contains(&"health"), "{candidates:?}");
                assert!(candidates.contains(&"verify"), "{candidates:?}");
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn unrecognised_request_is_refused() {
        let e = parse("make me a sandwich").unwrap_err();
        assert_eq!(e.code(), "unrecognised_request");
    }

    #[test]
    fn mutation_requests_are_named_precisely() {
        for (req, expected) in [
            ("deploy this contract to testnet", "deploy"),
            ("invoke the mint function", "invoke"),
            ("submit transaction AAAA", "tx.submit"),
            ("extend ttl of the contract", "storage.extend"),
            ("fund account alice", "identity.fund"),
        ] {
            let e = parse(req).unwrap_err();
            match e {
                ParseError::MutationRequested { capability_id } => {
                    assert_eq!(capability_id, expected, "for {req:?}")
                }
                other => panic!("{req:?}: expected MutationRequested, got {other:?}"),
            }
        }
    }

    #[test]
    fn missing_contract_is_reported_not_guessed() {
        let e = parse("check health of the contract").unwrap_err();
        match e {
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "health");
                assert!(argument.contains("contract"));
            }
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[test]
    fn missing_wasm_for_verify_is_reported() {
        let e = parse("verify contract CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .unwrap_err();
        match e {
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "verify");
                assert!(argument.contains("wasm"));
            }
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[test]
    fn missing_baseline_for_upgrade_is_reported() {
        let e = parse("is it safe to upgrade to new.wasm").unwrap_err();
        match e {
            ParseError::MissingArgument { argument, .. } => assert!(argument.contains("baseline")),
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[test]
    fn contract_id_detection_rejects_lookalikes() {
        assert!(is_contract_id(
            "CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV"
        ));
        assert!(!is_contract_id(
            "GAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV"
        ));
        assert!(!is_contract_id("CtooShort"));
    }

    #[test]
    fn mainnet_is_extracted_so_the_planner_can_refuse_it() {
        let i = parse("check health of contract CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA on mainnet")
            .unwrap();
        assert_eq!(i.args.network.as_deref(), Some("mainnet"));
    }
}
