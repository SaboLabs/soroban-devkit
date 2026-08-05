# Milestone 11 — Candidate Proposal A: Static Security Analysis (`sdkt audit`)

> **STATUS: CANDIDATE PROPOSAL — NOT APPROVED SCOPE.**
> There is no official Milestone 11 specification, ROADMAP entry, or ENG design document.
> This document is **one of three candidate proposals** for M11 (see ROADMAP.md / CHANGELOG v0.11.0-alpha).
> It will be promoted to the implementation plan only if the operator approves Candidate A.
> No Rust code has been written.

**Branch:** `feat/milestone-11`
**Baseline:** v0.10.0-alpha (Milestone 10 merged to `main`)
**Target (if approved):** New `sdkt-audit` crate + `sdkt audit` CLI subcommand performing static analysis of Soroban contracts.
**Status:** Candidate design — documentation only.

---

## 1. Objectives

Address **Gap C** from `GAP_ANALYSIS.md`, the only original gap with zero code.

1. Add `sdkt audit <target>` — static analysis of a Soroban contract's Rust source (or WASM).
2. Detect Soroban-specific logic/security bugs that the Rust compiler does **not** catch:
   - Missing `require_auth()` on privileged / admin functions.
   - Move-semantics violations (`Address`, `Symbol`, or owned `ScVal` used after move).
   - Unprotected / unauthenticated `env.invoke_contract` calls.
3. Provide an **extensible rule registry** so future lints (and eventually plugins, M13) can register without touching core.
4. Reuse the existing `OutputFormat` (Json / Pretty) for machine- and human-readable reports.

---

## 2. Scope

### In Scope
- New crate `sdkt-audit` (depends on `syn`, `serde`, `sdkt-core`, `sdkt-wasm`).
- `AuditRule` trait + built-in rule set (auth, moves, invoke-auth).
- `AuditReport` / `Finding` / `Severity` public types.
- `sdkt audit` subcommand in `sdkt-cli`.
- `AuditConfig` added to `DevKitConfig` (serde default, non-breaking).
- Unit + integration tests with fixture contracts.
- Docs: README, CHANGELOG (v0.11.0-alpha), this plan.

### Out of Scope (this milestone)
- Runtime / on-chain analysis (only static source/WASM analysis).
- Automatic fixing / `cargo fix`-style rewrites.
- Plugin loading from external crates (deferred to M13 — the `AuditRule` trait is designed to enable it).
- WASM-only decompiled AST analysis beyond spec cross-check (M11 analyzes Rust source; WASM path maps functions via `ContractSpec` for context only).

---

## 3. Architecture

```
sdkt-cli (Audit variant)
   │  routes + formats
   ▼
sdkt-audit  (NEW)
   │  - parses source with `syn`
   │  - runs registered AuditRule(s)
   │  - cross-checks against ContractSpec (from sdkt-wasm)
   ▼
sdkt-core   (AuditConfig, OutputFormat)
sdkt-wasm   (parse_contract_spec — function list for context)
```

- `sdkt-audit` performs **no networking and no I/O** beyond reading the target file path the CLI passes in. This keeps it consistent with the `sdkt-xdr`/`sdkt-core` boundary rule.
- `syn` is the only new heavy dependency, confined to `sdkt-audit`.

### Public API (proposed)

```rust
// sdkt-audit
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity { Info, Warning, Critical }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule_id: String,        // e.g. "AUTH-001"
    pub severity: Severity,
    pub message: String,
    pub location: Option<Span>, // file:line:col when source is available
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub findings: Vec<Finding>,
    pub summary: AuditSummary,
}

pub trait AuditRule {
    fn id(&self) -> &str;
    fn run(&self, ast: &syn::File, spec: &ContractSpec) -> Vec<Finding>;
}

pub fn audit_source(
    src: &str,
    spec: &ContractSpec,
    rules: &[Box<dyn AuditRule>],
) -> AuditReport;
```

---

## 4. Rule Set (built-in, M11)

| Rule ID | Name | Severity | Detection |
|---------|------|----------|-----------|
| `AUTH-001` | Missing `require_auth` on admin fn | Critical | Function whose name matches admin/privileged heuristics (contains `admin`, `set_`, `mint`, `withdraw`, `upgrade`, `initialize`) and has no `require_auth()` call in its body. |
| `AUTH-002` | Unauthenticated `invoke_contract` | Warning | `env.invoke_contract(...)` with no preceding `require_auth()` in the same fn. |
| `MOVE-001` | Use of value after move | Warning | Heuristic: a `Symbol`/`Address`/owned local bound from a contract call is referenced after being passed by value into another call. (Conservative; may produce Info-level notes.) |
| `AUTH-003` | `initialize` not guarded | Critical | `initialize`/`constructor`-like fn lacks auth + lacks one-time-execution guard. |

Rules are registered in `sdkt-audit::rules` and collected into a `Vec<Box<dyn AuditRule>>`.
`--disable <RULE_ID>` (repeatable) removes rules before the run.

---

## 5. CLI Design

```rust
/// Static security analysis of a Soroban contract
Audit {
    /// Path to contract .rs source (or compiled WASM for spec-context mode)
    target: String,
    /// Output format (json | pretty)
    #[arg(short, long, default_value = "pretty")]
    format: OutputFormat,
    /// Disable a rule by id (repeatable)
    #[arg(long)]
    disable: Vec<String>,
},
```

Example output (pretty):
```
sdkt audit report
Target: contracts/token/src/lib.rs
Findings: 2
  [CRITICAL] AUTH-001  mint() — no require_auth() call
                         contracts/token/src/lib.rs:42:1
  [WARNING ] AUTH-002  invoke_contract without prior auth in transfer()
                         contracts/token/src/lib.rs:88:5
Summary: 1 critical, 1 warning
```

---

## 6. Phases

| Phase | Task | Crate | Est. |
|-------|------|-------|------|
| 0 | Scaffold `sdkt-audit` crate, add to workspace, `AuditConfig` in `sdkt-core` | sdkt-audit, sdkt-core | 30m |
| 1 | `AuditReport`/`Finding`/`Severity` types + serde | sdkt-audit | 30m |
| 2 | `syn` parse wrapper + `AuditRule` trait + registry | sdkt-audit | 45m |
| 3 | Built-in rules AUTH-001/002/003, MOVE-001 | sdkt-audit | 2h |
| 4 | `audit_source()` orchestration + `ContractSpec` cross-check | sdkt-audit | 1h |
| 5 | `Audit` CLI subcommand + `--disable` + OutputFormat wiring | sdkt-cli | 45m |
| 6 | Fixture contracts (good/bad) + unit tests (≥8) | sdkt-audit | 1h |
| 7 | Integration tests (`sdkt audit <fixture>` stdout assertions) | sdkt-cli | 45m |
| 8 | README + CHANGELOG v0.11.0-alpha + docs | root | 30m |
| 9 | `cargo fmt` + `clippy -- -D warnings` + `test --workspace` | workspace | 15m |

---

## 7. Test Strategy

**Unit (`sdkt-audit`):**
- `syn` parse of valid + invalid Rust (graceful error on non-Rust).
- AUTH-001 positive (admin fn without auth) and negative (admin fn with auth).
- AUTH-002 detection when invoke precedes no auth.
- AUTH-003 initialize-guard detection.
- MOVE-001 heuristic on a fixture with a moved `Address`.
- Rule registry `--disable` removes the rule's findings.
- Severity sort / summary counts.

**Integration (`sdkt-cli`):**
- `sdkt audit good_contract.rs` → exit 0, zero critical findings in JSON.
- `sdkt audit bad_contract.rs` → stdout JSON contains `AUTH-001`.
- `sdkt audit bad_contract.rs --disable AUTH-001` → `AUTH-001` absent.

**Validation (mandatory gates):**
```
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

## 8. Backward Compatibility

| Area | Impact | Strategy |
|------|--------|----------|
| `DevKitConfig` | ⚠️ Partial | `AuditConfig` added with `#[serde(default)]`; old `.sdkt.toml` still parses. |
| Existing subcommands | ✅ Full | `Audit` is purely additive. |
| `sdkt-core` API | ✅ Full | Only additive types; no changes to existing. |

---

## 9. Risks

| Risk | Mitigation |
|------|------------|
| `syn` false positives on move analysis | MOVE-001 emits `Warning`/`Info` only, never `Critical`; documented as heuristic. |
| Admin-fn heuristic misses obscure names | Cross-check against `ContractSpec` exported functions; rule list is extensible. |
| WASM-only contracts can't be source-analyzed | WASM path uses `ContractSpec` for context + reports limited scope (no line spans). |
| Build-size / compile-time from `syn` | Confined to `sdkt-audit`; does not affect `sdkt-cli` binary deps beyond the crate. |
