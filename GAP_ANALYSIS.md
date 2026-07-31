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