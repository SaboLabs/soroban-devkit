# sdkt-fuzz — extension guide

Contributor-facing guide for extending the fuzz crate: adding a mutation
operator, adding an oracle condition, and changing campaign-level settings.
The baseline functionality it documents is wired as of `origin/main`
(`5f6e7c8b`); every symbol named here exists in the source tree at that
revision. The `MapInsertKey` operator in Section 4 is the one exception: it
is introduced by this PR and is therefore only present on the branch that
carries it.

## Where this guide lives

`crates/sdkt-fuzz/README.md` — linked from the workspace `README.md`
(Capabilities section) and `CONTRIBUTING.md` (Good First Issues). Chosen
because:

1. `crates/sdkt-fuzz/Cargo.toml` sets `readme = "../../README.md"`, so the
   crate has no contributor-facing entry point of its own.
2. `CONTRIBUTING.md` § Good First Issues invites contributors into the
   codebase; the fuzz pipeline is mature enough that "add a mutation operator"
   or "add an oracle condition" is a natural extension, but the current
   docs do not say where the wiring lives.
3. The out-of-scope list already lives in `lib.rs` (`//! Out of scope`), and
   a crate-local README is where contributors look first.

## How to read this guide

- Section 1 adds a mutation operator.
- Section 2 adds an oracle condition.
- Section 3 changes campaign-level configuration (e.g. generation caps).
- Section 4 is a worked example: a complete `MapInsertKey` mutation operator
  added end to end, with the test that pins it.

Every step names the exact file, type, and function to edit.

---

## 1. Adding a mutation operator

### 1.1 Where the wiring lives

| Concern | File | Type / function |
|---|---|---|
| Operator variant | `crates/sdkt-fuzz/src/mutation.rs` | `pub enum Operator` |
| Operator → type-class map | `crates/sdkt-fuzz/src/mutation.rs` | `Operator::for_type` |
| Operator name (identity) | `crates/sdkt-fuzz/src/mutation.rs` | `Operator::name` |
| Apply operator to a value | `crates/sdkt-fuzz/src/mutation.rs` | `apply_operator` (private) |
| Plan which (arg, op) to apply | `crates/sdkt-fuzz/src/mutation.rs` | `plan_mutation` |

### 1.2 Which type classes each operator is valid for

Operators are grouped by the type class they apply to. The mapping is
**structural**: an operator is selected by `ContractType.kind` + `ContractType.name`,
so an ill-typed mutation can never be selected.

| Type class | `ContractType.kind` / `name` | Operators |
|---|---|---|
| Bool | `primitive` / `bool` | `BoolTrue`, `BoolFalse` |
| Numeric | `primitive` / `u32` `i32` `u64` `i64` `u128` `i128` | `Zero`, `One`, `Min`, `Max`, `Random` |
| String / bytes | `primitive` / `string` `bytes` | `Empty`, `LengthBoundary`, `Random` |
| Other primitives | `primitive` / (anything else) | `Random` only |
| Vec | `compound` / `vec<...>` | `VecEmpty`, `VecRemoveOne`, `VecDuplicate`, `VecAppend`, `VecMutateOne` |
| Option | `compound` / `option<...>` | `OptionNone`, `OptionSome`, `OptionSomeMutated` |
| Map | `compound` / `map<...>` | `MapEmpty`, `MapRemoveOne`, `MapInsertKey` |
| All others | (unsupported) | none — `Vec::new()` |

When adding a new operator, decide which row of this table it belongs to. If
it fits an existing class, add it to that row. If it requires a new class
(e.g. a `Set<...>` type), you must also extend `Operator::for_type` to map the
new type name to the operator.

### 1.3 Determinism contract

A mutation's identity is `(seed, case_id, arg_index, operator)`. The same tuple
must always produce the same mutation:

- All randomness comes from `SplitMix64::keyed(&[derive_key_child(seed, case_id),
  arg_index.to_le_bytes(), operator.name().as_bytes()].concat())` (see
  `mutate_arg`). No global RNG, no wall-clock, no `ModuleCache` internals.
- Non-target arguments are carried through byte-identical — the mutator
  returns the full new argument vector with exactly one position changed.
- An operator that cannot produce a well-typed value returns
  `MutationError::NoApplicableMutation`; it is never silently emitted.

### 1.4 Steps to add an operator

1. Add the variant to `pub enum Operator` in `mutation.rs`, in the correct type-class group.
2. Add the arm to `Operator::name` returning the snake_case name. This name is part of the identity — it must never change once published.
3. Add the arm to `Operator::for_type` mapping the operator to its type class(es).
4. Add the arm to `apply_operator` implementing the actual mutation.
5. Add a test in `crates/sdkt-fuzz/tests/generator_mutation.rs` (section E/F).

### 1.5 What a new operator test must prove

- Same `(seed, case_id, arg_index, operator)` → identical `Mutation` (determinism).
- Non-target args are preserved byte-identical.
- The operator produces a value that passes `type_matches` (stays well-typed).
- The operator refuses to apply to a type it does not belong to
  (`MutationError::OperatorNotApplicableForType`).
- The operator refuses when it cannot produce a usable value
  (`MutationError::NoApplicableMutation`).

---

## 2. Adding an oracle condition

### 2.1 Where the wiring lives

| Concern | File | Type / function |
|---|---|---|
| Declare an expectation | `crates/sdkt-fuzz/src/oracle.rs` | `pub struct Expected` |
| Declared call outcome | `crates/sdkt-fuzz/src/oracle.rs` | `pub enum ExpectedBehavior` |
| Reason a finding was raised | `crates/sdkt-fuzz/src/oracle.rs` | `pub enum ReasonCode` |
| Oracle verdict | `crates/sdkt-fuzz/src/oracle.rs` | `pub enum Classification` |
| The classifier | `crates/sdkt-fuzz/src/oracle.rs` | `Oracle::classify` |

### 2.2 Which `Expected` field gates each condition

| Condition | `Expected` field | Trigger |
|---|---|---|
| `ReturnMismatch` | `behavior: Success { expect_return: Some(want) }` | Call succeeded with a different value. |
| `StateMismatch` | `state: Some(map)` | Declared expected state differs from observed state (byte-exact XDR). |
| `EventMismatch` | `events: Some(vec)` | Declared expected events differ from observed events. |
| `UnexpectedError` | `behavior: Success { .. }` or `SuccessWith` | Execution produced an error. |
| `AuthorizationBypass` | `auth_required: true` | Host enforced authorization AND execution succeeded anyway. |
| `ResourceLimit` | `resource_limit_is_finding: true` + `behavior: Success` | Budget-class host error under a declared Success. |

### 2.3 The "undeclared conditions never produce findings" rule

This is the core rule of the oracle:

> A finding exists only where a rule was **explicitly declared**. Undeclared
> conditions can only yield `Classification::Pass` or
> `Classification::ExpectedError`.

In code: `Expected` defaults to `Default::default()` (all fields `None`/`false`).
`Oracle::classify` only returns `Classification::Finding(reason)` for conditions
that are explicitly declared. If you want to detect a new class of problem,
you must declare it in `Expected` and implement the check in `classify`.

### 2.4 Determinism / identity requirements

- The oracle is a pure deterministic function: `classify(obs)` always returns
  the same `Classification` for the same `Observation`.
- An error is never evidence of a defect by itself. It is classified against a
  declared `ExpectedBehavior::Error { error_type, code }` pair.
- The `resource_limit_is_finding` rule must be **explicitly enabled** — resource
  exhaustion is an outcome, not a verdict, by default.

### 2.5 Steps to add an oracle condition

1. Add the variant to `pub enum ReasonCode` in `oracle.rs`.
2. Add the name arm to `ReasonCode::name` and the inverse parse arm to `ReasonCode::from_name` (artifact/replay reuse this vocabulary).
3. Add the field to `pub struct Expected` that gates the condition.
4. Implement the check in `Oracle::classify`, returning `Classification::Finding(your_reason)`.
5. Add a test in `crates/sdkt-fuzz/tests/oracle_minimizer_campaign.rs` or `auth_oracle_matrix.rs`.

### 2.6 What a new oracle test must prove

- The condition triggers **only** when the gating field is set.
- The condition produces the expected `ReasonCode`.
- The same observation always produces the same `Classification` (determinism).
- When the gating field is not set, the condition is never raised (the "undeclared never produces findings" rule).

---

## 3. Changing campaign-level configuration

### 3.1 Where the wiring lives

| Concern | File | Type / function |
|---|---|---|
| Generation caps | `crates/sdkt-fuzz/src/generator.rs` | `pub struct GenerationCaps` |
| Campaign input | `crates/sdkt-fuzz/src/campaign.rs` | `pub struct CampaignInput` |
| Execution config | `crates/sdkt-fuzz/src/config.rs` | `pub struct FuzzConfig` |
| Budget | `crates/sdkt-fuzz/src/config.rs` | `pub struct BudgetConfig` |

### 3.2 Generation caps

`GenerationCaps` controls the depth/size bounds of generated values. It is part
of the deterministic contract: the same caps must produce the same values.

```rust
pub struct GenerationCaps {
    pub max_depth: u32,       // Max compound nesting depth (outermost parameter = depth 0).
    pub max_vec_len: usize,   // Max elements in a generated Vec.
    pub max_byte_len: usize,  // Max bytes in a generated String / Bytes.
    pub max_map_entries: usize, // Max entries in a generated Map.
}
```

To add a new cap:

1. Add the field to `GenerationCaps`.
2. Update `Default::default()`.
3. Use it in `generate_value` (generator.rs) to bound the relevant dimension.
4. Add a test proving the cap is respected and deterministic.

### 3.3 Adding a new environment field

The `Environment` struct (`crates/sdkt-fuzz/src/environment.rs`) models the
ledger context (network, ledger sequence, timestamp). To add a field:

1. Add the field to `Environment`.
2. Update `Default::default()`.
3. Wire it into `Environment::validate` (called by `FuzzConfig::validate`).
4. If it affects the ledger info, wire it into `Executor` construction.

---

## 4. Worked example: `MapInsertKey` mutation operator

This section adds a real `MapInsertKey` operator end to end, with the test
that pins it. The operator inserts a new key into a `map<K, V>` value.

### 4.1 Add the variant

In `crates/sdkt-fuzz/src/mutation.rs`, inside `pub enum Operator`:

```rust
    // Map
    MapEmpty,
    MapRemoveOne,
    MapInsertKey, // NEW
```

### 4.2 Add the name arm

In `Operator::name`:

```rust
            Operator::MapInsertKey => "map_insert_key",
```

### 4.3 Add the type-class mapping

In `Operator::for_type`, the map arm:

```rust
            ("compound", n) if n.starts_with("map<") => {
                vec![Operator::MapEmpty, Operator::MapRemoveOne, Operator::MapInsertKey] // NEW
            }
```

### 4.4 Implement the mutation

In `apply_operator`:

```rust
        Operator::MapInsertKey => {
            let kt = t.type_args.first().ok_or_else(no_mutation)?;
            let vt = t.type_args.get(1).ok_or_else(no_mutation)?;
            let mut entries = match current {
                ScVal::Map(Some(m)) => m.0.to_vec(),
                _ => Vec::new(),
            };
            // Retry up to 100 times to find a key that is unique relative
            // to existing entries. If we cannot produce one, refuse rather
            // than silently overwriting an existing key.
            let max_attempts = 100;
            let (new_key, new_val) = (0..max_attempts)
                .find_map(|attempt| {
                    let key_bytes = rng.bytes(16);
                    let key = generate_value(kt, &key_bytes, 1, caps).ok()?;
                    if entries.iter().any(|e| e.key == key) {
                        return None; // collision → try again
                    }
                    let val_bytes = rng.bytes(16);
                    let val = generate_value(vt, &val_bytes, 1, caps).ok()?;
                    Some((key, val))
                })
                .ok_or_else(no_mutation)?;
            entries.push(ScMapEntry {
                key: new_key,
                val: new_val,
            });
            entries.sort_by(|a, b| a.key.cmp(&b.key));
            if entries.len() > caps.max_map_entries.saturating_mul(2).max(1) {
                return Err(no_mutation());
            }
            ScVal::Map(Some(ScMap(
                VecM::try_from(entries).map_err(|_| no_mutation())?,
            )))
        }
```

The uniqueness retry is the point of the example: a `map<K, V>` insertion that
collides with an existing key would silently *replace* that entry, so the
operator refuses (`NoApplicableMutation`) rather than produce a mutation that
does not do what its name claims. Note there is no `dedup_by` — dedup after a
push is what makes the guarantee unprovable, because the pushed entry is the
one that gets dropped.

Add `ScMapEntry` to the imports at the top of `mutation.rs`:

```rust
use soroban_env_host::xdr::{
    Int128Parts, ScBytes, ScMap, ScMapEntry, ScString, ScVal, ScVec, UInt128Parts, VecM,
};
```

### 4.5 Add the test

In `crates/sdkt-fuzz/tests/generator_mutation.rs`, append:

```rust
#[test]
fn map_insert_key_operator_is_deterministic_and_typed() {
    let map = compound(
        "map<u32, bool>".to_string(),
        vec![counter_type("u32"), counter_type("bool")],
    );
    // Multi-argument baseline: the map is argument 1, so arguments 0 and 2
    // are non-target. The sentinel is a value no mutation can produce, so
    // asserting it survives is a real check, not a coincidence.
    let baseline = vec![
        ScVal::U64(7),
        ScVal::Map(Some(ScMap(VecM::try_from(vec![]).unwrap()))),
        ScVal::Bool(true),
    ];
    let seed = [13u8; 32];

    let a = sdkt_fuzz::mutation::mutate_arg(
        &map,
        &baseline,
        1,
        sdkt_fuzz::Operator::MapInsertKey,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    let b = sdkt_fuzz::mutation::mutate_arg(
        &map,
        &baseline,
        1,
        sdkt_fuzz::Operator::MapInsertKey,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();

    // Determinism: same identity → identical mutation.
    assert_eq!(a, b);
    assert_eq!(a.mutation_id, b.mutation_id);

    // The result is still a well-typed map.
    match &a.value {
        ScVal::Map(Some(m)) => {
            assert_eq!(m.0.len(), 1, "insert into empty map yields one entry");
        }
        other => panic!("expected a map, got {other:?}"),
    }

    // Non-target arguments are preserved exactly, not just in count.
    assert_eq!(a.args.len(), baseline.len());
    assert_eq!(a.args[0], ScVal::U64(7));
    assert_eq!(a.args[2], ScVal::Bool(true));

    // Operator does not apply to a non-map type.
    let err = sdkt_fuzz::mutation::mutate_arg(
        &counter_type("u32"),
        &baseline,
        1,
        sdkt_fuzz::Operator::MapInsertKey,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::MutationError::OperatorNotApplicableForType { .. }
    ));
}
```

Two stronger regression tests accompany this snippet in the same file:

- `map_insert_key_guarantees_new_entry_on_non_empty_map` — inserts into a
  map that already has entries, asserts the length grows rather than stays
  flat, and asserts every pre-existing entry survives unchanged.
- `map_insert_key_refuses_when_key_space_is_exhausted` — `map<bool, bool>`
  with both keys present cannot accept a new unique key, so the operator
  returns `NoApplicableMutation` instead of overwriting.

The snippet above is the minimal illustration of the wiring; the two tests
below it are the ones that pin the non-target preservation and uniqueness
contracts from Section 1.5.

Add `ScMap`, `ScMapEntry`, `VecM` to the test imports:

```rust
use soroban_env_host::xdr::{ScMap, ScMapEntry, ScVal, VecM};
```

### 4.6 Run the test

```bash
cargo test -p sdkt-fuzz --test generator_mutation map_insert_key
```

---

## 5. Verification checklist

Before submitting a PR that extends the fuzz crate:

- [ ] Every symbol referenced in this guide exists in the source.
- [ ] `cargo test -p sdkt-fuzz` passes.
- [ ] The new operator or oracle condition has a test that pins its behavior.
- [ ] The test proves determinism (same identity → same result).
- [ ] The test proves non-target arguments are preserved (for operators).
- [ ] The test proves the "undeclared never produces findings" rule (for oracles).
- [ ] The workspace `README.md` or `CONTRIBUTING.md` links to this guide.

## 6. Known limitations / awkwardness

- **`Operator::for_type` is a single match.** Adding a new type class means
  adding a new arm to this match. The match is exhaustive, so the compiler
  will tell you if you miss one, but it is a central bottleneck.
- **`apply_operator` is a large match.** It will grow as operators are added.
  Consider splitting it into per-type-class functions if it becomes unwieldy.
- **The oracle is not composable.** Each condition is a separate `if` block in
  `Oracle::classify`. Adding a condition means editing this function. There is
  no plugin system for oracle rules.
- **No coverage model.** The crate deliberately does not track coverage, so a
  new operator does not automatically increase coverage metrics.
