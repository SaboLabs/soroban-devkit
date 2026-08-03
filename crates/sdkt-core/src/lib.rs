//! sdkt-core entry point.
//!
//! Provides the primary configuration structures and parsing logic
//! for workspace-wide tools.

pub mod config;
pub use config::{DecodeConfig, DevKitConfig, NetworkConfig, StorageConfig};
