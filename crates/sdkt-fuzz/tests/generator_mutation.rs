//! Phase 2 tests A–G: generator and mutation.

use sdkt_fuzz::{GenerationCaps, GeneratorError, UnsupportedContractType};
use sdkt_wasm::{parse_contract_spec, ContractSpec, ContractType};
use soroban_env_host::xdr::{ScMap, ScMapEntry, ScVal, VecM};

const COUNTER_WASM: &[u8] = include_bytes!("../../../crates/sdkt-cli/tests/fixtures/us_new.wasm");

fn counter_spec() -> ContractSpec {
    parse_contract_spec(COUNTER_WASM).expect("counter fixture carries a ContractSpec")
}

fn counter_type(name: &str) -> ContractType {
    // Build primitive ContractTypes the same way sdkt-wasm maps them.
    ContractType {
        name: name.to_string(),
        kind: "primitive".to_string(),
        doc: String::new(),
        members: vec![],
        type_args: vec![],
        bytes_n: None,
    }
}

fn compound(name: String, args: Vec<ContractType>) -> ContractType {
    ContractType {
        name,
        kind: "compound".to_string(),
        doc: String::new(),
        members: vec![],
        type_args: args,
        bytes_n: None,
    }
}

// ---------------------------------------------------------------------------
// A. generator determinism / D. compound type safety
// ---------------------------------------------------------------------------

#[test]
fn generator_deterministic_and_typed() {
    // Counter spec's functions carry no params, so type coverage is
    // exercised directly against parsed-style ContractType values.
    let _spec = counter_spec();

    for t in [
        counter_type("bool"),
        counter_type("u32"),
        counter_type("i32"),
        counter_type("u64"),
        counter_type("i64"),
        counter_type("u128"),
        counter_type("i128"),
        counter_type("string"),
        counter_type("bytes"),
        counter_type("address"),
        counter_type("muxed_address"),
    ] {
        let a =
            sdkt_fuzz::generator::generate_value(&t, b"k", 0, GenerationCaps::default()).unwrap();
        let b =
            sdkt_fuzz::generator::generate_value(&t, b"k", 0, GenerationCaps::default()).unwrap();
        assert_eq!(a, b, "{:?} must be deterministic", t.name);
        assert!(
            type_matches(&t, &a),
            "generated value must match type {:?}",
            t.name
        );
    }

    // Compounds: vec/option/map generate well-typed values.
    let vec_u32 = compound("vec<u32>".to_string(), vec![counter_type("u32")]);
    let a =
        sdkt_fuzz::generator::generate_value(&vec_u32, b"k", 0, GenerationCaps::default()).unwrap();
    let b =
        sdkt_fuzz::generator::generate_value(&vec_u32, b"k", 0, GenerationCaps::default()).unwrap();
    assert_eq!(a, b);
    assert!(type_matches(&vec_u32, &a));

    let opt_str = compound("option<string>".to_string(), vec![counter_type("string")]);
    let v =
        sdkt_fuzz::generator::generate_value(&opt_str, b"k", 0, GenerationCaps::default()).unwrap();
    assert!(type_matches(&opt_str, &v));

    let map = compound(
        "map<u32, bool>".to_string(),
        vec![counter_type("u32"), counter_type("bool")],
    );
    let v = sdkt_fuzz::generator::generate_value(&map, b"k", 0, GenerationCaps::default()).unwrap();
    assert!(type_matches(&map, &v));
}

fn type_matches(t: &ContractType, v: &ScVal) -> bool {
    match (t.kind.as_str(), t.name.as_str()) {
        ("primitive", "bool") => matches!(v, ScVal::Bool(_)),
        ("primitive", "u32") => matches!(v, ScVal::U32(_)),
        ("primitive", "i32") => matches!(v, ScVal::I32(_)),
        ("primitive", "u64") => matches!(v, ScVal::U64(_)),
        ("primitive", "i64") => matches!(v, ScVal::I64(_)),
        ("primitive", "u128") => matches!(v, ScVal::U128(_)),
        ("primitive", "i128") => matches!(v, ScVal::I128(_)),
        ("primitive", "string") => matches!(v, ScVal::String(_)),
        ("primitive", "bytes") => matches!(v, ScVal::Bytes(_)),
        ("primitive", "address") => matches!(v, ScVal::Address(_)),
        ("primitive", "muxed_address") => matches!(v, ScVal::Address(_)),
        ("compound", n) if n.starts_with("vec<") => matches!(v, ScVal::Vec(_)),
        ("compound", n) if n.starts_with("option<") => {
            matches!(v, ScVal::Void | ScVal::Vec(None))
                || !matches!(v, ScVal::Bool(_) | ScVal::U32(_))
        }
        ("compound", n) if n.starts_with("map<") => matches!(v, ScVal::Map(_)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// B. seed divergence
// ---------------------------------------------------------------------------

#[test]
fn different_seeds_diverge() {
    // Divergence across a spread of keys: same type, different seed streams.
    // (A fixed key pair could coincide by chance; require divergence on a
    // majority of sampled keys.)
    let t = counter_type("u32");
    let mut different = 0;
    for i in 0..16u64 {
        let a = sdkt_fuzz::generator::generate_value(
            &t,
            &i.to_le_bytes(),
            0,
            GenerationCaps::default(),
        )
        .unwrap();
        let b = sdkt_fuzz::generator::generate_value(
            &t,
            &(i + 1000).to_le_bytes(),
            0,
            GenerationCaps::default(),
        )
        .unwrap();
        if a != b {
            different += 1;
        }
    }
    assert!(
        different > 8,
        "different seeds must diverge for the vast majority of streams ({different}/16)"
    );

    // And per-call streams keyed by case_id diverge too.
    let seed = [3u8; 32];
    let a = sdkt_fuzz::generator::generate_call(
        &counter_spec(),
        "hello",
        &seed,
        "caseA",
        GenerationCaps::default(),
    )
    .unwrap();
    let b = sdkt_fuzz::generator::generate_call(
        &counter_spec(),
        "hello",
        &seed,
        "caseB",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_ne!(a.generation_id, b.generation_id);
}

// ---------------------------------------------------------------------------
// C. primitive type safety + generation through the real spec
// ---------------------------------------------------------------------------

#[test]
fn generated_call_matches_spec_arity() {
    let spec = counter_spec();
    // hello() and increment() take no parameters; generation must succeed
    // with an empty arg vector.
    let call = sdkt_fuzz::generator::generate_call(
        &spec,
        "increment",
        &[7u8; 32],
        "arity",
        GenerationCaps::default(),
    )
    .expect("increment is generatable");
    assert_eq!(call.function, "increment");
    assert!(call.args.is_empty());

    // Unknown function is an explicit error, not a panic.
    let err = sdkt_fuzz::generator::generate_call(
        &spec,
        "nope",
        &[7u8; 32],
        "arity",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(err, GeneratorError::FunctionNotFound { .. }));
}

// ---------------------------------------------------------------------------
// G. unsupported type handling
// ---------------------------------------------------------------------------

#[test]
fn unsupported_types_are_refused_not_panicked() {
    // tuple / result / bytesn / UDT kinds are outside the supported set.
    let tuple = compound("tuple<u32>".to_string(), vec![counter_type("u32")]);
    let err = sdkt_fuzz::generator::generate_value(&tuple, b"k", 0, GenerationCaps::default())
        .unwrap_err();
    assert_eq!(err.type_kind, "compound");

    let result_ty = compound(
        "result<u32, u32>".to_string(),
        vec![counter_type("u32"), counter_type("u32")],
    );
    assert!(
        sdkt_fuzz::generator::generate_value(&result_ty, b"k", 0, GenerationCaps::default())
            .is_err()
    );

    let bytesn = ContractType {
        name: "bytesn<4>".to_string(),
        kind: "compound".to_string(),
        doc: String::new(),
        members: vec![],
        type_args: vec![],
        bytes_n: Some(4),
    };
    assert!(
        sdkt_fuzz::generator::generate_value(&bytesn, b"k", 0, GenerationCaps::default()).is_err()
    );

    let udt = ContractType {
        name: "Point".to_string(),
        kind: "udt".to_string(),
        doc: String::new(),
        members: vec![],
        type_args: vec![],
        bytes_n: None,
    };
    let err =
        sdkt_fuzz::generator::generate_value(&udt, b"k", 0, GenerationCaps::default()).unwrap_err();
    assert_eq!(
        err,
        UnsupportedContractType {
            type_name: "Point".to_string(),
            type_kind: "udt".to_string()
        }
    );

    // selectability: a spec function over an unsupported type is skipped
    // with a reason, never silently.
    let us_old = parse_contract_spec(include_bytes!(
        "../../../crates/sdkt-cli/tests/fixtures/us_old.wasm"
    ))
    .unwrap();
    let (ok, skipped) = sdkt_fuzz::generator::selectable_functions(&us_old);
    // transfer(to: address) IS supported (address is in the set).
    assert!(ok.contains(&"transfer".to_string()));
    // mint(amt: u32) is supported; the UDT Point is unused by functions.
    assert!(ok.contains(&"mint".to_string()));
    assert!(skipped.is_empty(), "us_old functions are all supported");
}

// ---------------------------------------------------------------------------
// E. mutation determinism / F. non-target preservation
// ---------------------------------------------------------------------------

#[test]
fn mutation_deterministic_and_non_targets_preserved() {
    let t = counter_type("u32");
    let baseline = vec![ScVal::U32(5), ScVal::Bool(true), ScVal::U64(9)];
    let seed = [11u8; 32];

    let a = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        0,
        sdkt_fuzz::Operator::Random,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    let b = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        0,
        sdkt_fuzz::Operator::Random,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(a, b, "same (seed, case, arg, operator) must be identical");
    assert_eq!(a.mutation_id, b.mutation_id);

    // Non-target arguments must be byte-identical.
    assert_eq!(a.args[1], baseline[1]);
    assert_eq!(a.args[2], baseline[2]);
    assert_eq!(a.args.len(), baseline.len());

    // Boundary operators are exactly what they say.
    let zero = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        0,
        sdkt_fuzz::Operator::Zero,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(zero.value, ScVal::U32(0));
    let one = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        0,
        sdkt_fuzz::Operator::One,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(one.value, ScVal::U32(1));
    let max = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        0,
        sdkt_fuzz::Operator::Max,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(max.value, ScVal::U32(u32::MAX));

    // Bool toggle set.
    let bt = counter_type("bool");
    let true_v = sdkt_fuzz::mutation::mutate_arg(
        &bt,
        &baseline,
        1,
        sdkt_fuzz::Operator::BoolTrue,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(true_v.value, ScVal::Bool(true));
    let false_v = sdkt_fuzz::mutation::mutate_arg(
        &bt,
        &baseline,
        1,
        sdkt_fuzz::Operator::BoolFalse,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    assert_eq!(false_v.value, ScVal::Bool(false));

    // Operator/type mismatch is refused, never emitted.
    let err = sdkt_fuzz::mutation::mutate_arg(
        &bt,
        &baseline,
        1,
        sdkt_fuzz::Operator::Zero,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::MutationError::OperatorNotApplicableForType { .. }
    ));

    // Out-of-range index is refused.
    let err = sdkt_fuzz::mutation::mutate_arg(
        &t,
        &baseline,
        9,
        sdkt_fuzz::Operator::Zero,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::MutationError::ArgIndexOutOfRange { .. }
    ));

    // Unsupported type has no operators at all.
    let udt = ContractType {
        name: "Point".to_string(),
        kind: "udt".to_string(),
        doc: String::new(),
        members: vec![],
        type_args: vec![],
        bytes_n: None,
    };
    let err = sdkt_fuzz::mutation::mutate_arg(
        &udt,
        &baseline,
        0,
        sdkt_fuzz::Operator::Zero,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::MutationError::UnsupportedType { .. }
    ));

    // Vec operators: remove-one on empty is refused (no applicable mutation).
    let vec_u32 = compound("vec<u32>".to_string(), vec![counter_type("u32")]);
    let empty_vec = vec![ScVal::Vec(None)];
    let err = sdkt_fuzz::mutation::mutate_arg(
        &vec_u32,
        &empty_vec,
        0,
        sdkt_fuzz::Operator::VecRemoveOne,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        sdkt_fuzz::MutationError::NoApplicableMutation
    ));

    // Vec append works and stays typed.
    let appended = sdkt_fuzz::mutation::mutate_arg(
        &vec_u32,
        &empty_vec,
        0,
        sdkt_fuzz::Operator::VecAppend,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();
    match &appended.value {
        ScVal::Vec(Some(v)) => assert_eq!(v.0.len(), 1),
        other => panic!("expected a vec, got {other:?}"),
    }
}

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

#[test]
fn map_insert_key_guarantees_new_entry_on_non_empty_map() {
    // A non-empty map: the operator must add an entry, not replace one.
    let map = compound(
        "map<u32, bool>".to_string(),
        vec![counter_type("u32"), counter_type("bool")],
    );
    let existing = ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: ScVal::U32(1),
                val: ScVal::Bool(true),
            },
            ScMapEntry {
                key: ScVal::U32(2),
                val: ScVal::Bool(false),
            },
        ])
        .unwrap(),
    );
    // Multi-argument baseline: the map is argument 1, so arguments 0 and 2
    // are non-target and must survive untouched.
    let baseline = vec![ScVal::U64(7), ScVal::Map(Some(existing)), ScVal::Bool(true)];
    let seed = [29u8; 32];

    let m = sdkt_fuzz::mutation::mutate_arg(
        &map,
        &baseline,
        1,
        sdkt_fuzz::Operator::MapInsertKey,
        &seed,
        "m",
        GenerationCaps::default(),
    )
    .unwrap();

    // Non-target arguments are preserved exactly, not just in count.
    assert_eq!(m.args[0], ScVal::U64(7));
    assert_eq!(m.args[2], ScVal::Bool(true));

    let ScVal::Map(Some(after)) = &m.value else {
        panic!("expected a map, got {:?}", m.value);
    };
    assert_eq!(after.0.len(), 3, "one entry must be added");

    // The two pre-existing entries are still present, unchanged.
    for (key, val) in [
        (ScVal::U32(1), ScVal::Bool(true)),
        (ScVal::U32(2), ScVal::Bool(false)),
    ] {
        assert!(
            after.0.iter().any(|e| e.key == key && e.val == val),
            "pre-existing entry {key:?} => {val:?} was lost"
        );
    }

    // Keys stay unique (no silent overwrite).
    let keys: Vec<_> = after.0.iter().map(|e| e.key.clone()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(keys.len(), sorted.len(), "duplicate keys in mutated map");
}

#[test]
fn map_insert_key_refuses_when_key_space_is_exhausted() {
    // map<bool, bool> has exactly two possible keys. With both already
    // present, no unique key can be produced, so the operator must refuse
    // instead of overwriting.
    let map = compound(
        "map<bool, bool>".to_string(),
        vec![counter_type("bool"), counter_type("bool")],
    );
    let full = ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: ScVal::Bool(false),
                val: ScVal::Bool(true),
            },
            ScMapEntry {
                key: ScVal::Bool(true),
                val: ScVal::Bool(false),
            },
        ])
        .unwrap(),
    );
    let baseline = vec![ScVal::Map(Some(full))];

    let err = sdkt_fuzz::mutation::mutate_arg(
        &map,
        &baseline,
        0,
        sdkt_fuzz::Operator::MapInsertKey,
        &[41u8; 32],
        "m",
        GenerationCaps::default(),
    )
    .unwrap_err();
    assert!(
        matches!(err, sdkt_fuzz::MutationError::NoApplicableMutation),
        "expected NoApplicableMutation, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// H. Mutation planner tests (plan_mutation distinct steps & cycling)
// ---------------------------------------------------------------------------

#[test]
fn plan_mutation_consecutive_steps_differ_for_bool_and_u64() {
    let t_bool = counter_type("bool");
    let t_u64 = counter_type("u64");
    let seed = *b"sdkt-fuzz-phase1-default-seed!!!";

    // 200-case sweep asserting steps 0 and 1 select different operators in 100% of cases
    for (t, op_count) in [(&t_bool, 2), (&t_u64, 5)] {
        assert_eq!(sdkt_fuzz::mutation::operators_for(t).len(), op_count);
        let param_types = [t];
        let mut duplicates = 0;
        let cases = 200;
        for c in 0..cases {
            let case_id = format!("case{c:04}");
            let step0 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, 0)
                .expect("mutation plan exists");
            let step1 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, 1)
                .expect("mutation plan exists");
            assert_ne!(
                step0, step1,
                "step 0 and step 1 must differ for case {case_id} on type {}",
                t.name
            );
            if step0 == step1 {
                duplicates += 1;
            }
        }
        assert_eq!(
            duplicates, 0,
            "100% of sampled cases must select different operators between consecutive steps"
        );
    }
}

#[test]
fn plan_mutation_reaches_full_candidate_set_in_len_steps() {
    let t_bool = counter_type("bool");
    let t_u64 = counter_type("u64");
    let seed = *b"sdkt-fuzz-phase1-default-seed!!!";

    let test_cases: Vec<Vec<&ContractType>> =
        vec![vec![&t_bool], vec![&t_u64], vec![&t_bool, &t_u64]];

    for param_types in test_cases {
        let mut expected_candidates: Vec<(usize, sdkt_fuzz::Operator)> = Vec::new();
        for (i, t) in param_types.iter().enumerate() {
            for op in sdkt_fuzz::mutation::operators_for(t) {
                expected_candidates.push((i, op));
            }
        }
        let total = expected_candidates.len();

        for c in 0..50 {
            let case_id = format!("case{c:04}");
            let mut visited = std::collections::BTreeSet::new();
            for step in 0..total {
                let choice =
                    sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, step)
                        .expect("mutation plan exists");
                assert!(
                    visited.insert(choice),
                    "duplicate candidate {:?} visited at step {} for case {}",
                    choice,
                    step,
                    case_id
                );
            }
            assert_eq!(
                visited.len(),
                total,
                "across candidates.len() steps, every candidate must be selected exactly once"
            );
        }
    }
}

#[test]
fn plan_mutation_determinism_across_repeated_calls() {
    let t_u64 = counter_type("u64");
    let param_types = [&t_u64];
    let seed = *b"sdkt-fuzz-phase1-default-seed!!!";

    for c in 0..20 {
        let case_id = format!("case{c:04}");
        for step in 0..10 {
            let call1 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, step);
            let call2 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, step);
            assert_eq!(
                call1, call2,
                "plan_mutation must be deterministic across calls"
            );
        }
    }
}

#[test]
fn plan_mutation_single_candidate_repeats() {
    // A primitive type with a single fallback operator (Operator::Random)
    let t_single = counter_type("custom_opaque");
    assert_eq!(
        sdkt_fuzz::mutation::operators_for(&t_single),
        vec![sdkt_fuzz::Operator::Random]
    );
    let param_types = [&t_single];
    let seed = *b"sdkt-fuzz-phase1-default-seed!!!";

    for c in 0..10 {
        let case_id = format!("case{c:04}");
        let step0 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, 0);
        let step1 = sdkt_fuzz::mutation::plan_mutation(&param_types, &seed, &case_id, 1);
        assert_eq!(
            step0,
            Some((0, sdkt_fuzz::Operator::Random)),
            "single candidate must return Operator::Random"
        );
        assert_eq!(
            step0, step1,
            "single-candidate type is documented to repeat across consecutive steps"
        );
    }
}

#[test]
fn campaign_summary_reports_counts() {
    let result = sdkt_fuzz::CampaignResult {
        executed: 4,
        passed: vec!["case0000".into(), "case0001".into()],
        expected_errors: vec![],
        findings: vec![],
        skipped_functions: vec![],
    };
    let summary = sdkt_fuzz::campaign::summarize(&result);
    assert_eq!(
        summary,
        "executed=4 passed=2 expected_errors=0 findings=0 skipped=0"
    );
}
