//! Built-in audit rules. Each implements [`crate::audit::AuditRule`].

use crate::audit::{AuditContext, AuditRule, FnScan};
use crate::types::{AuditReport, Finding, Severity};

/// AUTH-001 — Missing `require_auth()` on a privileged/admin function.
pub struct Auth001;

impl AuditRule for Auth001 {
    fn id(&self) -> &'static str {
        "AUTH-001"
    }
    fn severity(&self) -> Severity {
        Severity::Critical
    }
    fn description(&self) -> &'static str {
        "Privileged/admin function must call require_auth()"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            if crate::audit::is_privileged(&s.fn_name) && s.require_auth == 0 {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!(
                        "Function `{}` looks privileged but does not call require_auth()",
                        s.fn_name
                    ),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

/// AUTH-002 — `invoke_contract()` without `require_auth()` in the same function.
pub struct Auth002;

impl AuditRule for Auth002 {
    fn id(&self) -> &'static str {
        "AUTH-002"
    }
    fn severity(&self) -> Severity {
        Severity::Critical
    }
    fn description(&self) -> &'static str {
        "Cross-contract invoke_contract() must be guarded by require_auth()"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            if s.invoke_contract > 0 && s.require_auth == 0 {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!(
                        "Function `{}` calls invoke_contract() without require_auth()",
                        s.fn_name
                    ),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

/// AUTH-003 — Unguarded `initialize()` entrypoint.
pub struct Auth003;

impl AuditRule for Auth003 {
    fn id(&self) -> &'static str {
        "AUTH-003"
    }
    fn severity(&self) -> Severity {
        Severity::Critical
    }
    fn description(&self) -> &'static str {
        "initialize()-style entrypoint must call require_auth()"
    }
    fn check(&self, scans: &[FnScan], ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            let is_init = crate::audit::is_initialize(&s.fn_name);
            if !is_init || s.require_auth > 0 {
                continue;
            }
            // When an ABI is available, only flag if the function is actually
            // exported (reuses sdkt-wasm ContractSpec). Without a spec we fall
            // back to the name heuristic.
            let exported = match ctx.spec {
                Some(spec) => spec
                    .functions
                    .iter()
                    .any(|f| f.name.to_lowercase() == s.fn_name.to_lowercase()),
                None => true,
            };
            if exported {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!(
                        "initialize-style function `{}` has no require_auth() guard",
                        s.fn_name
                    ),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

/// AUTH-004 — Token transfer without `require_auth()` on source/destination.
///
/// Detects functions that invoke `transfer` or `transfer_from` on a token
/// client without a preceding `require_auth()` call in the same function
/// scope — a common vulnerability pattern in Soroban token implementations
/// where the caller's authority over the source funds is never verified.
pub struct Auth004;

impl AuditRule for Auth004 {
    fn id(&self) -> &'static str {
        "AUTH-004"
    }
    fn severity(&self) -> Severity {
        Severity::Critical
    }
    fn description(&self) -> &'static str {
        "Token transfer must be guarded by require_auth()"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            if s.token_transfer > 0 && s.require_auth == 0 {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!(
                        "Function `{}` performs token transfer without require_auth()",
                        s.fn_name
                    ),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

/// MOVE-001 — Suspicious move-after-use heuristic (Warning only).
pub struct Move001;

impl AuditRule for Move001 {
    fn id(&self) -> &'static str {
        "MOVE-001"
    }
    fn severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "Local used as a call argument multiple times (possible move-after-use)"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            for (name, count) in &s.usage {
                if *count >= 2 {
                    report.add(Finding {
                        rule_id: self.id().to_string(),
                        severity: self.severity(),
                        message: format!(
                            "Local `{}` in `{}` is used as a call argument {} times — possible move-after-use",
                            name, s.fn_name, count
                        ),
                        location: Some(format!("{}:{}", s.fn_name, name)),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_for(src: &str) -> AuditReport {
        let ast = syn::parse_file(src).unwrap();
        let scans = crate::audit::scan_all_functions(&ast);
        let ctx = AuditContext { spec: None };
        let mut report = AuditReport::default();
        let rules: Vec<Box<dyn AuditRule>> = vec![
            Box::new(Auth001),
            Box::new(Auth002),
            Box::new(Auth003),
            Box::new(Auth004),
            Box::new(Move001),
        ];
        for rule in &rules {
            rule.check(&scans, &ctx, &mut report);
        }
        report
    }

    fn has(rep: &AuditReport, id: &str) -> bool {
        rep.findings.iter().any(|f| f.rule_id == id)
    }

    #[test]
    fn auth004_fires_on_transfer_without_auth() {
        let src = "pub fn move_tokens(from: Address, to: Address, amount: i128) { token.transfer(&from, &to, &amount); }";
        let rep = report_for(src);
        assert!(
            has(&rep, "AUTH-004"),
            "transfer without auth should trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_fires_on_transfer_from_without_auth() {
        let src = "pub fn move_tokens(from: Address, to: Address, amount: i128) { token.transfer_from(&from, &to, &amount); }";
        let rep = report_for(src);
        assert!(
            has(&rep, "AUTH-004"),
            "transfer_from without auth should trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_silent_when_auth_present() {
        let src = "pub fn move_tokens(from: Address, to: Address, amount: i128) { require_auth(); token.transfer(&from, &to, &amount); }";
        let rep = report_for(src);
        assert!(
            !has(&rep, "AUTH-004"),
            "transfer with auth should not trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_silent_on_non_transfer_fn() {
        let src = "pub fn balance_of(who: Address) -> u32 { 0 }";
        let rep = report_for(src);
        assert!(
            !has(&rep, "AUTH-004"),
            "non-transfer function should not trigger AUTH-004"
        );
    }
}
