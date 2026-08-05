use thiserror::Error;

/// Errors produced while auditing a source file.
#[derive(Debug, Error)]
pub enum AuditError {
    /// The input was not valid Rust source (e.g. a WASM binary passed by mistake).
    #[error("source parse error: {0}")]
    Parse(#[from] syn::Error),
}
