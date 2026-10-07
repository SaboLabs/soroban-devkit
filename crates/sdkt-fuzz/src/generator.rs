//! Deterministic typed input generation from `sdkt-wasm` ContractSpec.
//!
//! Pipeline: WASM → [`sdkt_wasm::parse_contract_spec`] (single parser, the
//! workspace source of truth — this crate never parses WASM itself) →
//! function selection → typed [`ScVal`] generation.
//!
//! Determinism contract: identical `(wasm, spec, seed, case_id, function,
//! arg_index)` yields byte-identical typed input. There is no global RNG
//! state: every value derives its PRNG stream from
//! `SplitMix64(sha256(seed ‖ labels))`, keyed by seed + case + function +
//! position.
//!
//! Unsupported types never panic and never yield ill-typed `ScVal`: they
//! return [`GeneratorError::UnsupportedContractType`] (function-level
//! generation) or [`UnsupportedContractType`] (value-level), and the
//! campaign skips such functions with an explicit reason.

use sha2::{Digest, Sha256};
use soroban_env_host::xdr::{
    AccountId, Int128Parts, MuxedEd25519Account, PublicKey, ScAddress, ScBytes, ScMap, ScMapEntry,
    ScString, ScVal, ScVec, StringM, UInt128Parts, Uint256, VecM,
};

use sdkt_wasm::{ContractFunction, ContractSpec, ContractType};

/// Generation caps. Depth/size bounds keep generation terminating and
/// XDR-sized; they are part of the deterministic contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationCaps {
    /// Max compound nesting depth (outermost parameter = depth 0).
    pub max_depth: u32,
    /// Max elements in a generated `Vec`.
    pub max_vec_len: usize,
    /// Max bytes in a generated `String` / `Bytes`.
    pub max_byte_len: usize,
    /// Max entries in a generated `Map`.
    pub max_map_entries: usize,
}

impl Default for GenerationCaps {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_vec_len: 4,
            max_byte_len: 32,
            max_map_entries: 4,
        }
    }
}

/// A ContractType this generator cannot produce values for.
///
/// Explicit, never a panic and never a silently-wrong value. `type_name` /
/// `type_kind` come straight from `sdkt-wasm`'s parsed model so callers can
/// report exactly what was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedContractType {
    pub type_name: String,
    pub type_kind: String,
}

impl std::fmt::Display for UnsupportedContractType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unsupported contract type `{}` (kind `{}`)",
            self.type_name, self.type_kind
        )
    }
}

/// Function-level generation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneratorError {
    /// The requested function is not in the parsed spec.
    FunctionNotFound { function: String },
    /// A parameter's ContractType is outside the supported set.
    Unsupported(UnsupportedContractType),
}

impl std::fmt::Display for GeneratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeneratorError::FunctionNotFound { function } => {
                write!(f, "function `{function}` not found in ContractSpec")
            }
            GeneratorError::Unsupported(u) => write!(f, "{u}"),
        }
    }
}

/// A generated call: function name + typed arguments + stable identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedCall {
    pub function: String,
    /// Typed arguments, positional (index = parameter index).
    pub args: Vec<ScVal>,
    /// Deterministic identity string of this generation
    /// (`seed ‖ case_id ‖ function`). Used to key PRNG streams; safe to log.
    pub generation_id: String,
}

/// Deterministic counter-based PRNG (SplitMix64).
///
/// Not cryptographically secure — it does not need to be. It needs to be
/// stable across processes, machines, and Rust versions: SplitMix64's
/// arithmetic is fully specified by `u64` wrapping ops.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// Key a stream from arbitrary bytes (SHA-256 truncated to 64 bits).
    pub fn keyed(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut word = [0u8; 8];
        word.copy_from_slice(&digest[..8]);
        Self(u64::from_le_bytes(word))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish value in `0..bound` (bound > 0).
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            out.extend_from_slice(&self.next_u64().to_le_bytes());
        }
        out.truncate(len);
        out
    }
}

/// Derive the per-(seed, labels...) PRNG key.
pub fn derive_key(seed: &[u8; 32], labels: &[&str]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(seed);
    for l in labels {
        h.update(l.as_bytes());
        h.update(&[0xff]); // label separator: ("ab","c") != ("a","bc")
    }
    h.finalize().to_vec()
}

/// The parameter types a [`ContractFunction`] accepts, in order.
pub fn function_param_types<'a>(f: &'a ContractFunction) -> Vec<&'a ContractType> {
    f.parameters.iter().map(|p| &p.type_).collect()
}

/// True when this ContractType (recursively) is inside the supported set.
pub fn type_supported(t: &ContractType) -> bool {
    match t.kind.as_str() {
        "primitive" => matches!(
            t.name.as_str(),
            "bool"
                | "u32"
                | "i32"
                | "u64"
                | "i64"
                | "u128"
                | "i128"
                | "string"
                | "bytes"
                | "address"
                | "muxed_address"
        ),
        "compound" => {
            if t.name.starts_with("vec<") || t.name.starts_with("option<") {
                t.type_args.first().map(type_supported).unwrap_or(false)
            } else if t.name.starts_with("map<") {
                t.type_args.len() == 2 && t.type_args.iter().all(type_supported)
            } else {
                false // tuple/result/bytesn: not in the supported set
            }
        }
        _ => false, // struct/union/enum/error_enum/udt: not in the supported set
    }
}

/// Generate one typed value for `t`. Deterministic for `(key, t, depth)`.
///
/// Returns [`UnsupportedContractType`] (never a panic, never an ill-typed
/// `ScVal`) for anything outside the supported set.
pub fn generate_value(
    t: &ContractType,
    key: &[u8],
    depth: u32,
    caps: GenerationCaps,
) -> Result<ScVal, UnsupportedContractType> {
    let unsupported = || UnsupportedContractType {
        type_name: t.name.clone(),
        type_kind: t.kind.clone(),
    };

    if depth > caps.max_depth {
        // Depth cap: only recurse-safe types may bottom out.
        return match t.kind.as_str() {
            "primitive" if t.name == "bool" => Ok(ScVal::Bool(false)),
            "compound" if t.name.starts_with("option<") => Ok(ScVal::Void),
            "compound" if t.name.starts_with("vec<") => Ok(ScVal::Vec(None)),
            "compound" if t.name.starts_with("map<") => Ok(ScVal::Map(None)),
            _ => Err(unsupported()),
        };
    }

    let mut rng = SplitMix64::keyed(&[key, &depth.to_le_bytes()].concat());

    match t.kind.as_str() {
        "primitive" => generate_primitive(&t.name, &mut rng, caps).ok_or_else(unsupported),
        "compound" => {
            if let Some(inner) = t.name.strip_prefix("vec<") {
                let _ = inner;
                let elem = t.type_args.first().ok_or_else(unsupported)?;
                let n = if caps.max_vec_len == 0 {
                    0
                } else {
                    rng.below(caps.max_vec_len as u64 + 1) as usize
                };
                let mut items = Vec::with_capacity(n);
                for i in 0..n {
                    let child_key = derive_key_child(key, &format!("v{i}"));
                    items.push(generate_value(elem, &child_key, depth + 1, caps)?);
                }
                let vm = VecM::try_from(items).map_err(|_| unsupported())?;
                return Ok(ScVal::Vec(Some(ScVec(vm))));
            }
            if t.name.starts_with("option<") {
                let elem = t.type_args.first().ok_or_else(unsupported)?;
                return if rng.below(2) == 0 {
                    Ok(ScVal::Void) // None
                } else {
                    let child_key = derive_key_child(key, "opt");
                    generate_value(elem, &child_key, depth + 1, caps)
                };
            }
            if t.name.starts_with("map<") {
                let kt = t.type_args.first().ok_or_else(unsupported)?;
                let vt = t.type_args.get(1).ok_or_else(unsupported)?;
                let n = if caps.max_map_entries == 0 {
                    0
                } else {
                    rng.below(caps.max_map_entries as u64 + 1) as usize
                };
                let mut entries: Vec<ScMapEntry> = Vec::with_capacity(n);
                for i in 0..n {
                    let kkey = derive_key_child(key, &format!("mk{i}"));
                    let vkey = derive_key_child(key, &format!("mv{i}"));
                    let k = generate_value(kt, &kkey, depth + 1, caps)?;
                    let v = generate_value(vt, &vkey, depth + 1, caps)?;
                    entries.push(ScMapEntry { key: k, val: v });
                }
                // Host requires sorted-by-key maps for ScVal::Map in most
                // paths; sort deterministically (ScVal: Ord).
                entries.sort_by(|a, b| a.key.cmp(&b.key));
                entries.dedup_by(|a, b| a.key == b.key);
                let vm = VecM::try_from(entries).map_err(|_| unsupported())?;
                return Ok(ScVal::Map(Some(ScMap(vm))));
            }
            Err(unsupported())
        }
        _ => Err(unsupported()),
    }
}

/// Derive a child key from a parent key + sub-label (stream separation).
pub fn derive_key_child(parent_key: &[u8], label: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(parent_key);
    h.update(label.as_bytes());
    h.finalize().to_vec()
}

fn generate_primitive(name: &str, rng: &mut SplitMix64, caps: GenerationCaps) -> Option<ScVal> {
    let v = match name {
        "bool" => ScVal::Bool(rng.below(2) == 1),
        "u32" => ScVal::U32(rng.next_u64() as u32),
        "i32" => ScVal::I32(rng.next_u64() as i32),
        "u64" => ScVal::U64(rng.next_u64()),
        "i64" => ScVal::I64(rng.next_u64() as i64),
        "u128" => ScVal::U128(UInt128Parts {
            hi: rng.next_u64(),
            lo: rng.next_u64(),
        }),
        "i128" => ScVal::I128(Int128Parts {
            hi: rng.next_u64() as i64,
            lo: rng.next_u64(),
        }),
        "string" => {
            // Printable ASCII subset, deterministic length 0..=cap.
            let len = rng.below(caps.max_byte_len as u64 + 1) as usize;
            let bytes: Vec<u8> = rng.bytes(len).iter().map(|b| 0x20 + (b % 0x5f)).collect();
            let sm = StringM::try_from(bytes).ok()?;
            ScVal::String(ScString(sm))
        }
        "bytes" => {
            let len = rng.below(caps.max_byte_len as u64 + 1) as usize;
            let bm = soroban_env_host::xdr::BytesM::try_from(rng.bytes(len)).ok()?;
            ScVal::Bytes(ScBytes(bm))
        }
        "address" => {
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&rng.bytes(32));
            ScVal::Address(ScAddress::Account(AccountId(
                PublicKey::PublicKeyTypeEd25519(Uint256(pk)),
            )))
        }
        "muxed_address" => {
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&rng.bytes(32));
            ScVal::Address(ScAddress::MuxedAccount(MuxedEd25519Account {
                id: rng.next_u64(),
                ed25519: Uint256(pk),
            }))
        }
        _ => return None,
    };
    Some(v)
}

/// Generate all arguments for `function` from a parsed spec.
pub fn generate_call(
    spec: &ContractSpec,
    function: &str,
    seed: &[u8; 32],
    case_id: &str,
    caps: GenerationCaps,
) -> Result<GeneratedCall, GeneratorError> {
    let f = spec
        .functions
        .iter()
        .find(|f| f.name == function)
        .ok_or_else(|| GeneratorError::FunctionNotFound {
            function: function.to_string(),
        })?;

    let generation_id = {
        let key = derive_key(seed, &[case_id, function]);
        hex(&key)
    };
    let root_key = derive_key(seed, &[case_id, function, "args"]);

    let mut args = Vec::with_capacity(f.parameters.len());
    for (i, p) in f.parameters.iter().enumerate() {
        if !type_supported(&p.type_) {
            return Err(GeneratorError::Unsupported(UnsupportedContractType {
                type_name: p.type_.name.clone(),
                type_kind: p.type_.kind.clone(),
            }));
        }
        let arg_key = derive_key_child(&root_key, &format!("arg{i}"));
        let v = generate_value(&p.type_, &arg_key, 0, caps).map_err(GeneratorError::Unsupported)?;
        args.push(v);
    }

    Ok(GeneratedCall {
        function: function.to_string(),
        args,
        generation_id,
    })
}

/// Look up one function, returning an explicit skip reason when its
/// parameters leave the supported set.
pub fn select_function_or_skip<'a>(
    spec: &'a ContractSpec,
    function: &str,
) -> Option<Result<&'a ContractFunction, UnsupportedContractType>> {
    let f = spec.functions.iter().find(|f| f.name == function)?;
    match f.parameters.iter().find(|p| !type_supported(&p.type_)) {
        None => Some(Ok(f)),
        Some(p) => Some(Err(UnsupportedContractType {
            type_name: p.type_.name.clone(),
            type_kind: p.type_.kind.clone(),
        })),
    }
}

/// Select functions whose full parameter lists are supported, in spec order.
///
/// Skipped functions are reported with an explicit reason — never silently
/// dropped.
pub fn selectable_functions(spec: &ContractSpec) -> (Vec<String>, Vec<(String, String)>) {
    let mut ok = Vec::new();
    let mut skipped = Vec::new();
    for f in &spec.functions {
        match f.parameters.iter().find(|p| !type_supported(&p.type_)) {
            None => ok.push(f.name.clone()),
            Some(p) => skipped.push((
                f.name.clone(),
                format!(
                    "unsupported parameter `{}` (kind `{}`)",
                    p.type_.name, p.type_.kind
                ),
            )),
        }
    }
    (ok, skipped)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
