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
         the generator core supports \
         u32|i32|u64|i64|u128|i128|bool|address|string|symbol|bytes|void only"
    )]
    UnsupportedType {
        function: String,
        type_name: String,
        where_: String,
    },
}

/// Map a supported primitive `ContractType` to its Rust representation.
fn map_scalar(t: &ContractType) -> Option<&'static str> {
    if t.kind != "primitive" {
        return None;
    }

    match t.name.as_str() {
        "u32" => Some("u32"),
        "i32" => Some("i32"),
        "u64" => Some("u64"),
        "i64" => Some("i64"),
        "u128" => Some("u128"),
        "i128" => Some("i128"),
        "bool" => Some("bool"),
        "address" | "string" | "symbol" | "bytes" => Some("String"),
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
        "u128" => Some("u128"),
        "i128" => Some("i128"),
        "bool" => Some("bool"),
        "address" => Some("address"),
        "string" => Some("string"),
        "symbol" => Some("symbol"),
        "bytes" => Some("bytes"),
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

/// Convert a function name to the generated call struct name.
fn struct_ident(fn_name: &str) -> String {
    let mut out = String::new();
    let mut uppercase_next = true;

    for ch in fn_name.chars() {
        if ch == '_' || !ch.is_ascii_alphanumeric() {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }

        uppercase_next = false;
    }

    out.push_str("Call");
    out
}

/// Return a formatted type name for an unsupported contract type.
fn unsupported_type_name(t: &ContractType) -> String {
    format!("{}:{}", t.kind, t.name)
}

/// Create an unsupported-type error for a parameter.
fn unsupported_parameter_error(
    function: &ContractFunction,
    parameter_name: &str,
    parameter_type: &ContractType,
) -> ClientGenError {
    ClientGenError::UnsupportedType {
        function: function.name.clone(),
        type_name: unsupported_type_name(parameter_type),
        where_: format!("parameter `{}`", parameter_name),
    }
}

/// Create an unsupported-type error for a return value.
fn unsupported_return_error(
    function: &ContractFunction,
    return_type: &ContractType,
) -> ClientGenError {
    ClientGenError::UnsupportedType {
        function: function.name.clone(),
        type_name: unsupported_type_name(return_type),
        where_: "return value".to_string(),
    }
}

/// Validate that a function uses only supported types.
fn check_supported(function: &ContractFunction) -> Result<(), ClientGenError> {
    for parameter in &function.parameters {
        if map_scalar(&parameter.type_).is_none() {
            return Err(unsupported_parameter_error(
                function,
                &parameter.name,
                &parameter.type_,
            ));
        }
    }

    for output in &function.outputs {
        if map_scalar(output).is_none() {
            return Err(unsupported_return_error(function, output));
        }
    }

    if function.outputs.len() > 1 {
        return Err(ClientGenError::UnsupportedType {
            function: function.name.clone(),
            type_name: format!("multiple returns ({})", function.outputs.len()),
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

/// Generate a deterministic Rust client module from a parsed `ContractSpec`.
pub fn generate_client(spec: &ContractSpec) -> Result<String, ClientGenError> {
    generate_client_with_options(spec, &GenerateOptions::default())
}

/// Generate a deterministic Rust client module with custom options.
pub fn generate_client_with_options(
    spec: &ContractSpec,
    options: &GenerateOptions,
) -> Result<String, ClientGenError> {
    let (supported, skipped) = collect_supported_functions(spec, options)?;

    let mut output = String::new();

    write_header(&mut output, &skipped);

    if spec.functions.is_empty() {
        write_empty_contract(&mut output);
        return Ok(output);
    }

    if supported.is_empty() {
        write_no_supported_functions(&mut output);
        return Ok(output);
    }

    write_contract_functions(&mut output, &supported);

    for function in &supported {
        write_function(&mut output, function);
    }

    Ok(output)
}

/// Separate supported functions from skipped functions.
fn collect_supported_functions<'a>(
    spec: &'a ContractSpec,
    options: &GenerateOptions,
) -> Result<
    (
        Vec<&'a ContractFunction>,
        Vec<(&'a ContractFunction, ClientGenError)>,
    ),
    ClientGenError,
> {
    let mut supported = Vec::new();
    let mut skipped = Vec::new();

    for function in &spec.functions {
        match check_supported(function) {
            Ok(()) => supported.push(function),
            Err(error) if options.skip_unsupported => {
                skipped.push((function, error));
            }
            Err(error) => return Err(error),
        }
    }

    Ok((supported, skipped))
}

/// Write the generated source header.
fn write_header(output: &mut String, skipped: &[(&ContractFunction, ClientGenError)]) {
    output.push_str("// Generated by `sdkt generate client`. Do not edit by hand.\n");
    output.push_str("//\n");
    output.push_str("// Each `*Call` struct encodes one contract function as `TYPE:VALUE`\n");
    output.push_str("// argument strings accepted by `sdkt call` and `sdkt invoke`.\n");

    if !skipped.is_empty() {
        output.push_str("//\n");
        output.push_str("// Skipped unsupported functions:\n");

        for (function, error) in skipped {
            let _ = writeln!(output, "// - {}: {}", function.name, error);
        }
    }

    output.push('\n');
}

/// Write the empty-contract output.
fn write_empty_contract(output: &mut String) {
    output.push_str("/// This contract exposes no callable functions.\n");
    output.push_str("pub fn contract_functions() -> &'static [&'static str] { &[] }\n");
}

/// Write the output used when every function is unsupported.
fn write_no_supported_functions(output: &mut String) {
    output.push_str(
        "/// No callable functions were generated (all functions use unsupported types).\n",
    );
    output.push_str("pub fn contract_functions() -> &'static [&'static str] { &[] }\n");
}

/// Write the list of generated contract functions.
fn write_contract_functions(output: &mut String, functions: &[&ContractFunction]) {
    output.push_str("/// Names of all generated contract calls, in spec order.\n");
    output.push_str("pub fn contract_functions() -> &'static [&'static str] {\n    &[");

    for (index, function) in functions.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }

        let _ = write!(output, "\"{}\"", function.name);
    }

    output.push_str("]\n}\n\n");
}

/// Write a generated call builder.
fn write_function(output: &mut String, function: &ContractFunction) {
    let struct_name = struct_ident(&function.name);
    let output_type = function
        .outputs
        .first()
        .map(|output| map_scalar(output).expect("function was validated"))
        .unwrap_or("()");

    let alias_name = format!("{}Output", struct_name.trim_end_matches("Call"));

    write_function_doc(output, function);
    write_struct(output, function, &struct_name);
    write_impl(output, function, &struct_name);
    write_output_alias(output, function, &alias_name, output_type);
}

/// Write documentation for a generated function.
fn write_function_doc(output: &mut String, function: &ContractFunction) {
    if !function.doc.trim().is_empty() {
        let _ = writeln!(
            output,
            "/// {} — {}",
            function.name,
            function.doc.trim().replace('\n', " ")
        );
    } else {
        let _ = writeln!(output, "/// Typed call builder for `{}`.", function.name);
    }
}

/// Write the generated call struct.
fn write_struct(output: &mut String, function: &ContractFunction, struct_name: &str) {
    let _ = writeln!(output, "pub struct {} {{", struct_name);

    for parameter in &function.parameters {
        write_parameter(output, parameter);
    }

    let _ = writeln!(output, "}}");
}

/// Write a single generated parameter.
fn write_parameter(output: &mut String, parameter: &crate::spec::ContractParameter) {
    let rust_type = map_scalar(&parameter.type_).expect("parameter was validated");
    let field = rust_ident(&parameter.name);

    if parameter.type_.name == "bytes" {
        output.push_str("    /// Hex-encoded bytes string, e.g. `0a0bff`.\n");
    }

    if !parameter.doc.trim().is_empty() {
        let _ = writeln!(
            output,
            "    /// {} — {}",
            parameter.name,
            parameter.doc.trim().replace('\n', " ")
        );
    }

    let _ = writeln!(output, "    pub {}: {},", field, rust_type);
}

/// Write the implementation containing `NAME` and `args()`.
fn write_impl(output: &mut String, function: &ContractFunction, struct_name: &str) {
    let _ = writeln!(output, "#[allow(dead_code)]");
    let _ = writeln!(output, "impl {} {{", struct_name);

    output.push_str("    /// Contract function name.\n");
    let _ = writeln!(
        output,
        "    pub const NAME: &'static str = \"{}\";",
        function.name
    );

    output.push_str("    /// Encode this call as `TYPE:VALUE` argument strings\n");

    let _ = writeln!(
        output,
        "    /// for `sdkt call <contract-id> {} [args...]`.",
        function.name
    );

    output.push_str("    pub fn args(&self) -> Vec<String> {\n");
    output.push_str("        vec![\n");

    for parameter in &function.parameters {
        write_argument(output, parameter);
    }

    output.push_str("        ]\n");
    output.push_str("    }\n");
    output.push_str("}\n");
}

/// Write one encoded argument.
fn write_argument(output: &mut String, parameter: &crate::spec::ContractParameter) {
    let tag = scalar_type_tag(&parameter.type_).expect("parameter was validated");
    let field = rust_ident(&parameter.name);

    let value = if parameter.type_.name == "bool" || parameter.type_.name.starts_with(['u', 'i']) {
        format!("self.{}.to_string()", field)
    } else {
        format!("self.{}.clone()", field)
    };

    let _ = writeln!(output, "            format!(\"{}:{{}}\", {}),", tag, value);
}

/// Write the generated output type alias.
fn write_output_alias(
    output: &mut String,
    function: &ContractFunction,
    alias_name: &str,
    output_type: &str,
) {
    let _ = writeln!(
        output,
        "/// Expected return type of `{}`.\npub type {} = {};\n",
        function.name, alias_name, output_type
    );
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
    fn generates_u128_i128_and_bytes() {
        let spec = spec_of(vec![func(
            "transfer",
            vec![
                param("amount", "u128", "primitive"),
                param("delta", "i128", "primitive"),
                param("data", "bytes", "primitive"),
            ],
            vec![],
        )]);

        let code = generate_client(&spec).unwrap();

        assert!(code.contains("pub amount: u128,"));
        assert!(code.contains("pub delta: i128,"));
        assert!(code.contains("pub data: String,"));

        assert!(code.contains("format!(\"u128:{}\", self.amount.to_string())"));
        assert!(code.contains("format!(\"i128:{}\", self.delta.to_string())"));
        assert!(code.contains("format!(\"bytes:{}\", self.data.clone())"));

        assert!(code.contains("/// Hex-encoded bytes string, e.g. `0a0bff`."));
    }

    #[test]
    fn output_is_deterministic() {
        let spec = spec_of(vec![
            func("mint", vec![param("amt", "u32", "primitive")], vec![]),
            func("burn", vec![param("amt", "u32", "primitive")], vec![]),
        ]);

        let first = generate_client(&spec).unwrap();
        let second = generate_client(&spec).unwrap();

        assert_eq!(
            first, second,
            "generation must be byte-identical across runs"
        );
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

        let result = generate_client(&spec);

        assert!(result.is_err());
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

        let options = GenerateOptions {
            skip_unsupported: true,
        };

        let code = generate_client_with_options(&spec, &options).unwrap();

        assert!(code.contains("pub struct HelloCall"));
        assert!(code.contains("pub struct IncrementCall"));
        assert!(!code.contains("pub struct BatchCall"));

        assert!(code.contains("// Skipped unsupported functions:"));

        assert!(code.contains(
            "// - batch: unsupported type 'compound:Map' \
             (parameter `transfers`) in function `batch`"
        ));

        assert!(code.contains(
            "pub fn contract_functions() -> &'static [&'static str] \
             {\n    &[\"hello\", \"increment\"]\n}"
        ));
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

        let options = GenerateOptions {
            skip_unsupported: true,
        };

        let code = generate_client_with_options(&spec, &options).unwrap();

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

        let options = GenerateOptions {
            skip_unsupported: true,
        };

        let skip_code = generate_client_with_options(&spec, &options).unwrap();

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

        let options = GenerateOptions {
            skip_unsupported: true,
        };

        let code = generate_client_with_options(&spec, &options).unwrap();

        assert!(code.contains("&[\"first\", \"second\", \"third\"]"));
    }
}
