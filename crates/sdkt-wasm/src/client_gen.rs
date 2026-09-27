//! Minimal typed Rust client generation from a parsed [`ContractSpec`].
//!
//! `generate_client` turns the *supported subset* of a contract interface into
//! deterministic, dependency-free Rust source. Each function becomes a
//! `*Call` struct carrying its name, typed parameters, and an `args()` method
//! that encodes the call as `TYPE:VALUE` strings — the exact convention
//! [`sdkt call` / `sdkt invoke`] already accept.
//!
//! Scope is intentionally narrow (maintainer core):
//! - Supported parameter/return types: the primitive scalar subset listed in
//!   [`map_scalar`].
//! - Everything else (UDT, Option, Result, Vec, Map, Tuple, BytesN, Val,
//!   unclassified primitives) fails with a clear
//!   [`ClientGenError::UnsupportedType`].
//! - No network access, no template system, no plugins.

use crate::spec::{ContractFunction, ContractSpec, ContractType};
use std::fmt::Write as _;

/// Errors that abort client generation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClientGenError {
    /// The interface references a type outside the supported subset.
    #[error(
        "unsupported type '{type_name}' ({where_}) in function `{function}` — \
         the generator core supports u32|i32|u64|i64|bool|address|string|symbol|void only"
    )]
    UnsupportedType {
        function: String,
        type_name: String,
        where_: String,
    },
}

/// Map a supported primitive `ContractType` to its Rust representation, or
/// `None` when the type is outside the core subset.
fn map_scalar(t: &ContractType) -> Option<&'static str> {
    if t.kind != "primitive" {
        return None;
    }
    match t.name.as_str() {
        "u32" => Some("u32"),
        "i32" => Some("i32"),
        "u64" => Some("u64"),
        "i64" => Some("i64"),
        "bool" => Some("bool"),
        "address" | "string" | "symbol" => Some("String"),
        "void" => Some("()"),
        _ => None,
    }
}

/// Encode a supported scalar as the `TYPE:` prefix for `sdkt` typed args.
fn scalar_type_tag(t: &ContractType) -> Option<&'static str> {
    match t.name.as_str() {
        "u32" => Some("u32"),
        "i32" => Some("i32"),
        "u64" => Some("u64"),
        "i64" => Some("i64"),
        "bool" => Some("bool"),
        "address" => Some("address"),
        "string" => Some("string"),
        "symbol" => Some("symbol"),
        _ => None,
    }
}

/// Sanitize a spec name into a valid Rust identifier fragment.
fn rust_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.extend(ch.to_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

/// Pascal-case a spec name for the generated struct type.
fn struct_ident(fn_name: &str) -> String {
    let mut out = String::new();
    let mut upper_next = true;
    for ch in fn_name.chars() {
        if ch == '_' || !ch.is_ascii_alphanumeric() {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
        upper_next = false;
    }
    out.push_str("Call");
    out
}

/// Validate a function against the supported subset.
fn check_supported(f: &ContractFunction) -> Result<(), ClientGenError> {
    for p in &f.parameters {
        if map_scalar(&p.type_).is_none() {
            return Err(ClientGenError::UnsupportedType {
                function: f.name.clone(),
                type_name: format!("{}:{}", p.type_.kind, p.type_.name),
                where_: format!("parameter `{}`", p.name),
            });
        }
    }
    for o in &f.outputs {
        if map_scalar(o).is_none() {
            return Err(ClientGenError::UnsupportedType {
                function: f.name.clone(),
                type_name: format!("{}:{}", o.kind, o.name),
                where_: "return value".to_string(),
            });
        }
    }
    if f.outputs.len() > 1 {
        return Err(ClientGenError::UnsupportedType {
            function: f.name.clone(),
            type_name: format!("multiple returns ({})", f.outputs.len()),
            where_: "return value".to_string(),
        });
    }
    Ok(())
}

/// Options controlling client generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerateOptions {
    /// When true, functions with unsupported parameter or return types
    /// are omitted from the generated client instead of aborting generation.
    pub skip_unsupported: bool,
}

/// Generate a deterministic Rust client module from a parsed ContractSpec using default options.
pub fn generate_client(spec: &ContractSpec) -> Result<String, ClientGenError> {
    generate_client_with_options(spec, &GenerateOptions::default())
}

/// Generate a deterministic Rust client module from a parsed ContractSpec with custom [`GenerateOptions`].
pub fn generate_client_with_options(
    spec: &ContractSpec,
    options: &GenerateOptions,
) -> Result<String, ClientGenError> {
    let mut supported = Vec::new();
    let mut skipped = Vec::new();

    for f in &spec.functions {
        match check_supported(f) {
            Ok(()) => supported.push(f),
            Err(err) => {
                if options.skip_unsupported {
                    skipped.push((f, err));
                } else {
                    return Err(err);
                }
            }
        }
    }

    let mut out = String::new();
    out.push_str("// Generated by `sdkt generate client`. Do not edit by hand.\n");
    out.push_str("//\n");
    out.push_str("// Each `*Call` struct encodes one contract function as `TYPE:VALUE`\n");
    out.push_str("// argument strings accepted by `sdkt call` and `sdkt invoke`.\n");

    if !skipped.is_empty() {
        out.push_str("//\n");
        out.push_str("// Skipped unsupported functions:\n");
        for (f, err) in &skipped {
            let _ = writeln!(out, "// - {}: {}", f.name, err);
        }
    }
    out.push('\n');

    if spec.functions.is_empty() {
        out.push_str("/// This contract exposes no callable functions.\n");
        out.push_str("pub fn contract_functions() -> &'static [&'static str] { &[] }\n");
        return Ok(out);
    }

    if supported.is_empty() {
        out.push_str(
            "/// No callable functions were generated (all functions use unsupported types).\n",
        );
        out.push_str("pub fn contract_functions() -> &'static [&'static str] { &[] }\n");
        return Ok(out);
    }

    out.push_str("/// Names of all generated contract calls, in spec order.\n");
    out.push_str("pub fn contract_functions() -> &'static [&'static str] {\n    &[");
    for (i, f) in supported.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "\"{}\"", f.name);
    }
    out.push_str("]\n}\n\n");

    for f in &supported {
        let struct_name = struct_ident(&f.name);
        let out_ty = f
            .outputs
            .first()
            .map(|o| map_scalar(o).unwrap_or("()"))
            .unwrap_or("()");
        let alias_name = format!("{}Output", struct_name.trim_end_matches("Call"));

        if !f.doc.trim().is_empty() {
            let _ = writeln!(out, "/// {} — {}", f.name, f.doc.trim().replace('\n', " "));
        } else {
            let _ = writeln!(out, "/// Typed call builder for `{}`.", f.name);
        }
        let _ = writeln!(out, "pub struct {} {{", struct_name);
        for p in &f.parameters {
            let rust_ty = map_scalar(&p.type_).expect("validated above");
            let field = rust_ident(&p.name);
            if !p.doc.trim().is_empty() {
                let _ = writeln!(
                    out,
                    "    /// {} — {}",
                    p.name,
                    p.doc.trim().replace('\n', " ")
                );
            }
            let _ = writeln!(out, "    pub {}: {},", field, rust_ty);
        }
        let _ = writeln!(out, "}}");

        let _ = writeln!(out, "#[allow(dead_code)]");
        let _ = writeln!(out, "impl {} {{", struct_name);
        let _ = writeln!(out, "    /// Contract function name.");
        let _ = writeln!(out, "    pub const NAME: &'static str = \"{}\";", f.name);
        let _ = writeln!(
            out,
            "    /// Encode this call as `TYPE:VALUE` argument strings"
        );
        let _ = writeln!(
            out,
            "    /// for `sdkt call <contract-id> {} [args...]`.",
            f.name
        );
        let _ = writeln!(out, "    pub fn args(&self) -> Vec<String> {{");
        let _ = writeln!(out, "        vec![");
        for p in &f.parameters {
            let tag = scalar_type_tag(&p.type_).expect("validated above");
            let field = rust_ident(&p.name);
            let expr = if p.type_.name == "bool" || p.type_.name.starts_with(['u', 'i']) {
                format!("self.{}.to_string()", field)
            } else {
                format!("self.{}.clone()", field)
            };
            let _ = writeln!(out, "            format!(\"{}:{{}}\", {}),", tag, expr);
        }
        let _ = writeln!(out, "        ]");
        let _ = writeln!(out, "    }}");
        let _ = writeln!(out, "}}");
        // Return type alias at module level (stable Rust — an associated type
        // inside an inherent impl would require nightly).
        let _ = writeln!(
            out,
            "/// Expected return type of `{}`.\npub type {} = {};\n",
            f.name, alias_name, out_ty
        );
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ContractParameter;

    fn param(name: &str, type_name: &str, kind: &str) -> ContractParameter {
        ContractParameter {
            name: name.into(),
            doc: String::new(),
            type_: ContractType {
                name: type_name.into(),
                kind: kind.into(),
                doc: String::new(),
                members: vec![],
            },
        }
    }

    fn ty(name: &str, kind: &str) -> ContractType {
        ContractType {
            name: name.into(),
            kind: kind.into(),
            doc: String::new(),
            members: vec![],
        }
    }

    fn func(
        name: &str,
        params: Vec<ContractParameter>,
        outputs: Vec<ContractType>,
    ) -> ContractFunction {
        ContractFunction {
            name: name.into(),
            doc: String::new(),
            parameters: params,
            outputs,
        }
    }

    fn spec_of(functions: Vec<ContractFunction>) -> ContractSpec {
        ContractSpec {
            env_meta: None,
            functions,
            custom_types: vec![],
            events: vec![],
        }
    }

    #[test]
    fn generates_empty_contract() {
        let code = generate_client(&spec_of(vec![])).unwrap();
        assert!(code.contains("no callable functions"));
        assert!(code.contains("pub fn contract_functions() -> &'static [&'static str] { &[] }"));
    }

    #[test]
    fn generates_supported_scalars() {
        let spec = spec_of(vec![func(
            "transfer",
            vec![
                param("to", "address", "primitive"),
                param("amount", "u64", "primitive"),
                param("flag", "bool", "primitive"),
            ],
            vec![ty("void", "primitive")],
        )]);
        let code = generate_client(&spec).unwrap();
        assert!(code.contains("pub struct TransferCall {"));
        assert!(code.contains("pub to: String,"));
        assert!(code.contains("pub amount: u64,"));
        assert!(code.contains("pub flag: bool,"));
        assert!(code.contains("pub const NAME: &'static str = \"transfer\";"));
        assert!(code.contains("format!(\"address:{}\", self.to.clone())"));
        assert!(code.contains("format!(\"u64:{}\", self.amount.to_string())"));
        assert!(code.contains("format!(\"bool:{}\", self.flag.to_string())"));
        assert!(code.contains("pub type TransferOutput = ();"));
    }

    #[test]
    fn output_is_deterministic() {
        let spec = spec_of(vec![
            func("mint", vec![param("amt", "u32", "primitive")], vec![]),
            func("burn", vec![param("amt", "u32", "primitive")], vec![]),
        ]);
        let a = generate_client(&spec).unwrap();
        let b = generate_client(&spec).unwrap();
        assert_eq!(a, b, "generation must be byte-identical across runs");
    }

    #[test]
    fn rejects_udt_parameter() {
        let spec = spec_of(vec![func(
            "store",
            vec![param("p", "Point", "udt")],
            vec![],
        )]);
        let err = generate_client(&spec).unwrap_err();
        match err {
            ClientGenError::UnsupportedType {
                function,
                type_name,
                where_,
            } => {
                assert_eq!(function, "store");
                assert!(type_name.contains("Point"));
                assert!(where_.contains("parameter"));
            }
        }
    }

    #[test]
    fn rejects_compound_return() {
        let spec = spec_of(vec![func("get", vec![], vec![ty("option", "compound")])]);
        let err = generate_client(&spec).unwrap_err();
        assert!(matches!(err, ClientGenError::UnsupportedType { .. }));
        assert!(err.to_string().contains("return value"));
    }

    #[test]
    fn rejects_multiple_returns() {
        let spec = spec_of(vec![func(
            "multi",
            vec![],
            vec![ty("u32", "primitive"), ty("u32", "primitive")],
        )]);
        let err = generate_client(&spec).unwrap_err();
        assert!(err.to_string().contains("multiple returns"));
    }

    #[test]
    fn no_partial_output_on_failure() {
        let spec = spec_of(vec![
            func("ok", vec![param("x", "u32", "primitive")], vec![]),
            func("bad", vec![param("p", "Point", "udt")], vec![]),
        ]);
        let res = generate_client(&spec);
        assert!(res.is_err());
    }

    #[test]
    fn identifiers_are_sanitized_and_valid() {
        let spec = spec_of(vec![func(
            "increment",
            vec![param("New", "u64", "primitive")],
            vec![ty("u64", "primitive")],
        )]);
        let code = generate_client(&spec).unwrap();
        assert!(code.contains("pub struct IncrementCall"));
        assert!(code.contains("pub type IncrementOutput = u64;"));
        // "New" -> "new" (lowercase, valid identifier)
        assert!(code.contains("pub new: u64,"));
    }

    #[test]
    fn string_and_symbol_map_to_string() {
        let spec = spec_of(vec![func(
            "greet",
            vec![
                param("name", "string", "primitive"),
                param("tag", "symbol", "primitive"),
            ],
            vec![],
        )]);
        let code = generate_client(&spec).unwrap();
        assert!(code.contains("pub name: String,"));
        assert!(code.contains("pub tag: String,"));
        assert!(code.contains("format!(\"string:{}\", self.name.clone())"));
        assert!(code.contains("format!(\"symbol:{}\", self.tag.clone())"));
    }

    #[test]
    fn i32_i64_supported() {
        let spec = spec_of(vec![func(
            "set",
            vec![
                param("a", "i32", "primitive"),
                param("b", "i64", "primitive"),
            ],
            vec![ty("bool", "primitive")],
        )]);
        let code = generate_client(&spec).unwrap();
        assert!(code.contains("pub a: i32,"));
        assert!(code.contains("pub b: i64,"));
        assert!(code.contains("format!(\"i64:{}\", self.b.to_string())"));
        assert!(code.contains("pub type SetOutput = bool;"));
    }

    #[test]
    fn skip_unsupported_emits_supported_functions_and_header() {
        let spec = spec_of(vec![
            func(
                "hello",
                vec![param("x", "u32", "primitive")],
                vec![ty("u32", "primitive")],
            ),
            func(
                "batch",
                vec![param("transfers", "Map", "compound")],
                vec![ty("u32", "primitive")],
            ),
            func(
                "increment",
                vec![param("by", "u32", "primitive")],
                vec![ty("u32", "primitive")],
            ),
        ]);

        let opts = GenerateOptions {
            skip_unsupported: true,
        };
        let code = generate_client_with_options(&spec, &opts).unwrap();

        // Supported builders present
        assert!(code.contains("pub struct HelloCall"));
        assert!(code.contains("pub struct IncrementCall"));
        // Unsupported function skipped
        assert!(!code.contains("pub struct BatchCall"));

        // Header documents the skipped function and its detail
        assert!(code.contains("// Skipped unsupported functions:"));
        assert!(code.contains("// - batch: unsupported type 'compound:Map' (parameter `transfers`) in function `batch`"));

        // contract_functions lists only supported functions in spec order
        assert!(code.contains("pub fn contract_functions() -> &'static [&'static str] {\n    &[\"hello\", \"increment\"]\n}"));
    }

    #[test]
    fn skip_unsupported_without_flag_still_aborts() {
        let spec = spec_of(vec![
            func(
                "hello",
                vec![param("x", "u32", "primitive")],
                vec![ty("u32", "primitive")],
            ),
            func(
                "batch",
                vec![param("transfers", "Map", "compound")],
                vec![ty("u32", "primitive")],
            ),
        ]);
        let err = generate_client(&spec).unwrap_err();
        assert!(matches!(err, ClientGenError::UnsupportedType { .. }));
    }

    #[test]
    fn skip_unsupported_all_functions_unsupported() {
        let spec = spec_of(vec![
            func("bad1", vec![param("p", "Point", "udt")], vec![]),
            func("bad2", vec![], vec![ty("vec", "compound")]),
        ]);

        let opts = GenerateOptions {
            skip_unsupported: true,
        };
        let code = generate_client_with_options(&spec, &opts).unwrap();

        assert!(code.contains("// Skipped unsupported functions:"));
        assert!(code.contains("// - bad1:"));
        assert!(code.contains("// - bad2:"));
        assert!(code.contains("No callable functions were generated"));
        assert!(code.contains("pub fn contract_functions() -> &'static [&'static str] { &[] }"));
    }

    #[test]
    fn skip_unsupported_with_no_unsupported_is_byte_identical_to_default() {
        let spec = spec_of(vec![
            func(
                "foo",
                vec![param("a", "u32", "primitive")],
                vec![ty("bool", "primitive")],
            ),
            func("bar", vec![param("b", "string", "primitive")], vec![]),
        ]);

        let default_code = generate_client(&spec).unwrap();
        let opts = GenerateOptions {
            skip_unsupported: true,
        };
        let skip_code = generate_client_with_options(&spec, &opts).unwrap();

        assert_eq!(
            default_code, skip_code,
            "output must be identical when nothing is skipped"
        );
        assert!(!skip_code.contains("Skipped"));
    }

    #[test]
    fn skip_unsupported_preserves_spec_order() {
        let spec = spec_of(vec![
            func("first", vec![param("a", "u32", "primitive")], vec![]),
            func("skip1", vec![param("b", "Vec", "compound")], vec![]),
            func("second", vec![param("c", "u64", "primitive")], vec![]),
            func("skip2", vec![param("d", "Map", "compound")], vec![]),
            func("third", vec![param("e", "bool", "primitive")], vec![]),
        ]);

        let opts = GenerateOptions {
            skip_unsupported: true,
        };
        let code = generate_client_with_options(&spec, &opts).unwrap();

        assert!(code.contains("&[\"first\", \"second\", \"third\"]"));
    }
}
