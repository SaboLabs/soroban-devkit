//! SARIF 2.1.0 serializer for [`AuditReport`].
//!
//! Converts an `AuditReport` into a valid
//! [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
//! log document that GitHub Code Scanning, GitLab, and Azure DevOps can ingest
//! natively.
//!
//! ## Mapping
//!
//! | `sdkt` concept        | SARIF concept                                      |
//! |-----------------------|----------------------------------------------------|
//! | `Finding.rule_id`     | `result.ruleId` + `run.tool.driver.rules[].id`     |
//! | `Severity::Critical`  | `result.level` = `"error"`                         |
//! | `Severity::Warning`   | `result.level` = `"warning"`                       |
//! | `Severity::Info`      | `result.level` = `"note"`                          |
//! | `Finding.message`     | `result.message.text`                              |
//! | audited file path     | `result.locations[].physicalLocation.artifactLocation.uri` |
//! | `Finding.location`    | `result.message.text` suffix (best-effort)         |
//!
//! Line/column information is not emitted today (the engine reports free-form
//! function names, not source spans). Adding structured line info is a
//! deliberate follow-up tracked separately.

use serde::Serialize;
use serde_json::Value;

use crate::types::{AuditReport, Finding, RuleInfo, Severity};

// ── SARIF 2.1.0 document structure ──────────────────────────────────────────

/// Top-level SARIF 2.1.0 log.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifLog {
    /// Must be "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0.json"
    #[serde(rename = "$schema")]
    pub schema: String,
    /// Must be "2.1.0".
    pub version: String,
    pub runs: Vec<SarifRun>,
}

/// One analysis run (one invocation of `sdkt audit`).
#[derive(Debug, Serialize)]
pub struct SarifRun {
    pub tool: SarifTool,
    pub results: Vec<SarifResult>,
}

#[derive(Debug, Serialize)]
pub struct SarifTool {
    pub driver: SarifDriver,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifDriver {
    pub name: String,
    pub version: String,
    pub information_uri: String,
    pub rules: Vec<SarifRule>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRule {
    pub id: String,
    pub name: String,
    pub short_description: SarifMessage,
    pub default_configuration: SarifConfiguration,
}

#[derive(Debug, Serialize)]
pub struct SarifConfiguration {
    pub level: String,
}

#[derive(Debug, Serialize)]
pub struct SarifMessage {
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifResult {
    pub rule_id: String,
    pub level: String,
    pub message: SarifMessage,
    pub locations: Vec<SarifLocation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifLocation {
    pub physical_location: SarifPhysicalLocation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifPhysicalLocation {
    pub artifact_location: SarifArtifactLocation,
}

#[derive(Debug, Serialize)]
pub struct SarifArtifactLocation {
    pub uri: String,
    #[serde(rename = "uriBaseId", skip_serializing_if = "Option::is_none")]
    pub uri_base_id: Option<String>,
}

// ── Severity mapping ─────────────────────────────────────────────────────────

fn severity_to_level(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

// ── URI helpers ───────────────────────────────────────────────────────────────

/// Return `true` when `source_file` is an absolute path on either Unix or
/// Windows.
///
/// Absolute paths must be converted to `file:` URIs rather than URI-references
/// so that SARIF consumers can resolve them without a `uriBaseId`.
fn is_absolute_source_path(source_file: &str) -> bool {
    // Unix absolute: starts with `/`
    if source_file.starts_with('/') {
        return true;
    }
    // Windows UNC absolute: `\\server\share\…` or `//server/share/…`
    if source_file.starts_with("\\\\") || source_file.starts_with("//") {
        return true;
    }
    // Windows drive-letter absolute: `C:\…` or `C:/…`
    let bytes = source_file.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    false
}

/// Percent-encode a single path *segment* (the text between `/` separators).
///
/// Encodes every byte that is not an RFC 3986 unreserved character or a
/// sub-delimiter safe in a path segment. `/` is handled by the callers and
/// must **not** be passed into this function.
fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            // RFC 3986 unreserved characters
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            // sub-delimiters that are safe in path segments (RFC 3986 §3.3)
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@' => out.push(byte as char),
            // everything else: space, %, #, ?, [, ], …
            b => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{:02X}", b);
            }
        }
    }
    out
}

/// Convert a relative file path into a percent-encoded URI-reference.
///
/// `/` separators are preserved; each segment between them is percent-encoded
/// individually. Backslashes are first normalised to forward slashes.
fn relative_uri_from_path(path: &str) -> String {
    let normalised = path.replace('\\', "/");
    normalised
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// Convert an absolute Unix or Windows path into a `file:` URI.
///
/// * Unix `/foo/bar baz.rs`           → `file:///foo/bar%20baz.rs`
/// * Windows `C:\foo\bar baz.rs`      → `file:///C:/foo/bar%20baz.rs`
/// * Windows UNC `\\srv\share\a b.rs` → `file://srv/share/a%20b.rs`
fn file_uri_from_path(path: &str) -> String {
    // Normalise Windows separators.
    let forward = path.replace('\\', "/");
    // Split on `/` and encode each segment.
    let encoded = forward
        .split('/')
        .map(|seg| {
            // Preserve empty segments (produced by a leading `/` on Unix paths
            // and by `//` on UNC paths) and the Windows drive segment (`C:`)
            // as-is.  `:` is allowed in path segments per RFC 3986 §3.3 and
            // must not be percent-encoded because `C%3A` is not a valid drive.
            if seg.is_empty() || (seg.len() == 2 && seg.as_bytes()[1] == b':') {
                seg.to_string()
            } else {
                encode_path_segment(seg)
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    // RFC 8089 URI construction:
    //
    // UNC path  \\server\share\path  normalises to  //server/share/path
    //   → authority = "server", path = "/share/path"
    //   → file://server/share/path
    //
    // Unix path  /foo/bar  (or ///foo/bar with extra leading slashes)
    //   → authority = "", path = "/foo/bar"
    //   → file:///foo/bar   (strip the leading `/` that trim gives us)
    //
    // Windows drive  C:/foo/bar
    //   → authority = "", path = "/C:/foo/bar"
    //   → file:///C:/foo/bar
    //
    // A path with three or more leading slashes (e.g. `///home/user/lib.rs`)
    // is a degenerate local Unix path, NOT a UNC path — guard the UNC branch
    // with `!starts_with("///")` so those extra slashes collapse correctly.
    if forward.starts_with("//") && !forward.starts_with("///") {
        // UNC: `encoded` = `//server/share/path`; `file://` + strip `//` =
        // `file://server/share/path` where `server` is the URI authority.
        let unc_part = encoded.trim_start_matches('/');
        format!("file://{}", unc_part)
    } else {
        // Unix / Windows drive / extra-slash Unix: strip leading `/` then
        // prepend `file:///`.
        let path_part = encoded.trim_start_matches('/');
        format!("file:///{}", path_part)
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Tool name embedded in every SARIF document emitted by `sdkt`.
pub const TOOL_NAME: &str = "sdkt";
/// Informational URI referenced in the SARIF driver entry.
pub const TOOL_URI: &str = "https://github.com/SaboLabs/soroban-devkit";
/// SARIF schema URI (§3.13.3).
pub const SARIF_SCHEMA: &str =
    "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0.json";
/// SARIF specification version.
pub const SARIF_VERSION: &str = "2.1.0";

/// Convert an [`AuditReport`] into a [`SarifLog`].
///
/// `source_file` is the path that was audited (e.g. `contracts/token/src/lib.rs`).
/// It is embedded as the `artifactLocation.uri` for every result, making findings
/// land on the correct file in GitHub Code Scanning even without line numbers.
///
/// `sdkt_version` is embedded in the tool driver (e.g. from `env!("CARGO_PKG_VERSION")`).
///
/// `rules_info` provides the human-readable descriptions for each rule id so
/// the `rules` section of the SARIF document is populated correctly. Any rule id
/// seen in `report.findings` that is not covered by `rules_info` will receive a
/// synthetic entry with the id as both name and description.
pub fn to_sarif(
    report: &AuditReport,
    source_file: &str,
    sdkt_version: &str,
    rules_info: &[RuleInfo],
) -> SarifLog {
    // Collect unique rule ids from findings, preserving first-seen order.
    let mut seen_ids: Vec<String> = Vec::new();
    for f in &report.findings {
        if !seen_ids.contains(&f.rule_id) {
            seen_ids.push(f.rule_id.clone());
        }
    }

    // Build the rules section: use provided RuleInfo where available, fall back
    // to a synthetic entry otherwise.
    let rules: Vec<SarifRule> = seen_ids
        .iter()
        .map(|id| {
            let info = rules_info.iter().find(|r| &r.id == id);
            let (description, severity) = match info {
                Some(ri) => (ri.description.clone(), ri.severity),
                None => (id.clone(), Severity::Warning),
            };
            SarifRule {
                id: id.clone(),
                name: id.clone(),
                short_description: SarifMessage { text: description },
                default_configuration: SarifConfiguration {
                    level: severity_to_level(severity).to_string(),
                },
            }
        })
        .collect();

    // Build results.
    let results: Vec<SarifResult> = report
        .findings
        .iter()
        .map(|f| finding_to_result(f, source_file))
        .collect();

    SarifLog {
        schema: SARIF_SCHEMA.to_string(),
        version: SARIF_VERSION.to_string(),
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: TOOL_NAME.to_string(),
                    version: sdkt_version.to_string(),
                    information_uri: TOOL_URI.to_string(),
                    rules,
                },
            },
            results,
        }],
    }
}

/// Serialize an [`AuditReport`] to a SARIF 2.1.0 JSON string.
///
/// This is the entry point used by the CLI `--format sarif` path.
pub fn report_to_sarif_string(
    report: &AuditReport,
    source_file: &str,
    sdkt_version: &str,
    rules_info: &[RuleInfo],
) -> Result<String, serde_json::Error> {
    let log = to_sarif(report, source_file, sdkt_version, rules_info);
    serde_json::to_string(&log)
}

fn finding_to_result(f: &Finding, source_file: &str) -> SarifResult {
    // Compose message text: include the optional location as a parenthetical
    // so it is still visible in tools that only show the message.
    let message_text = match &f.location {
        Some(loc) => format!("{} ({})", f.message, loc),
        None => f.message.clone(),
    };

    let is_absolute = is_absolute_source_path(source_file);
    let (uri, uri_base_id) = if is_absolute {
        // Absolute paths become self-contained `file:` URIs; no base ID needed.
        (file_uri_from_path(source_file), None)
    } else {
        // Relative paths are encoded as URI-references and anchored to the
        // repository root via %SRCROOT% so GitHub Code Scanning resolves them.
        (
            relative_uri_from_path(source_file),
            Some("%SRCROOT%".to_string()),
        )
    };

    SarifResult {
        rule_id: f.rule_id.clone(),
        level: severity_to_level(f.severity).to_string(),
        message: SarifMessage { text: message_text },
        locations: vec![SarifLocation {
            physical_location: SarifPhysicalLocation {
                artifact_location: SarifArtifactLocation { uri, uri_base_id },
            },
        }],
    }
}

/// Convert a [`SarifLog`] into a [`serde_json::Value`] for schema validation
/// or further inspection.
pub fn sarif_to_value(log: &SarifLog) -> Result<Value, serde_json::Error> {
    serde_json::to_value(log)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AuditReport, Finding, RuleInfo, Severity};

    fn rule_info(id: &str, sev: Severity, desc: &str) -> RuleInfo {
        RuleInfo {
            id: id.to_string(),
            severity: sev,
            description: desc.to_string(),
        }
    }

    fn finding(rule_id: &str, sev: Severity, msg: &str, loc: Option<&str>) -> Finding {
        Finding {
            rule_id: rule_id.to_string(),
            severity: sev,
            message: msg.to_string(),
            location: loc.map(str::to_string),
        }
    }

    fn make_rules() -> Vec<RuleInfo> {
        vec![
            rule_info(
                "AUTH-001",
                Severity::Critical,
                "Missing require_auth() in privileged function",
            ),
            rule_info(
                "AUTH-003",
                Severity::Warning,
                "Unguarded initialize function",
            ),
            rule_info("MOVE-001", Severity::Info, "Suspicious move-after-use"),
        ]
    }

    // ── Severity mapping ──────────────────────────────────────────────────

    #[test]
    fn critical_maps_to_error() {
        assert_eq!(severity_to_level(Severity::Critical), "error");
    }

    #[test]
    fn warning_maps_to_warning() {
        assert_eq!(severity_to_level(Severity::Warning), "warning");
    }

    #[test]
    fn info_maps_to_note() {
        assert_eq!(severity_to_level(Severity::Info), "note");
    }

    // ── Empty-findings case ───────────────────────────────────────────────

    #[test]
    fn empty_report_produces_valid_sarif_with_zero_results() {
        let report = AuditReport::default();
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());

        assert_eq!(log.version, "2.1.0");
        assert_eq!(log.schema, SARIF_SCHEMA);
        assert_eq!(log.runs.len(), 1);
        let run = &log.runs[0];
        assert!(run.results.is_empty(), "no results for empty report");
        assert!(
            run.tool.driver.rules.is_empty(),
            "no rules section for empty report"
        );
    }

    #[test]
    fn empty_report_serialises_to_valid_json() {
        let report = AuditReport::default();
        let json = report_to_sarif_string(&report, "src/lib.rs", "2.5.0", &make_rules())
            .expect("serialization succeeds");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["version"], "2.1.0");
        let results = v["runs"][0]["results"].as_array().unwrap();
        assert!(results.is_empty());
    }

    // ── Single finding per severity ───────────────────────────────────────

    #[test]
    fn critical_finding_emits_error_level() {
        let mut report = AuditReport::default();
        report.add(finding(
            "AUTH-001",
            Severity::Critical,
            "Missing require_auth",
            Some("mint_token"),
        ));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        let result = &log.runs[0].results[0];
        assert_eq!(result.rule_id, "AUTH-001");
        assert_eq!(result.level, "error");
    }

    #[test]
    fn warning_finding_emits_warning_level() {
        let mut report = AuditReport::default();
        report.add(finding(
            "AUTH-003",
            Severity::Warning,
            "Unguarded initialize",
            None,
        ));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        assert_eq!(log.runs[0].results[0].level, "warning");
    }

    #[test]
    fn info_finding_emits_note_level() {
        let mut report = AuditReport::default();
        report.add(finding("MOVE-001", Severity::Info, "Possible reuse", None));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        assert_eq!(log.runs[0].results[0].level, "note");
    }

    // ── Rule ids in rules section ─────────────────────────────────────────

    #[test]
    fn rules_section_contains_each_unique_rule_id() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        report.add(finding("AUTH-001", Severity::Critical, "msg2", None)); // duplicate id
        report.add(finding("AUTH-003", Severity::Warning, "msg3", None));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        let rule_ids: Vec<&str> = log.runs[0]
            .tool
            .driver
            .rules
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        // AUTH-001 appears once despite two findings; AUTH-003 also present.
        assert_eq!(rule_ids, vec!["AUTH-001", "AUTH-003"]);
    }

    #[test]
    fn rule_description_comes_from_rules_info() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        let rule = &log.runs[0].tool.driver.rules[0];
        assert_eq!(
            rule.short_description.text,
            "Missing require_auth() in privileged function"
        );
    }

    #[test]
    fn unknown_rule_id_gets_synthetic_entry() {
        let mut report = AuditReport::default();
        report.add(finding("CUSTOM-999", Severity::Warning, "custom msg", None));
        // No RuleInfo for CUSTOM-999
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &[]);
        let rule = &log.runs[0].tool.driver.rules[0];
        assert_eq!(rule.id, "CUSTOM-999");
        assert_eq!(rule.short_description.text, "CUSTOM-999");
    }

    // ── Artifact location (file path) ─────────────────────────────────────

    #[test]
    fn result_artifact_uri_relative_path_has_srcroot_base_id() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        let log = to_sarif(
            &report,
            "contracts/token/src/lib.rs",
            "2.5.0",
            &make_rules(),
        );
        let loc = &log.runs[0].results[0].locations[0];
        assert_eq!(
            loc.physical_location.artifact_location.uri,
            "contracts/token/src/lib.rs"
        );
        assert_eq!(
            loc.physical_location.artifact_location.uri_base_id,
            Some("%SRCROOT%".to_string())
        );
    }

    #[test]
    fn result_artifact_uri_unix_absolute_path_becomes_file_uri_no_base_id() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        let log = to_sarif(&report, "/home/user/contracts/lib.rs", "2.5.0", &[]);
        let loc = &log.runs[0].results[0].locations[0];
        assert_eq!(
            loc.physical_location.artifact_location.uri,
            "file:///home/user/contracts/lib.rs"
        );
        assert_eq!(
            loc.physical_location.artifact_location.uri_base_id, None,
            "absolute paths must not carry uriBaseId"
        );
    }

    #[test]
    fn result_artifact_uri_windows_absolute_path_becomes_file_uri_no_base_id() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        // Simulate a Windows path passed from the CLI
        let log = to_sarif(&report, "C:\\Users\\dev\\contracts\\lib.rs", "2.5.0", &[]);
        let loc = &log.runs[0].results[0].locations[0];
        let uri = &loc.physical_location.artifact_location.uri;
        assert!(
            uri.starts_with("file://"),
            "Windows path must become file: URI"
        );
        assert!(
            uri.contains("contracts/lib.rs"),
            "path segments must be preserved"
        );
        assert_eq!(
            loc.physical_location.artifact_location.uri_base_id, None,
            "absolute paths must not carry uriBaseId"
        );
    }

    // ── URI helper unit tests ─────────────────────────────────────────────

    #[test]
    fn encode_path_segment_passthrough_unreserved() {
        assert_eq!(encode_path_segment("lib.rs"), "lib.rs");
        assert_eq!(encode_path_segment("my-contract_v1~"), "my-contract_v1~");
    }

    #[test]
    fn encode_path_segment_encodes_space_and_percent() {
        assert_eq!(encode_path_segment("my file"), "my%20file");
        assert_eq!(encode_path_segment("100%"), "100%25");
    }

    #[test]
    fn encode_path_segment_encodes_hash_query_brackets() {
        assert_eq!(encode_path_segment("a#b"), "a%23b");
        assert_eq!(encode_path_segment("a?b"), "a%3Fb");
        assert_eq!(encode_path_segment("a[b]"), "a%5Bb%5D");
    }

    #[test]
    fn relative_uri_from_path_preserves_slashes() {
        assert_eq!(
            relative_uri_from_path("contracts/token/src/lib.rs"),
            "contracts/token/src/lib.rs"
        );
    }

    #[test]
    fn relative_uri_from_path_encodes_spaces_in_segments() {
        assert_eq!(
            relative_uri_from_path("my contracts/token lib.rs"),
            "my%20contracts/token%20lib.rs"
        );
    }

    #[test]
    fn relative_uri_from_path_normalises_backslashes() {
        assert_eq!(
            relative_uri_from_path("contracts\\token\\lib.rs"),
            "contracts/token/lib.rs"
        );
    }

    #[test]
    fn is_absolute_source_path_unix() {
        assert!(is_absolute_source_path("/home/user/lib.rs"));
        assert!(!is_absolute_source_path("contracts/lib.rs"));
    }

    #[test]
    fn is_absolute_source_path_windows() {
        assert!(is_absolute_source_path("C:\\Users\\dev\\lib.rs"));
        assert!(is_absolute_source_path("D:/projects/lib.rs"));
        assert!(!is_absolute_source_path("contracts/lib.rs"));
    }

    #[test]
    fn file_uri_from_path_unix_encodes_spaces() {
        assert_eq!(
            file_uri_from_path("/home/my user/lib.rs"),
            "file:///home/my%20user/lib.rs"
        );
    }

    #[test]
    fn file_uri_from_path_windows_produces_triple_slash() {
        let uri = file_uri_from_path("C:\\Users\\dev\\lib.rs");
        assert!(
            uri.starts_with("file:///"),
            "drive path needs empty authority"
        );
        assert!(uri.contains("C:"));
        assert!(!uri.contains('\\'), "backslashes must be converted");
    }

    #[test]
    fn is_absolute_source_path_unc() {
        assert!(is_absolute_source_path("\\\\server\\share\\file.rs"));
        assert!(is_absolute_source_path("//server/share/file.rs"));
        // relative paths with a single slash prefix are not UNC
        assert!(!is_absolute_source_path("contracts/lib.rs"));
    }

    #[test]
    fn file_uri_from_path_unc_preserves_server_as_authority() {
        // \\server\share\a b.rs → file://server/share/a%20b.rs
        assert_eq!(
            file_uri_from_path("\\\\server\\share\\a b.rs"),
            "file://server/share/a%20b.rs"
        );
    }

    #[test]
    fn file_uri_from_path_unc_forward_slash_form() {
        assert_eq!(
            file_uri_from_path("//server/share/lib.rs"),
            "file://server/share/lib.rs"
        );
    }

    #[test]
    fn file_uri_from_path_triple_slash_unix_is_local_not_unc() {
        // ///home/user/lib.rs has three leading slashes — it is a degenerate
        // local Unix path, not a UNC path. `home` must NOT become the authority.
        assert_eq!(
            file_uri_from_path("///home/user/lib.rs"),
            "file:///home/user/lib.rs"
        );
    }

    #[test]
    fn result_artifact_uri_unc_path_becomes_file_uri_no_base_id() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        let log = to_sarif(&report, "\\\\srv\\share\\lib.rs", "2.5.0", &[]);
        let loc = &log.runs[0].results[0].locations[0];
        let uri = &loc.physical_location.artifact_location.uri;
        assert!(
            uri.starts_with("file://srv"),
            "UNC server must be URI authority"
        );
        assert_eq!(
            loc.physical_location.artifact_location.uri_base_id, None,
            "UNC paths must not carry uriBaseId"
        );
    }

    // ── Location string appended to message ───────────────────────────────

    #[test]
    fn location_string_appended_to_message_text() {
        let mut report = AuditReport::default();
        report.add(finding(
            "AUTH-001",
            Severity::Critical,
            "Missing auth",
            Some("mint_token"),
        ));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        let msg = &log.runs[0].results[0].message.text;
        assert!(msg.contains("Missing auth"), "base message present");
        assert!(msg.contains("mint_token"), "location appended");
    }

    #[test]
    fn no_location_message_unchanged() {
        let mut report = AuditReport::default();
        report.add(finding(
            "AUTH-001",
            Severity::Critical,
            "Missing auth",
            None,
        ));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        assert_eq!(log.runs[0].results[0].message.text, "Missing auth");
    }

    // ── All three severities in one report ────────────────────────────────

    #[test]
    fn all_severities_map_correctly_in_one_report() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "c", None));
        report.add(finding("AUTH-003", Severity::Warning, "w", None));
        report.add(finding("MOVE-001", Severity::Info, "i", None));
        let log = to_sarif(&report, "src/lib.rs", "2.5.0", &make_rules());
        let levels: Vec<&str> = log.runs[0]
            .results
            .iter()
            .map(|r| r.level.as_str())
            .collect();
        assert_eq!(levels, vec!["error", "warning", "note"]);
    }

    // ── Tool metadata ─────────────────────────────────────────────────────

    #[test]
    fn tool_driver_name_and_version() {
        let report = AuditReport::default();
        let log = to_sarif(&report, "src/lib.rs", "1.2.3", &[]);
        let driver = &log.runs[0].tool.driver;
        assert_eq!(driver.name, "sdkt");
        assert_eq!(driver.version, "1.2.3");
        assert_eq!(driver.information_uri, TOOL_URI);
    }

    // ── JSON serialization field names ────────────────────────────────────

    #[test]
    fn json_field_names_are_camel_case() {
        let mut report = AuditReport::default();
        report.add(finding("AUTH-001", Severity::Critical, "msg", None));
        let json = report_to_sarif_string(&report, "src/lib.rs", "2.5.0", &make_rules())
            .expect("serialization");
        // Spot-check camelCase field names required by the SARIF schema.
        assert!(json.contains("\"ruleId\""), "ruleId field");
        assert!(
            json.contains("\"physicalLocation\""),
            "physicalLocation field"
        );
        assert!(
            json.contains("\"artifactLocation\""),
            "artifactLocation field"
        );
        assert!(json.contains("\"uriBaseId\""), "uriBaseId field");
        assert!(json.contains("\"informationUri\""), "informationUri field");
        assert!(
            json.contains("\"shortDescription\""),
            "shortDescription field"
        );
        assert!(
            json.contains("\"defaultConfiguration\""),
            "defaultConfiguration field"
        );
    }

    #[test]
    fn schema_and_version_present_in_json() {
        let report = AuditReport::default();
        let json = report_to_sarif_string(&report, "src/lib.rs", "2.5.0", &[]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["$schema"], SARIF_SCHEMA);
        assert_eq!(v["version"], "2.1.0");
    }
}
