//! Read-only natural-language front end for the Soroban DevKit CLI.
//!
//! # What this is
//!
//! A **deterministic, rule-based planner** plus a **thin executor**:
//!
//! ```text
//! request text
//!   → Intent::parse            (keywords + argument extraction; no model)
//!   → CapabilityRegistry        (sdkt_core::registry — the only source of commands)
//!   → safety gate               (mutating / confirmation-required ⇒ blocked)
//!   → argv builder              (validated, explicit network when one is needed)
//!   → executor                  (separate stdout/stderr/exit code, timeout, no retry)
//!   → ExecutionResult           (structured; human summary or pure JSON)
//! ```
//!
//! v0.1 has **no** model, no network service, and no scraping of `--help`.
//! If a request cannot be mapped unambiguously it returns
//! [`Status::NeedsClarification`] and runs nothing.
//!
//! # Safety posture
//!
//! v0.1 executes **read-only** capabilities only. Anything the registry marks
//! [`Safety::Mutating`], anything with `requires_confirmation`, and anything
//! that writes local state ([`Safety::LocalWrite`]) is refused *before* argv
//! construction, with the reason recorded in the result. There is no code
//! path in this crate that can sign, submit, deploy, fund, or extend TTL,
//! and the executor is invoked with the testnet endpoint only — a mainnet
//! target would require a saved profile the agent refuses to guess.

pub mod executor;
pub mod intent;
pub mod plan;
pub mod result;

pub use executor::{execute, Executor, RawExecution};
pub use intent::{Intent, ParseError};
pub use plan::{plan, plan_intent, Plan, PlanError, Planned};
pub use result::{Evidence, ExecutionResult, Status};

/// Result schema version of the agent's own output document.
pub const AGENT_SCHEMA_VERSION: u32 = 1;
