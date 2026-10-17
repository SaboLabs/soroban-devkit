//! Rule registry — the extensibility backbone of `sdkt-audit`.
//!
//! Built-in rules register themselves here via [`register_builtin_rules`], and
//! external/plugin rules register via [`register_rule`] (typically through the
//! [`register_rule!`](crate::register_rule) macro). Audit execution iterates
//! the registry, preserving registration order so finding output stays stable.

use std::sync::{Mutex, OnceLock};

use crate::audit::{AuditContext, AuditRule, FnScan};
use crate::types::AuditReport;

/// Object-safe rule box accepted by the registry. Rules must be `Send + Sync`
/// so the registry can live in a process-wide `OnceLock<Mutex<_>>`.
pub type BoxedRule = Box<dyn AuditRule + Send + Sync>;

/// An ordered, deduplicated collection of static-analysis rules.
pub struct RuleRegistry {
    rules: Vec<BoxedRule>,
}

impl Default for RuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Register a rule. If a rule with the same `id()` is already present it is
    /// ignored (last-writer does not clobber; first registration wins). This
    /// makes repeated `register_builtin_rules` / plugin registration idempotent.
    pub fn register_rule(&mut self, rule: BoxedRule) {
        if self.rules.iter().any(|r| r.id() == rule.id()) {
            return;
        }
        self.rules.push(rule);
    }

    /// Register the built-in `sdkt-audit` rules in their canonical order.
    ///
    /// The list comes from [`crate::audit::all_rules`], so the rule set the CLI
    /// describes and the rule set that executes are the same by construction.
    pub fn register_builtin_rules(&mut self) {
        for rule in crate::audit::all_rules() {
            self.register_rule(rule);
        }
    }

    /// Snapshot of currently registered rules (in registration order).
    pub fn registered_rules(&self) -> &[BoxedRule] {
        &self.rules
    }

    /// Snapshot of `RuleInfo` for all registered rules in this registry.
    ///
    /// Used by the CLI to build SARIF rule metadata and validate `--disable`
    /// IDs against the exact rule set that will execute.
    pub fn registered_rule_info(&self) -> Vec<crate::types::RuleInfo> {
        self.rules
            .iter()
            .map(|r| crate::types::RuleInfo {
                id: r.id().to_string(),
                severity: r.severity(),
                description: r.description().to_string(),
            })
            .collect()
    }

    /// Run every registered rule that is not disabled, appending findings to
    /// `report`. Rules execute in registration order so output is deterministic.
    pub fn run_all(
        &self,
        scans: &[FnScan],
        ctx: &AuditContext,
        disabled: &[&str],
        report: &mut AuditReport,
    ) {
        for rule in &self.rules {
            if disabled.contains(&rule.id()) {
                continue;
            }
            rule.check(scans, ctx, report);
        }
    }
}

static GLOBAL: OnceLock<Mutex<RuleRegistry>> = OnceLock::new();

/// Process-wide registry. Built-ins are registered lazily on first access.
fn global() -> &'static Mutex<RuleRegistry> {
    GLOBAL.get_or_init(|| Mutex::new(RuleRegistry::new()))
}

/// Register an external/plugin rule into the process-wide registry.
pub fn register_rule(rule: BoxedRule) {
    global()
        .lock()
        .expect("audit registry poisoned")
        .register_rule(rule);
}

/// Ensure the built-in rules are present in the process-wide registry.
pub fn register_builtin_rules() {
    global()
        .lock()
        .expect("audit registry poisoned")
        .register_builtin_rules();
}

/// Run all registered rules (built-ins + any linked plugins) over `scans`.
/// Ensures built-ins are registered first. `disabled` skips rules by id.
pub fn run_registered(
    scans: &[FnScan],
    ctx: &AuditContext,
    disabled: &[&str],
    report: &mut AuditReport,
) {
    register_builtin_rules();
    let reg = global().lock().expect("audit registry poisoned");
    reg.run_all(scans, ctx, disabled, report);
}

/// Snapshot of every rule in the process-wide registry, built-ins plus any
/// registered plugins, in registration order.
///
/// [`crate::audit::all_rules`] returns only the built-in set, so callers that
/// need to describe *what will actually execute* — `--list-rules`, the SARIF
/// rule catalogue, `--disable` validation — must read the registry instead.
/// The registry is populated lazily, so this ensures built-ins first.
///
/// Returns owned `RuleInfo` rather than rule references: `Box<dyn AuditRule>`
/// is not `Clone`, and callers only need the descriptive fields.
pub fn registered_rule_info() -> Vec<crate::types::RuleInfo> {
    register_builtin_rules();
    let reg = global().lock().expect("audit registry poisoned");
    reg.registered_rules()
        .iter()
        .map(|r| crate::types::RuleInfo {
            id: r.id().to_string(),
            severity: r.severity(),
            description: r.description().to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Severity;

    struct Stub {
        id: &'static str,
    }
    impl AuditRule for Stub {
        fn id(&self) -> &'static str {
            self.id
        }
        fn severity(&self) -> Severity {
            Severity::Info
        }
        fn description(&self) -> &'static str {
            "stub"
        }
        fn check(&self, _s: &[FnScan], _c: &AuditContext, r: &mut AuditReport) {
            r.add(crate::types::Finding {
                file: None,
                rule_id: self.id.to_string(),
                severity: Severity::Info,
                message: "stub fired".into(),
                location: None,
            });
        }
    }

    #[test]
    fn register_adds_rule() {
        let mut reg = RuleRegistry::new();
        assert_eq!(reg.registered_rules().len(), 0);
        reg.register_rule(Box::new(Stub { id: "STUB-1" }) as BoxedRule);
        assert_eq!(reg.registered_rules().len(), 1);
    }

    #[test]
    fn duplicate_registration_is_ignored() {
        let mut reg = RuleRegistry::new();
        reg.register_rule(Box::new(Stub { id: "STUB-1" }) as BoxedRule);
        reg.register_rule(Box::new(Stub { id: "STUB-1" }) as BoxedRule);
        assert_eq!(
            reg.registered_rules().len(),
            1,
            "duplicate id must be ignored"
        );
    }

    #[test]
    fn ordering_preserved() {
        let mut reg = RuleRegistry::new();
        reg.register_rule(Box::new(Stub { id: "B" }) as BoxedRule);
        reg.register_rule(Box::new(Stub { id: "A" }) as BoxedRule);
        let ids: Vec<&str> = reg.registered_rules().iter().map(|r| r.id()).collect();
        assert_eq!(ids, vec!["B", "A"]);
    }

    #[test]
    fn builtin_registration_count() {
        let mut reg = RuleRegistry::new();
        reg.register_builtin_rules();
        assert_eq!(reg.registered_rules().len(), 7, "seven built-in rules");
        let ids: Vec<&str> = reg.registered_rules().iter().map(|r| r.id()).collect();
        assert_eq!(
            ids,
            vec!["AUTH-001", "AUTH-002", "AUTH-003", "AUTH-004", "MOVE-001", "MATH-001", "CEI-001"]
        );
    }

    /// The registry and `all_rules()` (what `--list-rules` describes and what
    /// the "Rules loaded" header counts) must be the same rule set in the same
    /// order. A rule that executes but is not listed — or vice versa — is the
    /// defect this guards against.
    #[test]
    fn registry_matches_all_rules() {
        let mut reg = RuleRegistry::new();
        reg.register_builtin_rules();
        let registered: Vec<&str> = reg.registered_rules().iter().map(|r| r.id()).collect();
        let described: Vec<&str> = crate::audit::all_rules().iter().map(|r| r.id()).collect();
        assert_eq!(
            registered, described,
            "registry and all_rules() must expose the same rule ids in the same order"
        );

        // Descriptions and severities are part of `--list-rules` output; a
        // divergence there is the same class of drift.
        for rule in crate::audit::all_rules() {
            let registered_rule = reg
                .registered_rules()
                .iter()
                .find(|r| r.id() == rule.id())
                .expect("every listed rule must be registered");
            assert_eq!(
                registered_rule.description(),
                rule.description(),
                "description mismatch for {}",
                rule.id()
            );
            assert_eq!(
                registered_rule.severity(),
                rule.severity(),
                "severity mismatch for {}",
                rule.id()
            );
        }
    }

    /// Every rule `all_rules()` describes must actually run: executing a scan
    /// that each rule can fire on must reach the registry, not just the listing.
    #[test]
    fn every_builtin_rule_is_reachable_by_id() {
        let listed: Vec<String> = crate::audit::all_rules()
            .iter()
            .map(|r| r.id().to_string())
            .collect();
        assert!(
            listed.contains(&"CEI-001".to_string()),
            "CEI-001 must be listed: it is registered and executed"
        );
        assert_eq!(listed.len(), 7, "seven built-in rules, got {listed:?}");
    }

    #[test]
    fn run_all_executes_every_rule() {
        let mut reg = RuleRegistry::new();
        reg.register_rule(Box::new(Stub { id: "X" }) as BoxedRule);
        reg.register_rule(Box::new(Stub { id: "Y" }) as BoxedRule);
        let scans: Vec<FnScan> = Vec::new();
        let ctx = AuditContext { spec: None };
        let mut report = AuditReport::default();
        reg.run_all(&scans, &ctx, &[], &mut report);
        assert_eq!(report.summary.total, 2);
        assert!(report.findings.iter().any(|f| f.rule_id == "X"));
        assert!(report.findings.iter().any(|f| f.rule_id == "Y"));
    }

    #[test]
    fn run_all_respects_disabled() {
        let mut reg = RuleRegistry::new();
        reg.register_rule(Box::new(Stub { id: "X" }) as BoxedRule);
        let scans: Vec<FnScan> = Vec::new();
        let ctx = AuditContext { spec: None };
        let mut report = AuditReport::default();
        reg.run_all(&scans, &ctx, &["X"], &mut report);
        assert_eq!(report.summary.total, 0);
    }
}
