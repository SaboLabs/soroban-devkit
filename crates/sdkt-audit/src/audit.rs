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

/// Event types for CEI (Checks-Effects-Interactions) ordering tracking.
///
/// CEI-001 tracks these events in source order within a function to detect
/// reentrancy-precondition hazards: when an external contract invocation
/// appears before a state write in the same function.
///
/// See [`crate::rules::Cei001`] for heuristic scope and recognized patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderEvent {
    /// External contract invocation (invoke_contract, invoke_contract_light, etc.)
    ExternalCall,
    /// State write operation (storage().set, storage().write, etc.)
    StateWrite,
}

/// Per-function scan result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FnScan {
    pub fn_name: String,
    pub require_auth: usize,
    pub invoke_contract: usize,
    /// Local bindings (let-bindings + parameters) eligible for move tracking.
    pub bound: HashSet<String>,
    /// Argument-usage count per bound local (move heuristic signal).
    pub usage: HashMap<String, usize>,
    /// True when a division result is subsequently multiplied in this function.
    #[serde(default)]
    pub division_before_multiplication: bool,
    /// True when a recognized external call precedes a recognized state write
    /// in source order within this function (CEI-001 heuristic hazard).
    ///
    /// This is a static-analysis signal only; it does not prove reentrancy or
    /// exploitability. See [`crate::rules::Cei001`] for heuristic scope and limitations.
    #[serde(default)]
    pub external_call_before_state_write: bool,
}

impl FnScan {
    fn new(fn_name: String) -> Self {
        Self {
            fn_name,
            require_auth: 0,
            invoke_contract: 0,
            bound: HashSet::new(),
            usage: HashMap::new(),
            division_before_multiplication: false,
            external_call_before_state_write: false,
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
pub(crate) fn unqualified(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// `syn` visitor that walks a function body and records auth/invoke calls and
/// argument usages of local bindings.
struct FnVisitor<'a> {
    scan: &'a mut FnScan,
    divided_bindings: HashSet<String>,
    /// Ordered sequence of CEI events (external calls and state writes) in source order.
    order_events: &'a mut Vec<OrderEvent>,
}

impl<'ast, 'a> Visit<'ast> for FnVisitor<'a> {
    fn visit_local(&mut self, node: &'ast Local) {
        if let Pat::Ident(pat) = &node.pat {
            let name = pat.ident.to_string();
            self.scan.bound.insert(name.clone());
            if let Some(init) = &node.init {
                if expr_contains_division(&init.expr)
                    || expr_uses_any(&init.expr, &self.divided_bindings)
                {
                    self.divided_bindings.insert(name);
                } else {
                    self.divided_bindings.remove(&name);
                }
            }
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::Mul(_))
            && (expr_contains_division(&node.left)
                || expr_contains_division(&node.right)
                || expr_uses_any(&node.left, &self.divided_bindings)
                || expr_uses_any(&node.right, &self.divided_bindings))
        {
            self.scan.division_before_multiplication = true;
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(p) = &*node.func {
            if let Some(seg) = p.path.segments.last() {
                let name = seg.ident.to_string();
                if is_auth_fn(&name) {
                    self.scan.require_auth += 1;
                }
            }
        }
        // Visit arguments first (to record nested events before this call).
        for arg in &node.args {
            self.count_ident(arg);
            visit::visit_expr(self, arg);
        }
        // Then record this call after its arguments are processed.
        if let Expr::Path(p) = &*node.func {
            if let Some(seg) = p.path.segments.last() {
                let name = seg.ident.to_string();
                if name == "invoke_contract" || name == "invoke_contract_light" {
                    self.scan.invoke_contract += 1;
                    self.order_events.push(OrderEvent::ExternalCall);
                }
            }
        }
        // Don't call visit::visit_expr_call since we've already visited arguments.
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if is_auth_fn(&method) {
            self.scan.require_auth += 1;
        }

        // Visit receiver first.
        visit::visit_expr(self, &node.receiver);
        // Visit arguments.
        for arg in &node.args {
            self.count_ident(arg);
            visit::visit_expr(self, arg);
        }

        // Now record the method call event after nested expressions are processed.
        if method == "invoke_contract" || method == "invoke_contract_light" {
            self.scan.invoke_contract += 1;
            self.order_events.push(OrderEvent::ExternalCall);
        }
        // Detect state write patterns: storage().set(...), storage().write(...), etc.
        self.detect_state_write(&method);
        // Don't call visit::visit_expr_method_call since we've manually visited children.
    }
}

fn expr_contains_division(expr: &Expr) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
            if matches!(node.op, syn::BinOp::Div(_))
                && !(is_constant_expr(&node.left) && is_constant_expr(&node.right))
            {
                self.0 = true;
            }
            visit::visit_expr_binary(self, node);
        }
    }
    let mut finder = Finder(false);
    finder.visit_expr(expr);
    finder.0
}

fn is_constant_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(literal) => matches!(literal.lit, syn::Lit::Int(_)),
        Expr::Paren(paren) => is_constant_expr(&paren.expr),
        Expr::Group(group) => is_constant_expr(&group.expr),
        Expr::Unary(unary) => is_constant_expr(&unary.expr),
        Expr::Binary(binary) => is_constant_expr(&binary.left) && is_constant_expr(&binary.right),
        _ => false,
    }
}

fn expr_uses_any(expr: &Expr, names: &HashSet<String>) -> bool {
    struct Finder<'a> {
        names: &'a HashSet<String>,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            if let Some(name) = node.path.get_ident() {
                self.found |= self.names.contains(&name.to_string());
            }
            visit::visit_expr_path(self, node);
        }
    }
    let mut finder = Finder {
        names,
        found: false,
    };
    finder.visit_expr(expr);
    finder.found
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

    /// Detect state write patterns and record them in the CEI order events.
    ///
    /// Recognized patterns (case-insensitive method names):
    /// - `set` - Direct storage writes
    /// - `write` - Alternative write method
    /// - `put` - Map/collection insertion
    /// - `remove` - Key removal
    /// - `delete` - Deletion operation
    /// - `extend` - Entry extension (e.g., ledger entries)
    ///
    /// Note: This is a heuristic based on method names only. Custom state-mutation
    /// patterns not matching these names will not be detected.
    fn detect_state_write(&mut self, method_name: &str) {
        // Common state write patterns in Soroban:
        // - storage().set(...), storage().write(...)
        // - map.set(...), map.put(...)
        // - set(...), put(...), write(...)
        let patterns = ["set", "write", "put", "remove", "delete", "extend"];
        if patterns.iter().any(|p| method_name.eq_ignore_ascii_case(p)) {
            self.order_events.push(OrderEvent::StateWrite);
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
    let mut order_events = Vec::new();
    let mut visitor = FnVisitor {
        scan: &mut scan,
        divided_bindings: HashSet::new(),
        order_events: &mut order_events,
    };
    visitor.visit_block(block);

    // Check if any external call precedes any later state write (CEI hazard).
    // Detect the pattern: external_call → ... → state_write
    let mut found_external_call = false;
    for event in order_events {
        match event {
            OrderEvent::ExternalCall => {
                found_external_call = true;
            }
            OrderEvent::StateWrite if found_external_call => {
                // An external call appeared before this state write.
                scan.external_call_before_state_write = true;
                break;
            }
            _ => {}
        }
    }

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
        Box::new(crate::rules::Auth004),
        Box::new(crate::rules::Move001),
        Box::new(crate::rules::Math001),
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
    // preserving registration order so output stays identical to .
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

pub fn audit_source_with_registry(
    src: &str,
    reg: &crate::registry::RuleRegistry,
    disabled: &[&str],
) -> Result<AuditReport, AuditError> {
    let ast = syn::parse_file(src).map_err(AuditError::Parse)?;
    let ctx = AuditContext { spec: None };
    let scans = scan_all_functions(&ast);
    let mut report = AuditReport::default();
    reg.run_all(&scans, &ctx, disabled, &mut report);
    Ok(report)
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
    use sdkt_wasm::spec::ContractFunction;

    fn report_for(src: &str) -> AuditReport {
        audit_source(src).unwrap()
    }

    fn has(rep: &AuditReport, id: &str) -> bool {
        rep.findings.iter().any(|f| f.rule_id == id)
    }

    #[test]
    fn audit_source_with_registry_uses_supplied_registry() {
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
            fn check(&self, _scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
                report.add(crate::types::Finding {
                    file: None,
                    rule_id: self.id.to_string(),
                    severity: Severity::Info,
                    message: "stub fired".into(),
                    location: None,
                });
            }
        }

        let mut reg = crate::registry::RuleRegistry::new();
        reg.register_builtin_rules();
        reg.register_rule(Box::new(Stub { id: "REG-TEST" }) as crate::registry::BoxedRule);
        let rep = audit_source_with_registry("pub fn mint() { }", &reg, &["AUTH-001"]).unwrap();
        assert!(rep.findings.iter().any(|f| f.rule_id == "REG-TEST"));
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
    fn auth003_matches_qualified_impl_method_against_unqualified_spec() {
        let src = "impl Token { pub fn initialize(admin: Address) { } }";
        let spec = ContractSpec {
            env_meta: None,
            functions: vec![ContractFunction {
                name: "initialize".into(),
                doc: String::new(),
                parameters: vec![],
                outputs: vec![],
            }],
            custom_types: vec![],
            events: vec![],
        };
        let rep = audit_source_with_spec(src, &spec, &[]).unwrap();
        assert!(has(&rep, "AUTH-003"));
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
    fn math001_detects_direct_and_intermediate_division_before_multiplication() {
        for src in [
            "pub fn quote(amount: i128, bps: i128) -> i128 { amount / 10_000 * bps }",
            "pub fn quote(amount: i128, bps: i128) -> i128 { (amount / 10_000) * (bps + 1) }",
            "pub fn quote(amount: i128, bps: i128) -> i128 { let divided = amount / 10_000; divided * bps }",
        ] {
            let report = report_for(src);
            let finding = report
                .findings
                .iter()
                .find(|finding| finding.rule_id == "MATH-001")
                .expect("division before multiplication should be reported");
            assert_eq!(finding.severity, Severity::Warning);
            assert_eq!(finding.location.as_deref(), Some("quote"));
            assert!(finding.message.contains("divides before multiplying"));
        }
    }

    #[test]
    fn math001_ignores_unrelated_or_non_multiplicative_arithmetic() {
        for src in [
            "pub fn quote(amount: i128, bps: i128) -> i128 { amount * bps }",
            "pub fn quote(bps: i128) -> i128 { 5 / 2 * bps }",
            "pub fn quote(amount: i128) -> i128 { amount / 10_000 }",
            "pub fn quote(amount: i128, bps: i128) -> i128 { amount / 10_000 + bps }",
            "pub fn quote(amount: i128, other: i128, bps: i128) -> i128 { amount / 10_000 + other * bps }",
            "pub fn quote(amount: i128, other: i128, bps: i128) -> i128 { let divided = amount / 10_000; other * bps + divided }",
        ] {
            assert!(
                !has(&report_for(src), "MATH-001"),
                "unexpected math finding for {src}"
            );
        }
    }

    #[test]
    fn math001_can_be_disabled() {
        let src = "pub fn quote(amount: i128, bps: i128) -> i128 { amount / 10_000 * bps }";
        assert!(!has(
            &audit_source_with(src, &["MATH-001"]).unwrap(),
            "MATH-001"
        ));
    }

    #[test]
    fn cei001_fires_on_external_call_before_state_write() {
        let src = r#"
            pub fn swap(env: Env, to: Address) {
                env.invoke_contract(&pool, sym!("withdraw"), args);
                env.storage().set(key, value);
            }
        "#;
        let rep = report_for(src);
        assert!(has(&rep, "CEI-001"));
    }

    #[test]
    fn cei001_detects_nested_invoke_in_set_argument() {
        // set(..., invoke_contract(...)) — the invoke is nested inside set args
        // Arguments are evaluated before the outer call, so invoke happens before set's effect
        let src = r#"
            pub fn swap(env: Env) {
                env.storage().set(key, env.invoke_contract(&pool, sym, args));
            }
        "#;
        let rep = report_for(src);
        // The invoke is inside the set's arguments and is evaluated before set's effect.
        assert!(
            has(&rep, "CEI-001"),
            "invoke in set args happens before set's effect"
        );
    }

    #[test]
    fn cei001_detects_invoke_wrapping_set_argument() {
        // invoke_contract(..., storage().set(...)) — set is nested inside invoke args
        // Arguments are evaluated before the outer call, so set happens before invoke's effect
        let src = r#"
            pub fn swap(env: Env) {
                env.invoke_contract(&pool, sym, env.storage().set(key, value));
            }
        "#;
        let rep = report_for(src);
        // The set happens in the arguments before invoke's effect.
        // This should NOT flag because set comes BEFORE the external call's effect.
        assert!(
            !has(&rep, "CEI-001"),
            "set in invoke args happens before invoke's effect"
        );
    }

    #[test]
    fn cei001_detects_write_call_write_pattern() {
        // write → invoke → write: has both patterns, should flag on the second write
        let src = r#"
            pub fn swap(env: Env) {
                env.storage().set(key1, value1);
                env.invoke_contract(&pool, sym!("withdraw"), args);
                env.storage().set(key2, value2);
            }
        "#;
        let rep = report_for(src);
        // The first set should not cause a flag (no prior call).
        // The invoke should set found_external_call = true.
        // The second set should flag because found_external_call is true.
        assert!(
            has(&rep, "CEI-001"),
            "write → call → write should flag on second write"
        );
    }

    #[test]
    fn cei001_still_emits_one_finding_per_function() {
        // Multiple write → call → write patterns should still emit only one finding.
        let src = r#"
            pub fn swap(env: Env) {
                env.storage().set(key1, value1);
                env.invoke_contract(&pool, sym!("withdraw"), args);
                env.storage().set(key2, value2);
                env.invoke_contract(&pool2, sym!("transfer"), args);
                env.storage().set(key3, value3);
            }
        "#;
        let rep = report_for(src);
        let cei_findings: Vec<_> = rep
            .findings
            .iter()
            .filter(|f| f.rule_id == "CEI-001")
            .collect();
        assert_eq!(
            cei_findings.len(),
            1,
            "should emit only one CEI-001 finding per function"
        );
    }

    #[test]
    fn cei001_silent_on_state_write_before_external_call() {
        let src = r#"
            pub fn swap_safe(env: Env, to: Address) {
                env.storage().set(key, value);
                env.invoke_contract(&pool, sym!("withdraw"), args);
            }
        "#;
        let rep = report_for(src);
        assert!(!has(&rep, "CEI-001"));
    }

    #[test]
    fn cei001_silent_on_only_external_call() {
        let src = r#"
            pub fn relay(env: Env) {
                env.invoke_contract(&addr, sym, args);
            }
        "#;
        let rep = report_for(src);
        assert!(!has(&rep, "CEI-001"));
    }

    #[test]
    fn cei001_silent_on_only_state_write() {
        let src = r#"
            pub fn set_state(env: Env) {
                env.storage().set(key, value);
            }
        "#;
        let rep = report_for(src);
        assert!(!has(&rep, "CEI-001"));
    }

    #[test]
    fn cei001_can_be_disabled() {
        let src = r#"
            pub fn swap(env: Env) {
                env.invoke_contract(&addr, sym, args);
                env.storage().set(key, value);
            }
        "#;
        assert!(
            !has(&audit_source_with(src, &["CEI-001"]).unwrap(), "CEI-001"),
            "CEI-001 should be suppressible"
        );
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
