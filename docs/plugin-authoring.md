# Plugin Authoring — `sdkt-audit` Rules

Milestone 17 (Phase A) turns `sdkt-audit` into an extensible platform. Rules are
implemented as Rust types implementing the [`AuditRule`](../../crates/sdkt-audit/src/audit.rs)
trait and registered into a process-wide [`RuleRegistry`](../../crates/sdkt-audit/src/registry.rs).

> **Phase A scope:** rules are compiled into the binary (workspace rule crates or
> local source linked by the consumer). Dynamic/shared-library loading is **not**
> part of Phase A — see the roadmap. The `--rules` CLI flag validates rule paths
> and runs all *registered* rules; to actually contribute a rule, add your crate
> as a dependency of the binary that consumes `sdkt-audit` (exactly as the
> reference crate `sdkt-audit-example-rule` does).

## Architecture

```
                ┌──────────────────────────────┐
   source.rs ──▶ │  scan_all_functions()        │  syn AST → Vec<FnScan>
                └──────────────┬───────────────┘
                               ▼
                ┌──────────────────────────────┐
                │  RuleRegistry (global)        │
                │   • built-in rules (AUTH/MOVE)│
                │   • linked plugin rules       │
                └──────────────┬───────────────┘
                               ▼
                for each rule (registration order):
                  rule.check(&scans, &ctx, &mut report)
                               ▼
                         AuditReport (JSON / text)
```

- **`AuditRule`** — the trait every rule implements (`id`, `severity`,
  `description`, `check`).
- **`RuleRegistry`** — ordered, de-duplicated collection; `register_rule`,
  `register_builtin_rules`, `registered_rules`, `run_all`.
- **`register_rule!` macro** — ergonomic registration into the global registry.
- **`AuditContext`** — optional `ContractSpec` for ABI cross-checks.
- **`Finding`** — the unit a rule emits into the report.

## Rule lifecycle

1. **Author** implements `AuditRule` and inspects `&[FnScan]` (per-function scan:
   bound locals, argument usage counts, `require_auth`/`invoke_contract` counts).
2. **Register** the rule (once) into the global registry.
3. **Audit** execution iterates the registry in registration order, skipping any
   id present in `--disable`. Ordering is stable, so output is deterministic.

## Creating a rule

```rust
use sdkt_audit::{AuditContext, AuditRule, AuditReport, Finding, FnScan, Severity};
use sdkt_audit::register_rule;

pub struct NoPanicRule;

impl AuditRule for NoPanicRule {
    fn id(&self) -> &'static str { "CUSTOM-001" }
    fn severity(&self) -> Severity { Severity::Warning }
    fn description(&self) -> &'static str { "Flags functions named 'sdkt_example_trigger'" }

    fn check(&self, scans: &[FnScan], _ctx: &AuditContext, report: &mut AuditReport) {
        for s in scans {
            if s.fn_name.contains("sdkt_example_trigger") {
                report.add(Finding {
                    rule_id: self.id().to_string(),
                    severity: self.severity(),
                    message: format!("Matched `{}`", s.fn_name),
                    location: Some(s.fn_name.clone()),
                });
            }
        }
    }
}

// Register into the global registry (call once at startup).
pub fn register() {
    register_rule!(NoPanicRule);
}
```

## Registering it

For Phase A, link your rule crate into the binary that calls `sdkt_audit`:

```toml
# in the consumer's Cargo.toml
[dependencies]
sdkt-audit = { path = "../sdkt-audit" }
my-rule-crate = { path = "../my-rule-crate" }

[features]
plugins = ["my-rule-crate"]
```

```rust
// in the consumer, gated behind the feature:
#[cfg(feature = "plugins")]
my_rule_crate::register();
```

The reference implementation `crates/sdkt-audit-example-rule` demonstrates this
end-to-end (rule `EXAMPLE-001`).

## Testing a rule

Rules are pure functions over `&[FnScan]`. Unit-test `check` directly:

```rust
#[test]
fn fires_on_trigger() {
    let scans = vec![FnScan {
        fn_name: "sdkt_example_trigger_x".into(), ..Default::default()
    }];
    let ctx = AuditContext { spec: None };
    let mut report = AuditReport::default();
    ExampleRule.check(&scans, &ctx, &mut report);
    assert_eq!(report.summary.total, 1);
}
```

Also add a CLI integration test (as in `crates/sdkt-cli/tests/audit_integration_test.rs`)
that builds with your feature and asserts the rule id appears in output.
