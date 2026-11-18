//! SEP-41 fungible-token classification from a contract spec.
//!
//! A deployed contract is classified as a token purely from its declared
//! interface ([`ContractSpec`]): each of the eleven SEP-41 methods counts
//! as present only when its name matches **and** its parameter count and
//! parameter types match the expected SEP-41 signature. A function with
//! the right name but the wrong signature is reported as
//! [`MethodPresence::SignatureMismatch`] — not present, not absent.
//!
//! Presence is determined from the spec only, never from a simulation
//! result. In particular `balance`, `allowance`, and every state-changing
//! method (`transfer`, `transfer_from`, `approve`, `burn`, `mint`) are
//! never simulated: they need address arguments or authorization, so a
//! simulation failure would say nothing about whether the method exists.

use serde::{Deserialize, Serialize};

use crate::spec::{ContractFunction, ContractSpec};

/// How one SEP-41 method shows up in a contract spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodPresence {
    /// Name matches and parameter count + types match the SEP-41 signature.
    Present,
    /// No function with this name is declared.
    Absent,
    /// A function with this name exists but its parameter count or types
    /// differ from the SEP-41 signature.
    SignatureMismatch,
}

impl MethodPresence {
    pub fn as_str(self) -> &'static str {
        match self {
            MethodPresence::Present => "present",
            MethodPresence::Absent => "absent",
            MethodPresence::SignatureMismatch => "signature_mismatch",
        }
    }
}

/// One SEP-41 method: its name and expected parameter types, in order.
///
/// Type names use the [`ContractType::name`] vocabulary (`"address"`,
/// `"i128"`, `"u32"`, `"string"`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sep41Method {
    /// Method name, e.g. `"transfer_from"`.
    pub name: &'static str,
    /// Expected parameter types in order, e.g. `&["address", "address", "i128"]`.
    pub params: &'static [&'static str],
}

/// The full SEP-41 fungible-token interface, in canonical order.
///
/// Signatures follow the Soroban token interface (SEP-41): account
/// parameters are `Address`, amounts are `i128`, and `approve` takes an
/// `expiration_ledger: u32`.
pub const SEP41_METHODS: &[Sep41Method] = &[
    Sep41Method {
        name: "name",
        params: &[],
    },
    Sep41Method {
        name: "symbol",
        params: &[],
    },
    Sep41Method {
        name: "decimals",
        params: &[],
    },
    Sep41Method {
        name: "total_supply",
        params: &[],
    },
    Sep41Method {
        name: "balance",
        params: &["address"],
    },
    Sep41Method {
        name: "allowance",
        params: &["address", "address"],
    },
    Sep41Method {
        name: "transfer",
        params: &["address", "address", "i128"],
    },
    Sep41Method {
        name: "transfer_from",
        params: &["address", "address", "address", "i128"],
    },
    Sep41Method {
        name: "approve",
        params: &["address", "address", "i128", "u32"],
    },
    Sep41Method {
        name: "burn",
        params: &["address", "i128"],
    },
    Sep41Method {
        name: "mint",
        params: &["address", "i128"],
    },
];

/// The four metadata methods whose joint presence defines a token.
pub const METADATA_METHODS: &[&str] = &["name", "symbol", "decimals", "total_supply"];

/// Classify one SEP-41 method against a contract spec.
pub fn classify_method(spec: &ContractSpec, method: &Sep41Method) -> MethodPresence {
    match spec.functions.iter().find(|f| f.name == method.name) {
        None => MethodPresence::Absent,
        Some(f) => {
            if method_matches(f, method) {
                MethodPresence::Present
            } else {
                MethodPresence::SignatureMismatch
            }
        }
    }
}

/// Classify every SEP-41 method, in [`SEP41_METHODS`] order.
pub fn classify_all(spec: &ContractSpec) -> Vec<(Sep41Method, MethodPresence)> {
    SEP41_METHODS
        .iter()
        .map(|m| (*m, classify_method(spec, m)))
        .collect()
}

/// `true` only when `name`, `symbol`, `decimals`, and `total_supply` are
/// all present with their expected signatures.
pub fn is_token(spec: &ContractSpec) -> bool {
    METADATA_METHODS.iter().all(|name| {
        SEP41_METHODS
            .iter()
            .find(|m| m.name == *name)
            .is_some_and(|m| classify_method(spec, m) == MethodPresence::Present)
    })
}

fn method_matches(f: &ContractFunction, method: &Sep41Method) -> bool {
    f.parameters.len() == method.params.len()
        && f.parameters
            .iter()
            .zip(method.params.iter())
            .all(|(p, expected)| p.type_.name == *expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ContractParameter, ContractType};

    fn func(name: &str, params: &[&str]) -> ContractFunction {
        ContractFunction {
            name: name.to_string(),
            doc: String::new(),
            parameters: params
                .iter()
                .enumerate()
                .map(|(i, t)| ContractParameter {
                    name: format!("arg{i}"),
                    doc: String::new(),
                    type_: ContractType {
                        name: (*t).to_string(),
                        kind: "primitive".to_string(),
                        doc: String::new(),
                        members: vec![],
                        type_args: vec![],
                        bytes_n: None,
                    },
                })
                .collect(),
            outputs: vec![],
        }
    }

    fn spec(functions: Vec<ContractFunction>) -> ContractSpec {
        ContractSpec {
            env_meta: None,
            functions,
            custom_types: vec![],
            events: vec![],
        }
    }

    fn token_spec() -> ContractSpec {
        spec(
            SEP41_METHODS
                .iter()
                .map(|m| func(m.name, m.params))
                .collect(),
        )
    }

    #[test]
    fn token_spec_reports_all_methods_present_and_is_token() {
        let spec = token_spec();
        for (method, presence) in classify_all(&spec) {
            assert_eq!(
                presence,
                MethodPresence::Present,
                "method {} should be present",
                method.name
            );
        }
        assert!(is_token(&spec));
    }

    #[test]
    fn empty_spec_reports_all_absent_and_not_a_token() {
        let spec = spec(vec![]);
        for (method, presence) in classify_all(&spec) {
            assert_eq!(
                presence,
                MethodPresence::Absent,
                "method {} should be absent",
                method.name
            );
        }
        assert!(!is_token(&spec));
    }

    #[test]
    fn wrong_param_type_is_signature_mismatch_not_present() {
        // `decimals` taking one address argument: right name, wrong signature.
        let spec = spec(vec![func("decimals", &["address"])]);
        assert_eq!(
            classify_method(
                &spec,
                SEP41_METHODS.iter().find(|m| m.name == "decimals").unwrap()
            ),
            MethodPresence::SignatureMismatch
        );
        assert!(!is_token(&spec));
    }

    #[test]
    fn wrong_param_count_is_signature_mismatch() {
        // `transfer` with two parameters instead of three.
        let spec = spec(vec![func("transfer", &["address", "address"])]);
        assert_eq!(
            classify_method(
                &spec,
                SEP41_METHODS.iter().find(|m| m.name == "transfer").unwrap()
            ),
            MethodPresence::SignatureMismatch
        );
    }

    #[test]
    fn missing_one_metadata_method_is_not_a_token() {
        // Everything except `symbol`.
        let mut spec = token_spec();
        spec.functions.retain(|f| f.name != "symbol");
        assert!(!is_token(&spec));
        assert_eq!(
            classify_method(
                &spec,
                SEP41_METHODS.iter().find(|m| m.name == "symbol").unwrap()
            ),
            MethodPresence::Absent
        );
    }

    #[test]
    fn metadata_method_with_wrong_signature_is_not_a_token() {
        // `name` present but taking a parameter: wrong signature.
        let mut spec = token_spec();
        for f in spec.functions.iter_mut() {
            if f.name == "name" {
                f.parameters.push(ContractParameter {
                    name: "x".to_string(),
                    doc: String::new(),
                    type_: ContractType {
                        name: "u32".to_string(),
                        kind: "primitive".to_string(),
                        doc: String::new(),
                        members: vec![],
                        type_args: vec![],
                        bytes_n: None,
                    },
                });
            }
        }
        assert!(!is_token(&spec));
    }
}
