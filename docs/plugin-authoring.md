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

**Phase B (M18) — dynamic loading is now available.** A rule can also be shipped
as a native shared library (`.so` / `.dylib` / `.dll`) and loaded at runtime with
`sdkt audit <src> --rules <plugin.so>` — no CLI rebuild required. See
[Dynamic plugins](#dynamic-plugins-phase-b) below.

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

## Dynamic plugins (Phase B)

A dynamic plugin is a native shared library exporting a fixed C-ABI. The host
(`sdkt-audit`, feature `plugins`) loads it with `libloading`, checks the ABI
major version, and wraps each plugin in a `PluginRule` implementing `AuditRule`.
Only `#[repr(C)]` flat data crosses the FFI — **no Rust trait objects or `Box`
cross the boundary**, and plugin-owned memory is freed inside the plugin.

### C-ABI contract (stable)

| Symbol | Signature | Purpose |
|--------|-----------|---------|
| `sdkt_plugin_abi_version` | `() -> u32` | Packed `(major<<16)|minor`; must match host major. |
| `sdkt_plugin_id` | `() -> *const c_char` | Rule id, e.g. `EXAMPLE-001`. |
| `sdkt_plugin_severity` | `() -> u32` | 0=critical, 1=warning, 2=info. |
| `sdkt_plugin_description` | `() -> *const c_char` | Human-readable description. |
| `sdkt_plugin_init` | `(*const c_char) -> c_int` | Cache the contract source; 0=ok. |
| `sdkt_plugin_check` | `(*mut SdktAuditReportC) -> c_int` | Run the rule, write findings into the buffer. |
| `sdkt_plugin_free` | `() -> ()` | Optional cleanup (host keeps the lib alive). |

`SdktAuditReportC` is a fixed-capacity (`MAX_FINDINGS = 64`) `#[repr(C)]` buffer
of `SdktAuditFindingC { rule_id, severity, message, location }` (all C strings).
The host copies the strings into owned `String`s during the call.

### Authoring a dynamic plugin

The reference crate `sdkt-audit-example-rule` (feature `plugins`) builds a
loadable `libsdkt_audit_example_rule.so` that exports these symbols. Its
`sdkt_plugin_check` runs **only its own** `ExampleRule` (via the in-crate
`AuditRule::check`) — never the global registry, to avoid re-entrant recursion.

```toml
# my-rule-crate/Cargo.toml
[dependencies]
sdkt-audit = { version = "1.0.0", path = "../sdkt-audit" }

[features]
plugins = ["sdkt-audit/plugins"]

[lib]
name = "my_rule"
crate-type = ["rlib", "cdylib"]   # cdylib → loadable artifact
```

```rust
// my-rule-crate/src/plugin_abi.rs  (only with feature `plugins`)
use sdkt_audit::plugin_abi::*;

static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_abi_version() -> u32 { abi_version_pack() }
#[no_mangle]
pub unsafe extern "C" fn sdkt_plugin_id() -> *const std::os::raw::c_char { /* "MYRULE-001\0" */ }
// ... severity / description / init / check / free as above
```

### Loading it

```bash
# Build the plugin (cdylib):
cargo build -p my-rule-crate --features plugins
# Load at audit time (CLI must also be built with --features plugins):
sdkt audit contracts/token/src/lib.rs --rules target/debug/libmy_rule.so
```

> **Security:** dynamic plugins run in-process. Only load plugins you trust or
> built yourself. A plugin whose ABI major version differs from the host is
> rejected with a clear error.

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
