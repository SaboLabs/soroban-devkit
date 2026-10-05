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
//! ambiguous rather than arbitrarily resolved. Artifact roles refuse for the
//! same reason: two paths whose candidate/baseline roles nothing in the
//! request marks are never assigned by listing order, because a silent
//! inversion compares an upgrade backwards.

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
    /// The candidate (newly built) artifact, once roles resolve.
    pub wasm: Option<String>,
    /// The baseline (previously deployed) artifact, once roles resolve.
    pub previous_wasm: Option<String>,
    /// Every artifact path the request named (bare or flag-supplied), in the
    /// order named. The planner checks this against the capability's
    /// artifact capacity — extras are refused, never dropped.
    pub artifact_paths: Vec<String>,
    /// Paths claimed as *candidate* by an artifact flag, a preceding marker
    /// word ("candidate", "new", "current", "to"), or the file's own name.
    pub candidate_claims: Vec<String>,
    /// Paths claimed as *baseline* by an artifact flag, a preceding marker
    /// word ("previous", "baseline", "old", "from"), or the file's own name.
    pub baseline_claims: Vec<String>,
    /// A `.rs` path for the audit.
    pub source_path: Option<String>,
    /// A contract function name (for `call`).
    pub function: Option<String>,
    /// A saved network profile name (for `network check`).
    pub profile: Option<String>,
    /// Network named in the request, normalised lowercase.
    pub network: Option<String>,
    /// An RPC endpoint URL supplied by the request (bare URL or `--rpc-url`).
    pub rpc_url: Option<String>,
    /// A passphrase supplied explicitly via `--network-passphrase`.
    pub network_passphrase: Option<String>,
    /// A transaction envelope: a path ending `.xdr`, or a long base64 blob.
    pub envelope: Option<String>,
    /// Typed function arguments (`u32:100`, `address:G...`) for `call`.
    pub typed_args: Vec<String>,
    /// True when the request asks for live RPC fee statistics (`--rpc`).
    pub use_rpc: bool,
    /// `--base-fees <v>` value supplied by the request.
    pub base_fees: Option<String>,
    /// Every `--flag` the request spelled out, with its captured value. The
    /// planner maps the ones the registry declares and refuses the rest —
    /// nothing the user asked for is dropped without saying so.
    pub explicit_flags: Vec<(String, Option<String>)>,
}

impl IntentArgs {
    /// The value captured for `flag`, if the request spelled it.
    pub(crate) fn flag_value(&self, flag: &str) -> Option<&str> {
        self.explicit_flags
            .iter()
            .find(|(f, _)| f == flag)
            .and_then(|(_, v)| v.as_deref())
    }

    /// All values captured for repeatable `flag`.
    pub(crate) fn flag_values(&self, flag: &str) -> Vec<String> {
        self.explicit_flags
            .iter()
            .filter(|(f, _)| f == flag)
            .filter_map(|(_, v)| v.clone())
            .collect()
    }

    /// True when the request spelled `flag` at all.
    pub(crate) fn has_flag(&self, flag: &str) -> bool {
        self.explicit_flags.iter().any(|(f, _)| f == flag)
    }

    /// True when the request marked artifact roles somewhere.
    fn roles_claimed(&self) -> bool {
        !(self.candidate_claims.is_empty() && self.baseline_claims.is_empty())
    }
}

/// Prefixes of the CLI's typed `TYPE:VALUE` argument syntax, used to notice a
/// function argument in free text without guessing its type or value.
const TYPED_ARG_PREFIXES: &[&str] = &[
    "u32:",
    "i32:",
    "u64:",
    "i64:",
    "u128:",
    "i128:",
    "bool:",
    "string:",
    "symbol:",
    "bytes:",
    "address:",
    "duration:",
    "timepoint:",
];

/// Words that mark an artifact as the candidate.
const CANDIDATE_MARKERS: &[&str] = &["candidate", "new", "current", "to"];
/// Words that mark an artifact as the baseline.
const BASELINE_MARKERS: &[&str] = &["previous", "baseline", "old", "from"];
/// Format words the CLI accepts as `--format` values (not shapeful, so they
/// need spelling out — prose words must never be swallowed as values).
const FORMAT_WORDS: &[&str] = &["json", "pretty"];

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
    /// The request carries two artifact paths and nothing says which is the
    /// candidate and which the baseline. Roles are never inferred from the
    /// order the paths happen to appear in.
    AmbiguousArtifactRoles {
        /// Capability id.
        capability_id: &'static str,
    },
    /// The request names an argument the capability cannot express (an
    /// unknown flag, or more artifacts than the capability takes). Refused
    /// rather than dropped, so a partial request never runs as if complete.
    UnsupportedArgument {
        /// Capability id.
        capability_id: &'static str,
        /// The unmappable argument(s), verbatim.
        arguments: String,
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
            ParseError::AmbiguousArtifactRoles { .. } => "ambiguous_artifact_roles",
            ParseError::UnsupportedArgument { .. } => "unsupported_argument",
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
            ParseError::AmbiguousArtifactRoles { capability_id } => format!(
                "{capability_id} was given two WASM artifacts but nothing in the request says \
                 which is the candidate and which the baseline; guessing from path order could \
                 compare the upgrade backwards, so nothing was run. Mark the roles explicitly \
                 ({} for --wasm, {} for --previous-wasm).",
                CANDIDATE_MARKERS.join("/"),
                BASELINE_MARKERS.join("/")
            ),
            ParseError::UnsupportedArgument {
                capability_id,
                arguments,
            } => format!(
                "{capability_id} cannot use {arguments}; refusing to run a partial request. \
                 Remove the unsupported argument or rephrase."
            ),
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
    ("network.list", &[&["networks"], &["network", "list"]]),
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
            &["call", "on", "contract"],
        ],
    ),
    ("diff", &[&["diff"]]),
    (
        "project.status",
        &[&["project", "status"], &["project", "deployment"]],
    ),
    (
        "fee.estimate",
        &[
            &["fee", "estimate"],
            &["estimate", "fee"],
            &["fee", "stats"],
        ],
    ),
    (
        "tx.simulate",
        &[
            &["simulate", "transaction"],
            &["simulate", "tx"],
            &["simulate", "envelope"],
            &["simulate"],
        ],
    ),
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
///
/// Artifact *capacity* (how many paths a capability can take) and flag
/// mappability are checked by the planner, which is where the registry entry
/// is in hand; role ambiguity is resolved here because the request text alone
/// decides it.
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
            (None, Some(_)) | (None, None)
                if args.artifact_paths.len() == 2 && !args.roles_claimed() =>
            {
                // Two artifacts, nothing marking the roles: the missing side
                // is not a missing argument, it is an unresolved direction.
                Err(ParseError::AmbiguousArtifactRoles { capability_id: id })
            }
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
        "release_assurance" if args.wasm.is_none() => {
            if args.artifact_paths.len() > 1 && !args.roles_claimed() {
                Err(ParseError::AmbiguousArtifactRoles { capability_id: id })
            } else {
                Err(ParseError::MissingArgument {
                    capability_id: id,
                    argument: "candidate wasm path",
                })
            }
        }
        "call" if args.function.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "function name",
        }),
        // Fee estimation needs one of the two sources the CLI itself offers;
        // neither is inventable, so an absent choice is a clarification.
        "fee.estimate" if args.base_fees.is_none() && !args.use_rpc && args.rpc_url.is_none() => {
            Err(ParseError::MissingArgument {
                capability_id: id,
                argument: "--base-fees <f1,f2,f3> or an RPC endpoint (--rpc-url) to query",
            })
        }
        "tx.simulate" if args.envelope.is_none() => Err(ParseError::MissingArgument {
            capability_id: id,
            argument: "transaction envelope (base64 XDR or a .xdr file path)",
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
/// Contract ids are found by shape (`C` + 55 base32 chars), artifact paths by
/// `.wasm` suffix, source paths by `.rs`, envelopes by `.xdr`/long base64,
/// endpoints by `http(s)://`, typed call arguments by `TYPE:VALUE` prefix.
/// Dash-flags are captured verbatim with a shapeful (or quoted, or known-word)
/// value so the planner can map what the registry declares and refuse the
/// rest — nothing the user asked for is dropped without saying so.
fn extract_args(request: &str) -> IntentArgs {
    let mut args = IntentArgs::default();
    let raw_tokens: Vec<&str> = request.split_whitespace().collect();

    // Marker word pending for the next bare artifact path.
    let mut pending_role: Option<bool> = None; // true = baseline, false = candidate
    let mut i = 0;
    while i < raw_tokens.len() {
        let cleaned = trim_punct(raw_tokens[i]);
        let lower = cleaned.to_ascii_lowercase();

        if cleaned.starts_with("--") && cleaned.len() > 2 {
            let (flag, eq_value) = match cleaned.split_once('=') {
                Some((f, v)) if !v.is_empty() => (f.to_string(), Some(v.to_string())),
                _ => (cleaned.to_string(), None),
            };
            // A flag's value is either spelled into the token, quoted (a
            // quoted value may contain spaces — passphrases do), or a
            // shapeful/known-word token following it. Ordinary prose is never
            // swallowed, so a value the parser cannot trust stays absent and
            // the planner refuses rather than guessing.
            let mut value = eq_value;
            // Tokens consumed by this flag: 1 = flag alone.
            let mut advance = 1usize;
            if value.is_none() {
                if let Some(next) = raw_tokens.get(i + 1) {
                    if let Some((quoted, used)) = quoted_value(&raw_tokens[i + 1..]) {
                        value = Some(quoted);
                        advance = 1 + used;
                    } else {
                        let n = trim_punct(next);
                        if !n.is_empty()
                            && !n.starts_with('-')
                            && (looks_like_value(n)
                                || FORMAT_WORDS.contains(&n.to_ascii_lowercase().as_str()))
                        {
                            value = Some(n.to_string());
                            advance = 2;
                        }
                    }
                }
            }
            // An artifact flag claims the role its spelling names.
            match flag.as_str() {
                "--wasm" | "--new-wasm" => {
                    if let Some(v) = &value {
                        args.candidate_claims.push(v.clone());
                        push_path(&mut args, v);
                        pending_role = None;
                    }
                }
                "--previous-wasm" | "--old-wasm" => {
                    if let Some(v) = &value {
                        args.baseline_claims.push(v.clone());
                        push_path(&mut args, v);
                        pending_role = None;
                    }
                }
                _ => {}
            }
            args.explicit_flags.push((flag, value));
            i += advance;
            continue;
        }

        if is_contract_id(cleaned) {
            args.contract = Some(cleaned.to_string());
            i += 1;
            continue;
        }
        if lower.ends_with(".wasm") {
            push_path(&mut args, cleaned);
            // Claim by the marker word that precedes the path, or by the
            // path's own name; never by its position in the listing.
            let claim = pending_role.take().or_else(|| role_from_name(&lower));
            match claim {
                Some(true) => args.baseline_claims.push(cleaned.to_string()),
                Some(false) => args.candidate_claims.push(cleaned.to_string()),
                None => {}
            }
            i += 1;
            continue;
        }
        if lower.ends_with(".rs") {
            args.source_path = Some(cleaned.to_string());
            i += 1;
            continue;
        }
        if lower.ends_with(".xdr") || is_base64_blob(cleaned) {
            let _ = args.envelope.get_or_insert_with(|| cleaned.to_string());
            i += 1;
            continue;
        }
        if looks_like_url(&lower) {
            let _ = args.rpc_url.get_or_insert_with(|| cleaned.to_string());
            i += 1;
            continue;
        }
        if lower
            .split_once(':')
            .is_some_and(|(t, v)| !v.is_empty() && TYPED_ARG_PREFIXES.contains(&t))
        {
            args.typed_args.push(cleaned.to_string());
            i += 1;
            continue;
        }
        if matches!(lower.as_str(), "testnet" | "mainnet" | "futurenet") {
            args.network = Some(lower);
            i += 1;
            continue;
        }
        if BASELINE_MARKERS.contains(&lower.as_str()) {
            pending_role = Some(true);
            i += 1;
            continue;
        }
        if CANDIDATE_MARKERS.contains(&lower.as_str()) {
            pending_role = Some(false);
            i += 1;
            continue;
        }
        i += 1;
    }

    // Flag-carried values: `--base-fees <v>` supplies samples; `--rpc` (the
    // bare flag) asks for live statistics; `--rpc-url <v>` supplies an
    // endpoint; `--envelope <v>` and repeated `--args <v>` carry their data.
    args.base_fees = args.flag_value("--base-fees").map(str::to_string);
    args.use_rpc = args.has_flag("--rpc");
    // "estimate fee from rpc" names the live source in prose, not as a flag.
    if !args.use_rpc {
        args.use_rpc = raw_tokens
            .iter()
            .any(|t| trim_punct(t).eq_ignore_ascii_case("rpc"));
    }
    if let Some(v) = args.flag_value("--rpc-url") {
        args.rpc_url = Some(v.to_string());
    }
    args.network_passphrase = args.flag_value("--network-passphrase").map(str::to_string);
    if let Some(v) = args.flag_value("--envelope") {
        args.envelope = Some(v.to_string());
    }
    for v in args.flag_values("--args") {
        if !args.typed_args.contains(&v) {
            args.typed_args.push(v);
        }
    }

    assign_artifact_roles(&mut args);

    args.profile = extract_profile(&raw_tokens);
    let wants_function = raw_tokens.iter().any(|t| {
        let c = trim_punct(t).to_ascii_lowercase();
        c == "function" || c == "call"
    });
    if wants_function {
        args.function = extract_function(&raw_tokens);
    }
    args
}

/// Record an artifact path once (a flag-supplied value must not double-count
/// against the capability's artifact capacity).
fn push_path(args: &mut IntentArgs, path: &str) {
    if !args
        .artifact_paths
        .iter()
        .any(|p| p.eq_ignore_ascii_case(path))
    {
        args.artifact_paths.push(path.to_string());
    }
}

/// Capture a whitespace-quoted value starting at `tokens[0]`, returning the
/// inner text and how many tokens it spans. Only quotes count — an unquoted
/// multi-word value is ambiguous by construction and stays uncaptured.
fn quoted_value(tokens: &[&str]) -> Option<(String, usize)> {
    let first = *tokens.first()?;
    let quote = first.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &first[1..];
    // Closing quote on the opening token: single-token value.
    if let Some(end) = rest.find(quote) {
        return Some((rest[..end].to_string(), 1));
    }
    // Scan a bounded run of following tokens for the closing quote.
    let mut parts: Vec<String> = Vec::new();
    for (k, t) in tokens.iter().enumerate().skip(1).take(16) {
        if let Some(pos) = t.find(quote) {
            parts.push(t[..pos].to_string());
            let inner = parts.join(" ").trim().to_string();
            let joined = if inner.is_empty() {
                rest.to_string()
            } else {
                format!("{rest} {inner}")
            };
            return Some((joined, k + 1));
        }
        parts.push(t.to_string());
    }
    // Unterminated quote: nothing is captured, the flag stays value-less and
    // the planner refuses rather than inventing a value.
    None
}

/// Place candidate/baseline from explicit claims only.
///
/// Claim sources: artifact flags (`--wasm X`), marker words directly before a
/// path ("previous", "from", "to", "new", …), and role markers inside the
/// file's own name (`us_old.wasm`, `baseline.wasm`).
///
/// With exactly two paths, one explicit claim leaves the other path the
/// remaining role — elimination, not listing order. Two paths with no claims
/// at all stay unresolved: guessing there once inverted release assurance
/// ("new.wasm old.wasm" → `--wasm old --previous-wasm new`), which compares
/// an upgrade backwards.
fn assign_artifact_roles(args: &mut IntentArgs) {
    dedup_claims(&mut args.candidate_claims);
    dedup_claims(&mut args.baseline_claims);

    match args.artifact_paths.len() {
        0 => {}
        1 => args.wasm = args.artifact_paths.first().cloned(),
        2 => {
            let pair = match (
                args.candidate_claims.as_slice(),
                args.baseline_claims.as_slice(),
            ) {
                ([candidate], [baseline]) if candidate != baseline => {
                    Some((candidate.clone(), baseline.clone()))
                }
                ([candidate], []) => other_path(&args.artifact_paths, candidate)
                    .map(|base| (candidate.clone(), base)),
                ([], [baseline]) => {
                    other_path(&args.artifact_paths, baseline).map(|cand| (cand, baseline.clone()))
                }
                _ => None,
            };
            if let Some((cand, base)) = pair {
                args.wasm = Some(cand);
                args.previous_wasm = Some(base);
            }
        }
        // Three or more: capacity is the planner's refusal. If the claims
        // name two distinct roles, resolve them so the leftover path reaches
        // the capacity gate instead of a false role-ambiguity.
        _ => {
            if let ([candidate], [baseline]) = (
                args.candidate_claims.as_slice(),
                args.baseline_claims.as_slice(),
            ) {
                if candidate != baseline {
                    args.wasm = Some(candidate.clone());
                    args.previous_wasm = Some(baseline.clone());
                }
            }
        }
    }
}

fn dedup_claims(claims: &mut Vec<String>) {
    claims.sort();
    claims.dedup();
}

/// The path in `paths` that is not `claimed` (case-insensitively).
fn other_path(paths: &[String], claimed: &str) -> Option<String> {
    paths
        .iter()
        .find(|p| !p.eq_ignore_ascii_case(claimed))
        .cloned()
}

/// Role encoded in a file's own name: markers split on `-_ ` in the stem.
/// `true` = baseline, `false` = candidate, `None` = neither. A name claiming
/// both roles claims nothing.
fn role_from_name(lower_path: &str) -> Option<bool> {
    let stem = lower_path.strip_suffix(".wasm").unwrap_or(lower_path);
    let file = stem.rsplit(['/', '\\']).next().unwrap_or(stem);
    let parts: Vec<String> = file
        .split(['-', '_', ' '])
        .map(|p| p.to_ascii_lowercase())
        .collect();
    let base = parts.iter().any(|p| BASELINE_MARKERS.contains(&p.as_str()));
    let cand = parts
        .iter()
        .any(|p| CANDIDATE_MARKERS.contains(&p.as_str()));
    match (base, cand) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Trim the punctuation the request wraps around a token.
fn trim_punct(token: &str) -> &str {
    token.trim_matches(|c: char| {
        c == ',' || c == ';' || c == '"' || c == '\'' || c == '(' || c == ')'
    })
}

/// True when a token plausibly carries a flag value: paths, URLs, ids,
/// numbers, fee lists, typed args — not ordinary sentence words. A flag whose
/// value cannot be trusted is recorded value-less so the planner refuses
/// precisely instead of eating the next word.
fn looks_like_value(token: &str) -> bool {
    let t = trim_punct(token);
    !t.is_empty()
        && !t.starts_with('"')
        && !t.starts_with('\'')
        && (t.contains('/')
            || t.contains('.')
            || t.contains(':')
            || t.contains(',')
            || t.chars().next().is_some_and(|c| c.is_ascii_digit())
            || is_contract_id(t))
}

fn looks_like_url(lower: &str) -> bool {
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// A base64 XDR blob (long, base64 alphabet, no path separators). Kept
/// conservative: short tokens like "AAAA" are not treated as envelopes.
fn is_base64_blob(token: &str) -> bool {
    token.len() >= 64
        && !token.contains('/')
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Extract the function name for `call`.
///
/// Deterministic patterns only:
/// - "…NAME function…" — the word before `function` ("call balance function")
/// - "function NAME" — the word after it ("call function balance")
/// - "call NAME …" — the word directly after `call`
///
/// Candidates must look like contract function names and must not be the
/// surrounding sentence words; anything else yields `None` so validation asks
/// for the name instead of guessing.
fn extract_function(tokens: &[&str]) -> Option<String> {
    let fn_word = |t: &str| {
        let c = trim_punct(t);
        let lower = c.to_ascii_lowercase();
        (!c.is_empty() && !FUNCTION_STOP.contains(&lower.as_str()) && is_fn_name(c))
            .then(|| c.to_string())
    };
    if let Some(i) = tokens
        .iter()
        .position(|t| trim_punct(t).eq_ignore_ascii_case("function"))
    {
        if i > 0 {
            if let Some(name) = fn_word(tokens[i - 1]) {
                return Some(name);
            }
        }
        if let Some(next) = tokens.get(i + 1) {
            if let Some(name) = fn_word(next) {
                return Some(name);
            }
        }
    }
    if let Some(i) = tokens
        .iter()
        .position(|t| trim_punct(t).eq_ignore_ascii_case("call"))
    {
        if let Some(next) = tokens.get(i + 1) {
            if let Some(name) = fn_word(next) {
                return Some(name);
            }
        }
    }
    None
}

/// Sentence words that must never be read as a function name.
const FUNCTION_STOP: &[&str] = &[
    "call",
    "read",
    "invoke",
    "function",
    "on",
    "of",
    "the",
    "a",
    "an",
    "to",
    "for",
    "in",
    "with",
    "and",
    "contract",
    "check",
    "show",
    "run",
    "please",
    "now",
    "right",
    "what",
    "list",
    "estimate",
    "simulate",
    "using",
    "from",
    "previous",
    "candidate",
    "baseline",
];

/// A contract function name: a lower/snake identifier, not an id, path or
/// URL. Deliberately narrow so ordinary sentence words are never emitted as
/// a function name.
fn is_fn_name(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 64
        && !is_contract_id(t)
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
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
    Some(trim_punct(next).to_string())
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

    // ------------------------------------------------------------ P2-1 ---

    #[test]
    fn unmarked_two_artifacts_are_ambiguous_not_guessed() {
        // The inversion bug: two plain names used to be assigned by listing
        // order, so "candidate old" and "old candidate" could invert.
        for req in [
            "release assurance for a.wasm b.wasm",
            "release assurance for b.wasm a.wasm",
            "release assurance for releasea.wasm snapshot2026.wasm",
        ] {
            let e = parse(req).unwrap_err();
            match e {
                ParseError::AmbiguousArtifactRoles { capability_id } => {
                    assert_eq!(capability_id, "release_assurance", "{req:?}")
                }
                other => panic!("{req:?}: expected AmbiguousArtifactRoles, got {other:?}"),
            }
        }
    }

    #[test]
    fn explicit_word_markers_place_both_roles() {
        for (req, cand, base) in [
            (
                "release assurance for candidate new.wasm previous old.wasm",
                "new.wasm",
                "old.wasm",
            ),
            (
                "release assurance previous old.wasm candidate new.wasm",
                "new.wasm",
                "old.wasm",
            ),
            (
                "release assurance for current today.wasm from yesterday.wasm",
                "today.wasm",
                "yesterday.wasm",
            ),
        ] {
            let i = parse(req).unwrap_or_else(|e| panic!("{req:?}: {e:?}"));
            assert_eq!(i.args.wasm.as_deref(), Some(cand), "{req:?}");
            assert_eq!(i.args.previous_wasm.as_deref(), Some(base), "{req:?}");
        }
    }

    #[test]
    fn one_explicit_claim_leaves_the_other_path_determined() {
        // Exactly one path is marked; the remaining path must take the
        // remaining role — elimination, not list order.
        let i = parse("release assurance for candidate new.wasm build42.wasm").unwrap();
        assert_eq!(i.args.wasm.as_deref(), Some("new.wasm"));
        assert_eq!(i.args.previous_wasm.as_deref(), Some("build42.wasm"));
    }

    #[test]
    fn artifact_flag_claims_place_both_roles() {
        let i = parse("release assurance --wasm cand.wasm --previous-wasm base.wasm --format json")
            .unwrap();
        assert_eq!(i.args.wasm.as_deref(), Some("cand.wasm"));
        assert_eq!(i.args.previous_wasm.as_deref(), Some("base.wasm"));
    }

    #[test]
    fn role_markers_survive_reversed_phrasing() {
        let a = parse("release assurance for candidate.wasm previous baseline.wasm").unwrap();
        assert_eq!(a.args.wasm.as_deref(), Some("candidate.wasm"));
        assert_eq!(a.args.previous_wasm.as_deref(), Some("baseline.wasm"));
        let b =
            parse("release assurance previous baseline.wasm for candidate candidate.wasm").unwrap();
        assert_eq!(b.args.wasm.as_deref(), Some("candidate.wasm"));
        assert_eq!(b.args.previous_wasm.as_deref(), Some("baseline.wasm"));
    }

    #[test]
    fn contradictory_claims_never_run() {
        // Same path claimed as both roles: nothing may be guessed.
        let e = parse("release assurance for new.wasm previous new.wasm old.wasm").unwrap_err();
        assert!(
            matches!(
                e,
                ParseError::AmbiguousArtifactRoles { .. } | ParseError::MissingArgument { .. }
            ),
            "got {e:?}"
        );
    }

    #[test]
    fn from_to_direction_still_works() {
        let i = parse("is it safe to upgrade from us_old.wasm to us_new.wasm").unwrap();
        assert_eq!(i.args.previous_wasm.as_deref(), Some("us_old.wasm"));
        assert_eq!(i.args.wasm.as_deref(), Some("us_new.wasm"));
    }

    // ------------------------------------------------------------ P2-2 ---

    #[test]
    fn explicit_flags_are_captured_verbatim() {
        let i = parse(
            "inspect contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV --max-growth-pct 50",
        )
        .unwrap();
        assert!(i
            .args
            .explicit_flags
            .contains(&("--max-growth-pct".to_string(), Some("50".to_string()))));
    }

    #[test]
    fn rpc_url_flag_overrides_bare_url() {
        let i = parse(
            "inspect contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV --rpc-url https://rpc.example.com",
        )
        .unwrap();
        assert_eq!(i.args.rpc_url.as_deref(), Some("https://rpc.example.com"));
    }

    #[test]
    fn quoted_values_with_spaces_are_captured() {
        let i = parse(
            "inspect contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV --rpc-url https://rpc.example.com --network-passphrase \"Custom Net ; 2026\"",
        )
        .unwrap();
        assert_eq!(
            i.args.network_passphrase.as_deref(),
            Some("Custom Net ; 2026")
        );
    }

    #[test]
    fn prose_words_are_never_swallowed_as_flag_values() {
        let i = parse("inspect contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV --network-passphrase on testnet")
            .unwrap();
        assert!(
            i.args.network_passphrase.is_none(),
            "{:?}",
            i.args.explicit_flags
        );
    }

    #[test]
    fn three_artifacts_are_captured_for_the_capacity_gate() {
        let i = parse("release assurance for a.wasm b.wasm c.wasm").unwrap_err();
        match i {
            ParseError::AmbiguousArtifactRoles { .. } | ParseError::MissingArgument { .. } => {}
            other => panic!("expected an artifact refusal, got {other:?}"),
        }
    }

    // ------------------------------------------------------------ P2-3 ---

    #[test]
    fn network_list_is_reachable() {
        for req in ["list networks", "show available networks", "network list"] {
            let i = parse(req).unwrap_or_else(|e| panic!("{req:?}: {e:?}"));
            assert_eq!(i.capability_id, "network.list", "{req:?}");
        }
    }

    #[test]
    fn project_status_is_reachable() {
        for req in [
            "show project status",
            "what is the project deployment status",
        ] {
            let i = parse(req).unwrap_or_else(|e| panic!("{req:?}: {e:?}"));
            assert_eq!(i.capability_id, "project.status", "{req:?}");
        }
    }

    #[test]
    fn fee_estimate_is_reachable_and_source_required() {
        let i = parse("estimate the fee using https://soroban-testnet.stellar.org").unwrap();
        assert_eq!(i.capability_id, "fee.estimate");
        assert_eq!(
            i.args.rpc_url.as_deref(),
            Some("https://soroban-testnet.stellar.org")
        );

        let i = parse("fee estimate --rpc --rpc-url https://soroban-testnet.stellar.org").unwrap();
        assert!(i.args.use_rpc);

        let i = parse("estimate the fee --base-fees 100,120,110").unwrap();
        assert_eq!(i.args.base_fees.as_deref(), Some("100,120,110"));

        let e = parse("estimate the fee").unwrap_err();
        match e {
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "fee.estimate");
                assert!(argument.contains("base-fees"), "{argument}");
            }
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[test]
    fn tx_simulate_is_reachable_and_envelope_required() {
        let i = parse("simulate transaction from tx.xdr").unwrap();
        assert_eq!(i.capability_id, "tx.simulate");
        assert_eq!(i.args.envelope.as_deref(), Some("tx.xdr"));

        let i = parse("simulate transaction --envelope tx.xdr").unwrap();
        assert_eq!(i.args.envelope.as_deref(), Some("tx.xdr"));

        let e = parse("simulate this transaction").unwrap_err();
        match e {
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "tx.simulate");
                assert!(argument.contains("envelope"), "{argument}");
            }
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }

    #[test]
    fn call_extracts_function_and_contract() {
        let i = parse(
            "call function balance on contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV",
        )
        .unwrap();
        assert_eq!(i.capability_id, "call");
        assert_eq!(i.args.function.as_deref(), Some("balance"));
        assert!(i.args.contract.is_some());

        let i = parse(
            "call balance on contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV",
        )
        .unwrap();
        assert_eq!(i.args.function.as_deref(), Some("balance"));

        let i = parse("call function get --args u32:100 on contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV")
            .unwrap();
        assert_eq!(i.args.function.as_deref(), Some("get"));
        assert_eq!(i.args.typed_args, vec!["u32:100".to_string()]);
    }

    #[test]
    fn call_function_name_not_guessed_from_ordinary_words() {
        let e = parse(
            "call on contract CAD6C24POQGRYXMBNBEGVDHUROF5ZC37XRDC6NCVILTXWMYJIBMISZCV right now",
        )
        .unwrap_err();
        match e {
            ParseError::MissingArgument {
                capability_id,
                argument,
            } => {
                assert_eq!(capability_id, "call");
                assert!(argument.contains("function"), "{argument}");
            }
            other => panic!("expected MissingArgument, got {other:?}"),
        }
    }
}
