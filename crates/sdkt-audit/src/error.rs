use thiserror::Error;

/// Errors produced while auditing a source file.
#[derive(Debug, Error)]
pub enum AuditError {
    /// The input was not valid Rust source (e.g. a WASM binary passed by mistake).
    #[error("source parse error: {0}")]
    Parse(#[from] syn::Error),
}

/// Errors produced while interacting with the plugin store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A plugin with the same id is already installed.
    #[error("plugin '{0}' already installed (use --force to overwrite)")]
    AlreadyInstalled(String),
    /// Two different plugin ids sanitize to the same store directory.
    #[error(
        "plugin id collision: '{incoming}' and '{existing}' both map to the same store directory"
    )]
    IdCollision {
        /// The id of the plugin being installed.
        incoming: String,
        /// The id of the plugin already occupying the store directory.
        existing: String,
    },
}
