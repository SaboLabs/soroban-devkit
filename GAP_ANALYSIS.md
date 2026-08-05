# Soroban DevKit — GAP ANALYSIS
**Date:** 2026-07-31 | **Prepared for:** Open-Source Startup Scaffold

---

## 1. Executive Summary

The Soroban ecosystem has matured significantly since mainnet launch. However, developer tooling remains fragmented across multiple repositories with overlapping functionality, missing critical workflows, and inconsistent maintenance. This analysis identifies gaps that `sdkt` (Soroban DevKit) will fill as a unified, modular toolkit.

---

## 2. Existing Projects — Feature Matrix

| Project | Purpose | Key Features | Strengths | Weaknesses | Stars | Maintenance |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **`stellar/soroban-example-dapp`** | Full-stack dApp template | Next.js + Freighter + Soroban RPC | Complete working example, good for beginners | Not a library — copy-paste only, no reusable modules | 1.4k | Active |
| **`stellar/rs-soroban-sdk`** | Rust contract SDK | Contract macros, storage types, test utilities | Official, well-documented, type-safe | No static analysis, no rent helpers, no XDR helpers | 196 | Active |
| **`stellar/soroban-examples`** | Contract examples | Token, atomic swap, single-offer | Reference implementations | No tooling, examples only | 126 | Active |
| **`stellar/launchtube`** | Meta-transaction relayer | Sponsored tx, gasless UX | Solves real problem | Centralized, single-point-of-failure | 25 | Stale |
| **`stellar/js-soroban-client`** | JS/TS RPC client | Contract invocation, event streaming | Official client | No CLI, no XDR decoding helpers | 24 | Active |
| **`stellar/quickstart`** | Docker local node | Full network in container | Fast local dev | Heavy (5GB+), no lightweight mode | 221 | Active |
| **`stellar/smart-account-kit`** | Passkey smart wallet | WebAuthn, policy signers | Cutting-edge auth | Experimental, minimal docs | 0 | Experimental |
| **`stellar/soroban-quest`** | Educational gamified course | Interactive tutorials | Great for learning | Not tooling | 9 | Stale |

---

## 3. Functional Gaps — What's Missing

### Gap A: No Unified CLI for Contract Lifecycle

**Current state:**
- `stellar-cli` handles build/deploy but not analysis or inspection
- `soroban-cli` is deprecated in favor of `stellar-cli`
- Developers must chain 5+ commands manually to:
  - Inspect storage TTL
  - Estimate rent fees
  - Decode XDR events
  - Audit security issues

**What `sdkt` provides:**
- Single binary with subcommands: `sdkt decode`, `sdkt storage`, `sdkt audit`
- Zero-config workflow — detects `.sdkt.toml` automatically

---

### Gap B: No Storage Rent Visibility

**Current state:**
- Soroban's storage rent model (Temporary/Persistent/Instance) is unique and complex
- No tool exists to check `remaining_ttl` for a contract or user state
- Developers discover expiration only when their contract locks

**What `sdkt` provides:**
- `sdkt storage check <contract-id>` — returns TTL timeline and extension cost
- `sdkt storage estimate <wasm>` — predicts deployment storage fees

---

### Gap C: No Static Security Analysis

**Current state:**
- Rust compiler catches type errors but not Soroban-specific logic bugs
- Move semantics violations (`Address`, `Symbol` moved by accident) cause hard-to-debug compile errors
- Missing `require_auth()` on admin functions is a common security hole

**What `sdkt` provides:**
- `sdkt audit` uses `syn` AST to flag move violations and missing auth checks
- Extensible rule system — plugins can add custom lints

---

### Gap D: No Local XDR Decoder

**Current state:**
- Soroban RPC returns events in base64-encoded XDR
- Developers copy-paste into Stellar Laboratory to decode
- No offline/CLI alternative for CI/CD or scripting

**What `sdkt` provides:**
- `sdkt decode <base64>` → JSON output
- Works offline with local WASM spec files
- Piping support for CI: `stellar contract invoke | sdkt decode`

---

### Gap E: No Standardized ABI/Interface Viewer

**Current state:**
- Contract interface inspection requires `stellar contract inspect` (limited)
- No way to view on-chain state variables without writing a script
- `soroban-cli` inspect is being phased out

**What `sdkt` provides:**
- `sdkt inspect <contract-id>` — reads WASM custom sections and current storage
- Interactive CLI menu listing all read/write functions

---

## 4. Why `sdkt` Should Exist

| Reason | Evidence |
| :--- | :--- |
| **Unified developer experience** | No other tool provides all lifecycle stages in one binary |
| **Reduces onboarding friction** | New developers can audit, decode, and deploy without learning 5 separate CLIs |
| **Prevents production failures** | Rent expiration kills contracts; `sdkt` catches it pre-emptively |
| **Enables CI/CD pipelines** | `sdkt decode` and `sdkt audit` can be integrated into GitHub Actions |
| **Community contribution catalyst** | Plugin system allows external developers to extend tooling without forking core |
| **Aligns with Stellar ecosystem goals** | Stellar Foundation actively encourages tooling development via SCF grants |

---

## 5. Feature Comparison Table

| Feature | `stellar-cli` | `soroban-cli` | `sdkt` |
| :--- | :--- | :--- | :--- |
| Build/Deploy | ✅ | ✅ | ✅ (via wrapper) |
| Storage TTL inspection | ❌ | ❌ | ✅ |
| Rent cost estimation | ❌ | ❌ | ✅ |
| XDR → JSON decoding | ❌ | ❌ | ✅ |
| Static security audit | ❌ | ❌ | ✅ |
| Move semantics checker | ❌ | ❌ | ✅ |
| Plugin system | ❌ | ❌ | ✅ |
| Unified config (`.sdkt.toml`) | ❌ | ❌ | ✅ |

---

## 6. Conclusion

Soroban DevKit (`sdkt`) does not duplicate existing tools — it **augments** them. It fills the gap between `stellar-cli` (build/deploy) and `rs-soroban-sdk` (contract code) by providing a cohesive developer experience for inspection, analysis, and debugging.

The gaps identified above are real production pain points. `sdkt` addresses them with a modular, extensible, and well-documented architecture that will grow with the ecosystem.

**Approved for implementation.**

---

## 7. Status Update — Post-Milestone 10 (2026-08-05)

This section supersedes the "What `sdkt` provides" promises above with **shipped reality**. The market gap analysis (Sections 1–6) remains valid; the implementation has caught up substantially.

### Gap Closure (current)

| Original Gap | State | Shipped In |
|--------------|-------|-----------|
| **Gap A** — Unified CLI lifecycle | ✅ **Closed** | M3A–M10: `decode`, `storage`, `inspect`, `tx`, `events`, `account`, `fee`, `wasm`, `identity`, `init`, `deploy` all present |
| **Gap B** — Storage rent visibility | ✅ **Closed** | M3A: `sdkt storage check` (TTL + extension cost), `storage estimate` |
| **Gap C** — Static security analysis | ✅ **Closed (M13)** | M13 shipped `sdkt-audit` (new crate): `AUTH-001/002/003` + `MOVE-001` rules, `sdkt audit` CLI. `docs/milestone-11-plan.md` is retained as historical candidate context only. |
| Plugin system | 🟡 **Planned (post-M13)** | M13's `sdkt-audit` `AuditRule` trait is now the natural on-ramp for external lint/rule plugins. |
| **Gap D** — Local XDR decoder | ✅ **Closed** | M3A/M5: `sdkt decode` (ScVal / TransactionEnvelope / ContractEvent), offline |
| **Gap E** — ABI / interface viewer | ✅ **Closed** | M3B + M10 (ENG-16): `sdkt inspect` + `--abi` ABI-aware decoding of events/storage |

### Capabilities shipped beyond the original GAP_ANALYSIS scope

- **Mutability foundation (M8):** `sdkt tx simulate`, `sdkt tx submit` (with polling), `sdkt tx build` envelope builder, `sdkt identity` ED25519 keystore.
- **WASM tooling (M9):** `sdkt wasm metadata` / `sdkt wasm cache`, `sdkt-wasm` crate with `ContractSpec` parser, `sdkt deploy` + `sdkt init` scaffolding.
- **ABI-aware decoding (M10 / ENG-16):** real base64 XDR event topic/value decoding via `decode_event_topics`.

### Remaining unaddressed pillars

1. **Plugin system** — no external lint/rule loading yet; M13's `sdkt-audit` `AuditRule` trait is now the natural foundation (planned post-M13).
2. **Contract upgrade safety** — M12 shipped the *diff* half (`sdkt diff`); the *recommend-abort-on-breaking-change* guard for `sdkt deploy` remains (M14).
3. **CI/CD Action packaging (M15)** — depends on audit-in-CI premise, now feasible since `sdkt audit` exists.

### Notable deltas vs original doc

- The GAP_ANALYSIS originally described `sdkt audit` as using `syn` AST to flag move violations and missing auth — this is now the concrete M11 design (`docs/milestone-11-plan.md`).
- "Interactive CLI menu" for `inspect` (Gap E) was scoped down to structured pretty/JSON output; no interactive TUI was built.
- Horiz/account graph enrichment shipped in M7, extending Gap A's account inspection beyond the original plan.

**Conclusion:** 4 of 5 original gaps are closed; Gap C is the active frontier. `sdkt` now spans the full read-only + mutating lifecycle and is production-hardened (M6 CI/clippy gates).