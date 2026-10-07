//! Type-aware semantic mutation.
//!
//! Pipeline: ContractSpec type → baseline typed value → mutation operator →
//! mutated typed value → `ScVal`. There is **no** raw byte mutation and no
//! blind XDR mutation: operators are chosen from the value's own type, and a
//! mutation that cannot produce a well-typed value is rejected rather than
//! emitted.
//!
//! Determinism: a mutation's identity is
//! `(seed, case_id, arg_index, operator)`, and the same tuple always
//! produces the same mutation. Non-target arguments are carried through
//! byte-identical — the mutator returns the full new argument vector with
//! exactly one position changed.

use sha2::{Digest, Sha256};
use soroban_env_host::xdr::{
    Int128Parts, ScBytes, ScMap, ScString, ScVal, ScVec, UInt128Parts, VecM,
};

use sdkt_wasm::ContractType;

use crate::generator::{derive_key_child, generate_value, GenerationCaps, SplitMix64};

/// Production operator applied to one argument.
///
/// Operator sets are grouped by the type class they are valid for, so an
/// ill-typed mutation is structurally impossible to select.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operator {
    // Bool
    BoolTrue,
    BoolFalse,
    // Numeric boundary operators
    Zero,
    One,
    Min,
    Max,
    // Deterministic random value
    Random,
    // String / bytes
    Empty,
    LengthBoundary,
    // Vec
    VecEmpty,
    VecRemoveOne,
    VecDuplicate,
    VecAppend,
    VecMutateOne,
    // Option
    OptionNone,
    OptionSome,
    OptionSomeMutated,
    // Map
    MapEmpty,
    MapRemoveOne,
}

impl Operator {
    pub fn name(self) -> &'static str {
        match self {
            Operator::BoolTrue => "bool_true",
            Operator::BoolFalse => "bool_false",
            Operator::Zero => "zero",
            Operator::One => "one",
            Operator::Min => "min",
            Operator::Max => "max",
            Operator::Random => "random",
            Operator::Empty => "empty",
            Operator::LengthBoundary => "length_boundary",
            Operator::VecEmpty => "vec_empty",
            Operator::VecRemoveOne => "vec_remove_one",
            Operator::VecDuplicate => "vec_duplicate",
            Operator::VecAppend => "vec_append",
            Operator::VecMutateOne => "vec_mutate_one",
            Operator::OptionNone => "option_none",
            Operator::OptionSome => "option_some",
            Operator::OptionSomeMutated => "option_some_mutated",
            Operator::MapEmpty => "map_empty",
            Operator::MapRemoveOne => "map_remove_one",
        }
    }

    fn for_type(t: &ContractType) -> Vec<Operator> {
        match (t.kind.as_str(), t.name.as_str()) {
            ("primitive", "bool") => vec![Operator::BoolTrue, Operator::BoolFalse],
            ("primitive", n) if matches!(n, "u32" | "i32" | "u64" | "i64" | "u128" | "i128") => {
                vec![
                    Operator::Zero,
                    Operator::One,
                    Operator::Min,
                    Operator::Max,
                    Operator::Random,
                ]
            }
            ("primitive", "string") | ("primitive", "bytes") => {
                vec![Operator::Empty, Operator::LengthBoundary, Operator::Random]
            }
            ("primitive", _) => vec![Operator::Random],
            ("compound", n) if n.starts_with("vec<") => vec![
                Operator::VecEmpty,
                Operator::VecRemoveOne,
                Operator::VecDuplicate,
                Operator::VecAppend,
                Operator::VecMutateOne,
            ],
            ("compound", n) if n.starts_with("option<") => vec![
                Operator::OptionNone,
                Operator::OptionSome,
                Operator::OptionSomeMutated,
            ],
            ("compound", n) if n.starts_with("map<") => {
                vec![Operator::MapEmpty, Operator::MapRemoveOne]
            }
            _ => Vec::new(),
        }
    }
}

/// One applied mutation, with the identity needed to reproduce it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutation {
    /// `(seed, case_id, arg_index, operator)` digest — mutation identity.
    pub mutation_id: String,
    /// Index of the mutated argument.
    pub arg_index: usize,
    /// Operator that produced [`Mutation::value`].
    pub operator: Operator,
    /// The mutated argument.
    pub value: ScVal,
    /// Full argument vector: only `arg_index` differs from the baseline.
    pub args: Vec<ScVal>,
}

/// Compute a mutation's identity digest.
pub fn mutation_id(seed: &[u8; 32], case_id: &str, arg_index: usize, operator: Operator) -> String {
    let mut h = Sha256::new();
    h.update(seed);
    h.update(case_id.as_bytes());
    h.update([0xff]);
    h.update(arg_index.to_le_bytes());
    h.update(operator.name().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Mutation failures — always a refusal, never an ill-typed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationError {
    /// The argument index is outside the baseline vector.
    ArgIndexOutOfRange { arg_index: usize, arity: usize },
    /// The type has no operators in the supported set.
    UnsupportedType {
        type_name: String,
        type_kind: String,
    },
    /// The operator does not apply to this type.
    OperatorNotApplicableForType {
        operator: &'static str,
        type_name: String,
    },
    /// The operator applies but produced nothing usable for this value
    /// (e.g. `VecRemoveOne` on an already-empty vec). Not emitted.
    NoApplicableMutation,
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutationError::ArgIndexOutOfRange { arg_index, arity } => {
                write!(f, "arg index {arg_index} out of range for arity {arity}")
            }
            MutationError::UnsupportedType {
                type_name,
                type_kind,
            } => write!(
                f,
                "no mutation operators for type `{type_name}` (kind `{type_kind}`)"
            ),
            MutationError::OperatorNotApplicableForType {
                operator,
                type_name,
            } => write!(
                f,
                "operator `{operator}` does not apply to type `{type_name}`"
            ),
            MutationError::NoApplicableMutation => {
                write!(f, "operator produced no applicable mutation for this value")
            }
        }
    }
}

/// Operators available for `t`, in the crate's deterministic order.
pub fn operators_for(t: &ContractType) -> Vec<Operator> {
    Operator::for_type(t)
}

/// Apply `operator` to argument `arg_index` of `args`.
///
/// The returned [`Mutation::args`] is the baseline vector with exactly one
/// position replaced — every other element is byte-identical (the elements
/// are `Clone`, never regenerated).
pub fn mutate_arg(
    t: &ContractType,
    args: &[ScVal],
    arg_index: usize,
    operator: Operator,
    seed: &[u8; 32],
    case_id: &str,
    caps: GenerationCaps,
) -> Result<Mutation, MutationError> {
    if arg_index >= args.len() {
        return Err(MutationError::ArgIndexOutOfRange {
            arg_index,
            arity: args.len(),
        });
    }
    if !operators_for(t).contains(&operator) {
        return if operators_for(t).is_empty() {
            Err(MutationError::UnsupportedType {
                type_name: t.name.clone(),
                type_kind: t.kind.clone(),
            })
        } else {
            Err(MutationError::OperatorNotApplicableForType {
                operator: operator.name(),
                type_name: t.name.clone(),
            })
        };
    }

    let mut rng = SplitMix64::keyed(
        &[
            derive_key_child(seed, case_id),
            arg_index.to_le_bytes().to_vec(),
            operator.name().as_bytes().to_vec(),
        ]
        .concat(),
    );

    let new_value = apply_operator(t, &args[arg_index], operator, &mut rng, caps)?;

    let mut mutated = args.to_vec();
    mutated[arg_index] = new_value.clone();

    Ok(Mutation {
        mutation_id: mutation_id(seed, case_id, arg_index, operator),
        arg_index,
        operator,
        value: new_value,
        args: mutated,
    })
}

fn apply_operator(
    t: &ContractType,
    current: &ScVal,
    operator: Operator,
    rng: &mut SplitMix64,
    caps: GenerationCaps,
) -> Result<ScVal, MutationError> {
    let no_mutation = || MutationError::NoApplicableMutation;
    let mut random_value = || -> Result<ScVal, MutationError> {
        let key = rng.bytes(16);
        generate_value(t, &key, 0, caps).map_err(|_| no_mutation())
    };

    let out = match operator {
        Operator::BoolTrue => ScVal::Bool(true),
        Operator::BoolFalse => ScVal::Bool(false),

        Operator::Zero => numeric(t, 0, false).ok_or_else(no_mutation)?,
        Operator::One => numeric(t, 1, false).ok_or_else(no_mutation)?,
        Operator::Min => numeric(t, 0, true).ok_or_else(no_mutation)?,
        Operator::Max => numeric(t, u64::MAX, true).ok_or_else(no_mutation)?,
        Operator::Random => random_value()?,

        Operator::Empty => match t.name.as_str() {
            "string" => ScVal::String(ScString(
                StringM::try_from(Vec::new()).map_err(|_| no_mutation())?,
            )),
            "bytes" => ScVal::Bytes(ScBytes(
                soroban_env_host::xdr::BytesM::try_from(Vec::new()).map_err(|_| no_mutation())?,
            )),
            _ => return Err(no_mutation()),
        },
        Operator::LengthBoundary => {
            let len = caps.max_byte_len;
            let bytes = rng.bytes(len);
            match t.name.as_str() {
                "string" => ScVal::String(ScString(
                    StringM::try_from(bytes).map_err(|_| no_mutation())?,
                )),
                "bytes" => ScVal::Bytes(ScBytes(
                    soroban_env_host::xdr::BytesM::try_from(bytes).map_err(|_| no_mutation())?,
                )),
                _ => return Err(no_mutation()),
            }
        }

        Operator::VecEmpty => ScVal::Vec(None),
        Operator::VecRemoveOne => match current {
            ScVal::Vec(Some(v)) if !v.0.is_empty() => {
                let mut items = v.0.to_vec();
                items.pop();
                ScVal::Vec(Some(ScVec(
                    VecM::try_from(items).map_err(|_| no_mutation())?,
                )))
            }
            _ => return Err(no_mutation()),
        },
        Operator::VecDuplicate => match current {
            ScVal::Vec(Some(v)) if !v.0.is_empty() => {
                let mut items = v.0.to_vec();
                let last = items.last().cloned().ok_or_else(no_mutation)?;
                let at = rng.below(items.len() as u64) as usize;
                items.insert(at, last);
                items.truncate(caps.max_vec_len.saturating_mul(2).max(items.len()));
                ScVal::Vec(Some(ScVec(
                    VecM::try_from(items).map_err(|_| no_mutation())?,
                )))
            }
            _ => return Err(no_mutation()),
        },
        Operator::VecAppend => {
            let elem = t.type_args.first().ok_or_else(no_mutation)?;
            let key = rng.bytes(16);
            let v = generate_value(elem, &key, 1, caps).map_err(|_| no_mutation())?;
            let mut items = match current {
                ScVal::Vec(Some(v)) => v.0.to_vec(),
                _ => Vec::new(),
            };
            if items.len() >= caps.max_vec_len.saturating_mul(2).max(1) {
                return Err(no_mutation());
            }
            items.push(v);
            ScVal::Vec(Some(ScVec(
                VecM::try_from(items).map_err(|_| no_mutation())?,
            )))
        }
        Operator::VecMutateOne => {
            let elem = t.type_args.first().ok_or_else(no_mutation)?;
            let (items, idx, baseline) = match current {
                ScVal::Vec(Some(v)) if !v.0.is_empty() => {
                    let items = v.0.to_vec();
                    let baseline = items[0].clone();
                    let idx = rng.below(items.len() as u64) as usize;
                    (items, idx, baseline)
                }
                _ => return Err(no_mutation()),
            };
            let ops = operators_for(elem);
            let op = ops
                .get(rng.below(ops.len() as u64) as usize)
                .copied()
                .ok_or_else(no_mutation)?;
            let mut inner = SplitMix64::keyed(&rng.bytes(16));
            let target = items.get(idx).cloned().unwrap_or(baseline);
            let mutated = apply_operator(elem, &target, op, &mut inner, caps)?;
            let mut items = items;
            items[idx] = mutated;
            ScVal::Vec(Some(ScVec(
                VecM::try_from(items).map_err(|_| no_mutation())?,
            )))
        }

        Operator::OptionNone => ScVal::Void,
        Operator::OptionSome => {
            let elem = t.type_args.first().ok_or_else(no_mutation)?;
            let key = rng.bytes(16);
            generate_value(elem, &key, 1, caps).map_err(|_| no_mutation())?
        }
        Operator::OptionSomeMutated => {
            let elem = t.type_args.first().ok_or_else(no_mutation)?;
            let ops = operators_for(elem);
            let op = ops
                .get(rng.below(ops.len() as u64) as usize)
                .copied()
                .ok_or_else(no_mutation)?;
            let mut inner = SplitMix64::keyed(&rng.bytes(16));
            let base = generate_value(elem, &rng.bytes(16), 1, caps).map_err(|_| no_mutation())?;
            apply_operator(elem, &base, op, &mut inner, caps)?
        }

        Operator::MapEmpty => ScVal::Map(None),
        Operator::MapRemoveOne => match current {
            ScVal::Map(Some(m)) if !m.0.is_empty() => {
                let mut entries = m.0.to_vec();
                entries.pop();
                ScVal::Map(Some(ScMap(
                    VecM::try_from(entries).map_err(|_| no_mutation())?,
                )))
            }
            _ => return Err(no_mutation()),
        },
    };
    Ok(out)
}

fn numeric(t: &ContractType, magnitude: u64, boundary: bool) -> Option<ScVal> {
    let signed_extreme = |hi: i64, lo: u64| (hi, lo);
    Some(match t.name.as_str() {
        "u32" => ScVal::U32(if boundary && magnitude == u64::MAX {
            u32::MAX
        } else {
            magnitude as u32
        }),
        "i32" => ScVal::I32(if boundary && magnitude == u64::MAX {
            i32::MAX
        } else if boundary {
            i32::MIN
        } else {
            magnitude as i32
        }),
        "u64" => ScVal::U64(magnitude),
        "i64" => ScVal::I64(if boundary && magnitude == u64::MAX {
            i64::MAX
        } else if boundary {
            i64::MIN
        } else {
            magnitude as i64
        }),
        "u128" => {
            let (hi, lo) = if boundary && magnitude == u64::MAX {
                (u64::MAX, u64::MAX)
            } else {
                (0, magnitude)
            };
            ScVal::U128(UInt128Parts { hi, lo })
        }
        "i128" => {
            let (hi, lo) = if boundary && magnitude == u64::MAX {
                signed_extreme(i64::MAX, u64::MAX)
            } else if boundary {
                signed_extreme(i64::MIN, 0)
            } else {
                signed_extreme(0, magnitude)
            };
            ScVal::I128(Int128Parts { hi, lo })
        }
        _ => return None,
    })
}

use soroban_env_host::xdr::StringM;

/// Deterministically pick the next mutation for a function's arguments:
/// cycles `(arg_index, operator)` in a stable order derived from the seed
/// and case id.
pub fn plan_mutation(
    param_types: &[&ContractType],
    seed: &[u8; 32],
    case_id: &str,
    step: usize,
) -> Option<(usize, Operator)> {
    let mut candidates: Vec<(usize, Operator)> = Vec::new();
    for (i, t) in param_types.iter().enumerate() {
        for op in operators_for(t) {
            candidates.push((i, op));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let mut rng =
        SplitMix64::keyed(&[derive_key_child(seed, case_id), step.to_le_bytes().to_vec()].concat());
    // Deterministic order: sample a permutation start then walk forward.
    let start = rng.below(candidates.len() as u64) as usize;
    Some(candidates[(start + step) % candidates.len()])
}
