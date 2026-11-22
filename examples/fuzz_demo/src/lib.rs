#![no_std]

//! Minimal Soroban contract for demonstrating the `sdkt fuzz` workflow.
//!
//! It contains exactly one deliberate defect, clearly marked intentional:
//! `compute` divides `a` by `b` *before* multiplying by `a` — the
//! division-before-multiplication precision-loss pattern flagged by the
//! `MATH-001` static-analysis rule (`(a / b) * a` truncates `a / b` to its
//! integer quotient before the multiply, silently discarding precision).
//!
//! This is a demonstration defect: do **not** copy this pattern into real
//! contracts. The campaign in `README.md` surfaces it deterministically by
//! driving `b` to zero (the fuzzer's `zero` argument mutation), which traps
//! the execution; declared with `--expect-success`, that trap is reported
//! as an `UNEXPECTED_ERROR` finding — the canonical "did it work?" signal.

use soroban_sdk::{contract, contractimpl};

#[contract]
pub struct FuzzDemo;

#[contractimpl]
impl FuzzDemo {
    /// Computes `(a / b) * a`.
    ///
    /// # DELIBERATE DEFECT (intentional — the `MATH-001` demonstration target)
    ///
    /// The division runs **before** the multiplication, so `a / b` is
    /// truncated to its integer quotient before being multiplied back by
    /// `a`; the lost fraction can never be recovered. When `b == 0` the
    /// division also traps the VM.
    ///
    /// Do not copy this pattern into real contracts — multiply before
    /// dividing and handle possible overflow instead. Checked arithmetic
    /// alone does not restore precision already lost to truncation
    /// (`checked_div` followed by `checked_mul` still truncates).
    pub fn compute(a: i128, b: i128) -> i128 {
        // INTENTIONAL DEFECT: divide before multiply (MATH-001 pattern).
        // `a / b` truncates first; the subsequent multiply cannot restore
        // the discarded fraction. `b == 0` traps execution.
        (a / b) * a
    }
}
