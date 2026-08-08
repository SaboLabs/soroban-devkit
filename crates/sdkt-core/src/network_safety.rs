//! Mainnet-safety guards for mutating RPC commands (M39).
//!
//! These helpers are deliberately small and pure: they consume the already
//! resolved [`NetworkConfig`] (whose precedence — explicit flags, then a saved
//! profile, then `.sdkt.toml`, then built-in defaults — is computed entirely by
//! the CLI's existing M29 resolution path) and enforce a single conservative
//! rule on mutating operations.
//!
//! A mutating command may only touch mainnet when the operator has explicitly
//! named the target network. Silently combining a mainnet RPC endpoint with the
//! default testnet passphrase (the classic foot-gun that signs an envelope for
//! the wrong network) is rejected with a clear, actionable error.
//!
//! No networking and no precedence logic lives here — this is a guard layered
//! on top of the existing resolution, not a replacement for it.

use crate::config::NetworkConfig;
use std::fmt;

/// Network passphrase of the public Stellar mainnet.
pub const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";

/// Network passphrase of the SDF testnet (also the [`NetworkConfig`] default).
pub const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";

/// Error returned when a mutating command is refused by the mainnet-safety guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainnetSafetyError(pub String);

impl fmt::Display for MainnetSafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MainnetSafetyError {}

/// Heuristic: does the RPC URL point at the public Stellar mainnet?
///
/// SDF mainnet endpoints live under `stellar.org` without the `testnet` /
/// `futurenet` markers used by the other public networks. This is intentionally
/// conservative — an unknown host is *not* treated as mainnet, so the guard only
/// fires on the well-known public mainnet RPC.
fn rpc_host_is_mainnet(rpc_url: &str) -> bool {
    let url = rpc_url.to_ascii_lowercase();
    if !url.contains("stellar.org") {
        return false;
    }
    if url.contains("testnet") || url.contains("futurenet") {
        return false;
    }
    true
}

/// Guard a *mutating* RPC operation against an unsafe mainnet configuration.
///
/// `network_explicit` is true when the operator named the target network through
/// an explicit flag (`--rpc-url`, `--network-passphrase`) or a saved
/// `--network-profile`. It is false when the resolved [`NetworkConfig`] came
/// entirely from built-in defaults (testnet).
///
/// The guard refuses in two clearly-wrong situations:
///
/// 1. The effective passphrase is the mainnet passphrase but the network was
///    *not* explicitly selected (e.g. a stray default that happens to match
///    mainnet). Operators must opt in deliberately.
/// 2. The RPC URL points at mainnet while the passphrase is *not* the mainnet
///    passphrase — i.e. someone aimed the tool at mainnet but forgot to set the
///    matching passphrase, which would sign an envelope for the wrong network.
///
/// Everything else (testnet by default, or mainnet with both an explicit,
/// matching passphrase) is allowed through.
pub fn guard_mutating_network(
    config: &NetworkConfig,
    network_explicit: bool,
) -> Result<(), MainnetSafetyError> {
    let passphrase_is_mainnet = config.passphrase == MAINNET_PASSPHRASE;
    let rpc_is_mainnet = rpc_host_is_mainnet(&config.rpc_url);

    if passphrase_is_mainnet && !network_explicit {
        return Err(MainnetSafetyError(
            "Refusing mutating operation on mainnet: the network was not explicitly selected. \
             Pass --network-passphrase 'Public Global Stellar Network ; September 2015' \
             (or use --network-profile) to target mainnet deliberately."
                .to_string(),
        ));
    }

    if rpc_is_mainnet && !passphrase_is_mainnet {
        return Err(MainnetSafetyError(
            "Refusing mutating operation: the RPC URL targets mainnet but the network passphrase \
             is not the mainnet passphrase (likely the testnet default). Set --network-passphrase \
             'Public Global Stellar Network ; September 2015' to match the endpoint."
                .to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkConfig;

    fn testnet_default() -> NetworkConfig {
        NetworkConfig::default()
    }

    fn mainnet_cfg() -> NetworkConfig {
        NetworkConfig {
            rpc_url: "https://soroban-rpc.stellar.org".to_string(),
            passphrase: MAINNET_PASSPHRASE.to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        }
    }

    #[test]
    fn testnet_default_is_allowed() {
        let cfg = testnet_default();
        assert!(guard_mutating_network(&cfg, false).is_ok());
        assert!(guard_mutating_network(&cfg, true).is_ok());
    }

    #[test]
    fn mainnet_explicit_passphrase_is_allowed() {
        let cfg = mainnet_cfg();
        // Operator explicitly selected the network (flag/profile).
        assert!(guard_mutating_network(&cfg, true).is_ok());
    }

    #[test]
    fn mainnet_passphrase_without_explicit_opt_in_is_refused() {
        let cfg = mainnet_cfg();
        let err = guard_mutating_network(&cfg, false).unwrap_err();
        assert!(err.to_string().contains("not explicitly selected"));
    }

    #[test]
    fn mainnet_rpc_with_testnet_passphrase_is_refused() {
        let cfg = NetworkConfig {
            rpc_url: "https://soroban-rpc.stellar.org".to_string(),
            passphrase: TESTNET_PASSPHRASE.to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        };
        // No explicit flag needed to trigger: the mismatch itself is the foot-gun.
        let err = guard_mutating_network(&cfg, true).unwrap_err();
        assert!(err.to_string().contains("not the mainnet passphrase"));
    }

    #[test]
    fn mainnet_rpc_with_testnet_default_is_refused() {
        // User passed --rpc-url mainnet but forgot --network-passphrase.
        let cfg = NetworkConfig {
            rpc_url: "https://soroban-rpc.stellar.org".to_string(),
            passphrase: TESTNET_PASSPHRASE.to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        };
        let err = guard_mutating_network(&cfg, false).unwrap_err();
        assert!(err.to_string().contains("not the mainnet passphrase"));
    }

    #[test]
    fn unknown_rpc_host_is_not_treated_as_mainnet() {
        let cfg = NetworkConfig {
            rpc_url: "https://my-custom-rpc.example.com".to_string(),
            passphrase: TESTNET_PASSPHRASE.to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        };
        assert!(guard_mutating_network(&cfg, true).is_ok());
    }

    #[test]
    fn testnet_rpc_url_is_not_mainnet() {
        let cfg = NetworkConfig {
            rpc_url: "https://soroban-testnet.stellar.org".to_string(),
            passphrase: TESTNET_PASSPHRASE.to_string(),
            timeout_secs: Some(15),
            pool_max_idle_per_host: Some(100),
        };
        assert!(guard_mutating_network(&cfg, true).is_ok());
    }
}
