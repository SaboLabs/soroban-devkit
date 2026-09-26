use thiserror::Error;

/// Errors produced while auditing a source file.
#[derive(Debug, Error)]
pub enum AuditError {
    /// The input was not valid Rust source (e.g. a WASM binary passed by mistake).
    #[error("source parse error: {0}")]
    Parse(#[from] syn::Error),

    /// Reading or writing a baseline file failed.
    #[error("baseline i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A baseline file was not valid JSON (or not a baseline document).
    #[error("baseline serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// The baseline was written by an incompatible format version.
    #[error("unsupported baseline format version {found} (expected {expected})")]
    BaselineFormat { found: u32, expected: u32 },
}
