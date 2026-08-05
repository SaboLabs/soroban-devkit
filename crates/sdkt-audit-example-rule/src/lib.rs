//! Example `sdkt-audit` plugin rule — reference implementation only.
//!
//! This crate demonstrates the plugin author workflow:
//!   1. Implement [`sdkt_audit::AuditRule`].
//!   2. Register the rule into the process-wide registry via
//!      [`sdkt_audit::register_rule`] (or the `register_rule!` macro).
//!   3. Produce a [`sdkt_audit::Finding`] when your condition holds.
//!
//! It is compiled in only when `sdkt-cli` is built with the `plugins` feature,
//! so default builds behave exactly like M16. See `docs/plugin-authoring.md`.

#[cfg(not(target_arch = "wasm32"))]
use sdkt_audit::{
    register_rule, AuditContext, AuditReport, AuditRule, BoxedRule, Finding, FnScan, Severity,
};

/// EXAMPLE-001 — flags any function whose name contains `sdkt_example_trigger`.
///
/// Deliberately contrived so it never interferes with real audits or the
/// built-in rules' outputs.
#[cfg(not(target_arch = "wasm32"))]
pub struct ExampleRule;

#[cfg(not(target_arch = "wasm32"))]
impl AuditRule for ExampleRule {
    fn id(&self) -> &'static str {
        "EXAMPLE-001"
    }
    fn severity(&self) -> Severity {
        Severity::Info
    }
    fn description(&self) -> &'static str {
        "Example rule: detects functions named sdkt_example_trigger"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            if s.fn_name.contains("sdkt_example_trigger") {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!("Example rule matched trigger function `{}`", s.fn_name),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

/// Register this plugin's rule into the global `sdkt-audit` registry.
#[cfg(not(target_arch = "wasm32"))]
pub fn register() {
    register_rule(Box::new(ExampleRule) as BoxedRule);
}

/// C-ABI exports for dynamic loading (M18, Phase B). Compiled only with the
/// `plugins` feature — produces a loadable shared library artifact.
#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
mod plugin_abi;

/// WASM JSON-ABI exports for sandboxed dynamic loading (M19, Phase C).
#[cfg(all(feature = "wasm-plugins", target_arch = "wasm32"))]
mod plugin_abi_wasm;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn example_rule_fires_on_trigger_function() {
        let scans = vec![FnScan {
            fn_name: "sdkt_example_trigger_admin".to_string(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let ctx = AuditContext { spec: None };
        let mut report = AuditReport::default();
        ExampleRule.check(&scans, &ctx, &mut report);
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.findings[0].rule_id, "EXAMPLE-001");
    }

    #[test]
    fn example_rule_silent_on_normal_function() {
        let scans = vec![FnScan {
            fn_name: "balance_of".to_string(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let ctx = AuditContext { spec: None };
        let mut report = AuditReport::default();
        ExampleRule.check(&scans, &ctx, &mut report);
        assert!(report.is_clean());
    }
}
