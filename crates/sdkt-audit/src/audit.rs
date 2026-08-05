//! Audit engine: rule trait, AST scanner, and entry points.

use std::collections::{HashMap, HashSet};

use sdkt_wasm::ContractSpec;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, FnArg, Item, Local, Pat};

use crate::error::AuditError;
use crate::types::{AuditReport, Severity};

/// Context passed to every rule. `spec` is `Some` when an ABI is available,
/// allowing rules to cross-check source against the declared `ContractSpec`.
pub struct AuditContext<'a> {
    pub spec: Option<&'a ContractSpec>,
}

/// A single static-analysis rule.
///
/// Rules are pure: they receive a pre-scanned view of the program
/// (`&[FnScan]`) plus optional ABI context, and push findings into the report.
pub trait AuditRule {
    /// Stable identifier, e.g. `AUTH-001`. Used by `--disable`.
    fn id(&self) -> &'static str;
    /// Severity this rule emits at.
    fn severity(&self) -> Severity;
    /// Human-readable description of what the rule checks.
    fn description(&self) -> &'static str;
    /// Run the rule over the scanned functions and record findings.
    fn check(&self, scans: &[FnScan], ctx: &AuditContext, report: &mut AuditReport);
}

/// Per-function scan result.
#[derive(Debug, Clone)]
pub struct FnScan {
    pub fn_name: String,
    pub require_auth: usize,
    pub invoke_contract: usize,
    /// Local bindings (let-bindings + parameters) eligible for move tracking.
    pub bound: HashSet<String>,
    /// Argument-usage count per bound local (move heuristic signal).
    pub usage: HashMap<String, usize>,
}

impl FnScan {
    fn new(fn_name: String) -> Self {
        Self {
            fn_name,
            require_auth: 0,
            invoke_contract: 0,
            bound: HashSet::new(),
            usage: HashMap::new(),
        }
    }
}

/// Returns true for `require_auth` and its variants.
fn is_auth_fn(name: &str) -> bool {
    matches!(
        name,
        "require_auth" | "require_auth_for_args" | "require_auth_for_caller"
    )
}

/// Heuristic: does this function name look privileged/admin?
pub(crate) fn is_privileged(name: &str) -> bool {
    let n = unqualified(name).to_lowercase();
    n == "initialize"
        || n.starts_with("initialize_")
        || n == "init"
        || n.starts_with("init_")
        || [
            "admin",
            "mint",
            "burn",
            "pause",
            "unpause",
            "upgrade",
            "withdraw",
            "configure",
            "freeze",
            "ban",
            "set_admin",
            "owner_set",
            "transfer_ownership",
            "change_owner",
            "set_auth",
        ]
        .iter()
        .any(|p| n.contains(p))
}

/// Heuristic: does this name denote an initialize-style entrypoint?
pub(crate) fn is_initialize(name: &str) -> bool {
    let n = unqualified(name).to_lowercase();
    n == "initialize" || n.starts_with("initialize_") || n == "init" || n.starts_with("init_")
}

/// Strip a `Type::` prefix so heuristics match the bare method name.
fn unqualified(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// `syn` visitor that walks a function body and records auth/invoke calls and
/// argument usages of local bindings.
struct FnVisitor<'a> {
    scan: &'a mut FnScan,
}

impl<'ast, 'a> Visit<'ast> for FnVisitor<'a> {
    fn visit_local(&mut self, node: &'ast Local) {
        if let Pat::Ident(pat) = &node.pat {
            self.scan.bound.insert(pat.ident.to_string());
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(p) = &*node.func {
            if let Some(seg) = p.path.segments.last() {
                let name = seg.ident.to_string();
                if is_auth_fn(&name) {
                    self.scan.require_auth += 1;
                }
                if name == "invoke_contract" {
                    self.scan.invoke_contract += 1;
                }
            }
        }
        for arg in &node.args {
            self.count_ident(arg);
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if is_auth_fn(&method) {
            self.scan.require_auth += 1;
        }
        if method == "invoke_contract" {
            self.scan.invoke_contract += 1;
        }
        self.count_ident(&node.receiver);
        for arg in &node.args {
            self.count_ident(arg);
        }
        visit::visit_expr_method_call(self, node);
    }
}

impl<'a> FnVisitor<'a> {
    fn count_ident(&mut self, expr: &Expr) {
        if let Expr::Path(p) = expr {
            if let Some(ident) = p.path.get_ident() {
                let s = ident.to_string();
                if self.scan.bound.contains(&s) {
                    *self.scan.usage.entry(s).or_insert(0) += 1;
                }
            }
        }
    }
}

/// Scan every function in the AST into a `FnScan` per function.
///
/// Covers both top-level `fn` items and methods inside `impl` blocks
/// (Soroban contract entrypoints are `impl` methods).
pub fn scan_all_functions(ast: &syn::File) -> Vec<FnScan> {
    let mut out = Vec::new();
    for item in &ast.items {
        match item {
            Item::Fn(f) => {
                let scan = fn_scan(&f.sig, &f.block);
                out.push(scan);
            }
            Item::Impl(imp) => {
                // Best-effort type name for context in locations.
                let type_name = match &*imp.self_ty {
                    syn::Type::Path(p) => p
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                for inner in &imp.items {
                    if let syn::ImplItem::Fn(m) = inner {
                        let qual = if type_name.is_empty() {
                            m.sig.ident.to_string()
                        } else {
                            format!("{}::{}", type_name, m.sig.ident)
                        };
                        let mut scan = fn_scan(&m.sig, &m.block);
                        scan.fn_name = qual;
                        out.push(scan);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Build a `FnScan` for one function signature + body.
fn fn_scan(sig: &syn::Signature, block: &syn::Block) -> FnScan {
    let mut scan = FnScan::new(sig.ident.to_string());
    for input in &sig.inputs {
        if let FnArg::Typed(pt) = input {
            if let Pat::Ident(pat) = &*pt.pat {
                scan.bound.insert(pat.ident.to_string());
            }
        }
    }
    let mut visitor = FnVisitor { scan: &mut scan };
    visitor.visit_block(block);
    scan
}

/// Scan source directly from a `&str` (parsing errors yield `None`).
/// Convenience wrapper around [`scan_all_functions`] for plugin authors who
/// receive raw source rather than an already-parsed AST.
pub fn scan_all_functions_str(src: &str) -> Option<Vec<FnScan>> {
    let ast = syn::parse_file(src).ok()?;
    Some(scan_all_functions(&ast))
}

/// All built-in rules.
pub fn all_rules() -> Vec<Box<dyn AuditRule>> {
    vec![
        Box::new(crate::rules::Auth001),
        Box::new(crate::rules::Auth002),
        Box::new(crate::rules::Auth003),
        Box::new(crate::rules::Move001),
    ]
}

fn run_rules(
    ast: &syn::File,
    ctx: &AuditContext,
    disabled: &[&str],
) -> Result<AuditReport, AuditError> {
    let scans = scan_all_functions(ast);
    let mut report = AuditReport::default();
    // Execute rules through the registry (built-ins + any linked plugins),
    // preserving registration order so output stays identical to M16.
    crate::registry::run_registered(&scans, ctx, disabled, &mut report);
    Ok(report)
}

/// Audit Rust source with no ABI context.
pub fn audit_source(src: &str) -> Result<AuditReport, AuditError> {
    audit_source_with(src, &[])
}

/// Audit Rust source, skipping any rule whose id is in `disabled`.
pub fn audit_source_with(src: &str, disabled: &[&str]) -> Result<AuditReport, AuditError> {
    let ast = syn::parse_file(src).map_err(AuditError::Parse)?;
    let ctx = AuditContext { spec: None };
    run_rules(&ast, &ctx, disabled)
}

/// Audit Rust source with an accompanying `ContractSpec` for cross-checking.
pub fn audit_source_with_spec(
    src: &str,
    spec: &ContractSpec,
    disabled: &[&str],
) -> Result<AuditReport, AuditError> {
    let ast = syn::parse_file(src).map_err(AuditError::Parse)?;
    let ctx = AuditContext { spec: Some(spec) };
    run_rules(&ast, &ctx, disabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Auth001, Auth002, Auth003, Move001};

    fn report_for(src: &str) -> AuditReport {
        audit_source(src).unwrap()
    }

    fn has(rep: &AuditReport, id: &str) -> bool {
        rep.findings.iter().any(|f| f.rule_id == id)
    }

    #[test]
    fn auth001_fires_on_privileged_without_auth() {
        let src = "pub fn mint_token(to: Address) { /* no auth */ }";
        let rep = report_for(src);
        assert!(has(&rep, "AUTH-001"));
    }

    #[test]
    fn auth001_silent_when_auth_present() {
        let src = "pub fn mint_token(to: Address) { require_auth(); }";
        let rep = report_for(src);
        assert!(!has(&rep, "AUTH-001"));
    }

    #[test]
    fn auth001_disabled_is_silent() {
        let src = "pub fn mint_token(to: Address) { }";
        let rep = audit_source_with(src, &["AUTH-001"]).unwrap();
        assert!(!has(&rep, "AUTH-001"));
    }

    #[test]
    fn auth001_negative_on_normal_fn() {
        let src = "pub fn balance_of(who: Address) -> u32 { 0 }";
        let rep = report_for(src);
        assert!(!has(&rep, "AUTH-001"));
    }

    #[test]
    fn auth002_fires_on_invoke_without_auth() {
        let src = "pub fn relay() { env.invoke_contract(&addr, sym, vec![]); }";
        let rep = report_for(src);
        assert!(has(&rep, "AUTH-002"));
    }

    #[test]
    fn auth002_silent_when_auth_present() {
        let src = "pub fn relay() { require_auth(); env.invoke_contract(&addr, sym, vec![]); }";
        let rep = report_for(src);
        assert!(!has(&rep, "AUTH-002"));
    }

    #[test]
    fn auth003_fires_on_unguarded_initialize() {
        let src = "pub fn initialize(admin: Address) { /* no auth */ }";
        let rep = report_for(src);
        assert!(has(&rep, "AUTH-003"));
    }

    #[test]
    fn auth003_silent_when_initialize_has_auth() {
        let src = "pub fn initialize(admin: Address) { require_auth(); }";
        let rep = report_for(src);
        assert!(!has(&rep, "AUTH-003"));
    }

    #[test]
    fn move001_fires_on_double_arg_use() {
        let src = "pub fn foo(a: Address) { bar(a); baz(a); }";
        let rep = report_for(src);
        assert!(has(&rep, "MOVE-001"));
    }

    #[test]
    fn move001_silent_on_single_use() {
        let src = "pub fn foo(a: Address) { bar(a); }";
        let rep = report_for(src);
        assert!(!has(&rep, "MOVE-001"));
    }

    #[test]
    fn clean_source_reports_no_findings() {
        let src = "pub fn hello(name: String) -> String { require_auth(); hello_helper(name) }";
        let rep = report_for(src);
        assert!(rep.is_clean());
        assert_eq!(rep.summary.total, 0);
    }

    #[test]
    fn parse_error_is_reported() {
        let res = audit_source("this is not valid rust @@@");
        assert!(matches!(res, Err(AuditError::Parse(_))));
    }

    // Compile-time guarantee that the rule structs implement the trait.
    #[test]
    fn rules_implement_trait() {
        fn assert_rule(_r: &dyn AuditRule) {}
        assert_rule(&Auth001);
        assert_rule(&Auth002);
        assert_rule(&Auth003);
        assert_rule(&Move001);
    }
}
