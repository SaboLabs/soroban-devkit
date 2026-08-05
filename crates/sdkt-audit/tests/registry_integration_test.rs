//! Integration tests for the `sdkt-audit` rule registry and audit pipeline.
//!
//! These assert that:
//! - built-in rules execute through the registry and behave identically to M16,
//! - external rules registered into the registry also run,
//! - the `--disable` mechanism still works end-to-end.

use sdkt_audit::{
    audit_source_with, registry::RuleRegistry, AuditContext, AuditReport, AuditRule, BoxedRule,
    Finding, FnScan, Severity,
};

struct Probe {
    id: &'static str,
}

impl AuditRule for Probe {
    fn id(&self) -> &'static str {
        self.id
    }
    fn severity(&self) -> Severity {
        Severity::Info
    }
    fn description(&self) -> &'static str {
        "probe rule"
    }
    fn check(&self, _s: &[FnScan], _c: &AuditContext, r: &mut AuditReport) {
        r.add(Finding {
            rule_id: self.id.to_string(),
            severity: Severity::Info,
            message: "probe fired".into(),
            location: None,
        });
    }
}

#[test]
fn builtins_unchanged_through_registry() {
    // A privileged function with no auth must still surface AUTH-001.
    let src = "pub fn mint_token(to: Address) { /* no auth */ }\n";
    let rep = audit_source_with(src, &[]).unwrap();
    assert!(rep.findings.iter().any(|f| f.rule_id == "AUTH-001"));
    // And an initialize without auth must surface AUTH-003.
    let src2 = "pub fn initialize(admin: Address) { }\n";
    let rep2 = audit_source_with(src2, &[]).unwrap();
    assert!(rep2.findings.iter().any(|f| f.rule_id == "AUTH-003"));
}

#[test]
fn disable_still_suppresses_builtin() {
    let src = "pub fn mint_token(to: Address) { }\n";
    let rep = audit_source_with(src, &["AUTH-001"]).unwrap();
    assert!(!rep.findings.iter().any(|f| f.rule_id == "AUTH-001"));
}

#[test]
fn registry_executes_builtins_and_external_rule() {
    let mut reg = RuleRegistry::new();
    reg.register_builtin_rules();
    reg.register_rule(Box::new(Probe { id: "PROBE-1" }) as BoxedRule);
    assert_eq!(reg.registered_rules().len(), 5);

    // Feed a source that triggers AUTH-001 (builtin) plus run the probe.
    let scans = sdkt_audit::scan_all_functions(
        &syn::parse_file("pub fn mint_token(to: Address) { }").unwrap(),
    );
    let ctx = AuditContext { spec: None };
    let mut report = AuditReport::default();
    reg.run_all(&scans, &ctx, &[], &mut report);

    assert!(report.findings.iter().any(|f| f.rule_id == "AUTH-001"));
    assert!(report.findings.iter().any(|f| f.rule_id == "PROBE-1"));
}

#[test]
fn registry_respects_disabled_in_run_all() {
    let mut reg = RuleRegistry::new();
    reg.register_rule(Box::new(Probe { id: "PROBE-1" }) as BoxedRule);
    let scans: Vec<FnScan> = Vec::new();
    let ctx = AuditContext { spec: None };
    let mut report = AuditReport::default();
    reg.run_all(&scans, &ctx, &["PROBE-1"], &mut report);
    assert!(report.findings.is_empty());
}
