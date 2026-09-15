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
pub struct Auth004;

impl AuditRule for Auth004 {
    fn id(&self) -> &'static str {
        "AUTH-004"
    }
    fn severity(&self) -> Severity {
        Severity::Critical
    }
    fn description(&self) -> &'static str {
        "Token transfer function must call require_auth() on source or destination"
    }
    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            let name = s.fn_name.to_lowercase();
            let is_transfer = matches!(
                name.as_str(),
                "transfer" | "transfer_from" | "transferfrom" | "withdraw" | "burn"
            );
            if is_transfer && s.require_auth == 0 {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!(
                        "Transfer-style function `{}` does not call require_auth()",
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
    use crate::types::AuditReport;

    fn has(report: &AuditReport, rule_id: &str) -> bool {
        report.findings.iter().any(|f| f.rule_id == rule_id)
    }

    #[test]
    fn auth001_flags_mint_without_auth() {
        let scans = vec![FnScan {
            fn_name: "mint_token".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-001"),
            "mint_token without auth should trigger AUTH-001"
        );
    }

    #[test]
    fn auth001_flags_initialize_without_auth() {
        let scans = vec![FnScan {
            fn_name: "initialize".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-001"),
            "initialize without auth should trigger AUTH-001"
        );
    }

    #[test]
    fn auth001_does_not_flag_non_privileged() {
        let scans = vec![FnScan {
            fn_name: "balance_of".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-001"),
            "balance_of should not trigger AUTH-001"
        );
    }

    #[test]
    fn auth001_does_not_flag_privileged_with_auth() {
        let scans = vec![FnScan {
            fn_name: "mint_token".into(),
            require_auth: 1,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-001"),
            "mint_token with auth should not trigger AUTH-001"
        );
    }

    #[test]
    fn auth002_flags_invoke_contract_without_auth() {
        let scans = vec![FnScan {
            fn_name: "cross_call".into(),
            require_auth: 0,
            invoke_contract: 1,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth002.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-002"),
            "invoke_contract without auth should trigger AUTH-002"
        );
    }

    #[test]
    fn auth002_does_not_flag_invoke_contract_with_auth() {
        let scans = vec![FnScan {
            fn_name: "cross_call".into(),
            require_auth: 1,
            invoke_contract: 1,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth002.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-002"),
            "invoke_contract with auth should not trigger AUTH-002"
        );
    }

    #[test]
    fn auth002_does_not_flag_without_invoke_contract() {
        let scans = vec![FnScan {
            fn_name: "local_call".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth002.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-002"),
            "no invoke_contract should not trigger AUTH-002"
        );
    }

    #[test]
    fn auth003_flags_initialize_without_auth() {
        let scans = vec![FnScan {
            fn_name: "initialize".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth003.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-003"),
            "initialize without auth should trigger AUTH-003"
        );
    }

    #[test]
    fn auth003_flags_init_token_without_auth() {
        let scans = vec![FnScan {
            fn_name: "init_token".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth003.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-003"),
            "init_token without auth should trigger AUTH-003"
        );
    }

    #[test]
    fn auth003_does_not_flag_initialize_with_auth() {
        let scans = vec![FnScan {
            fn_name: "initialize".into(),
            require_auth: 1,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth003.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-003"),
            "initialize with auth should not trigger AUTH-003"
        );
    }

    #[test]
    fn auth003_does_not_flag_non_initialize() {
        let scans = vec![FnScan {
            fn_name: "transfer".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth003.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-003"),
            "non-initialize function should not trigger AUTH-003"
        );
    }

    #[test]
    fn move001_flags_reused_local() {
        use std::collections::HashMap;
        let mut usage = HashMap::new();
        usage.insert("amount".to_string(), 2);
        let scans = vec![FnScan {
            fn_name: "do_swap".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage,
        }];
        let mut rep = AuditReport::default();
        Move001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "MOVE-001"),
            "local used 2x as arg should trigger MOVE-001"
        );
    }

    #[test]
    fn move001_does_not_flag_single_use() {
        use std::collections::HashMap;
        let mut usage = HashMap::new();
        usage.insert("amount".to_string(), 1);
        let scans = vec![FnScan {
            fn_name: "do_swap".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage,
        }];
        let mut rep = AuditReport::default();
        Move001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "MOVE-001"),
            "local used once should not trigger MOVE-001"
        );
    }

    #[test]
    fn move001_does_not_flag_no_usage() {
        let scans = vec![FnScan {
            fn_name: "do_nothing".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Move001.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "MOVE-001"),
            "no usage should not trigger MOVE-001"
        );
    }
    #[test]
    fn auth004_flags_transfer_from_without_auth() {
        let scans = vec![FnScan {
            fn_name: "transfer_from".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth004.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            has(&rep, "AUTH-004"),
            "transfer_from without auth should trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_does_not_flag_transfer_with_auth() {
        let scans = vec![FnScan {
            fn_name: "transfer".into(),
            require_auth: 1,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth004.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-004"),
            "transfer with auth should not trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_does_not_flag_non_transfer() {
        let scans = vec![FnScan {
            fn_name: "balance_of".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth004.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-004"),
            "non-transfer function should not trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_does_not_flag_transfer_ownership() {
 // `transfer_ownership` is an admin-management function, not a token
        // transfer — it must not trigger AUTH-004.
        let scans = vec![FnScan {
            fn_name: "transfer_ownership".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth004.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-004"),
            "transfer_ownership should not trigger AUTH-004"
        );
    }

    #[test]
    fn auth004_does_not_flag_admin_transfer() {
        let scans = vec![FnScan {
            fn_name: "admin_transfer".into(),
            require_auth: 0,
            invoke_contract: 0,
            bound: Default::default(),
            usage: Default::default(),
        }];
        let mut rep = AuditReport::default();
        Auth004.check(&scans, &crate::audit::AuditContext { spec: None }, &mut rep);
        assert!(
            !has(&rep, "AUTH-004"),
            "admin_transfer should not trigger AUTH-004"
        );
    }
}
