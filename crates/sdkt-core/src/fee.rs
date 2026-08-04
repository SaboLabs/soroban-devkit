//! Fee estimation engine for sdkt-core.
//!
//! Produces base-fee estimates from recent ledger data and applies
//! network-specific multipliers so CLI and wallet users can preview the
//! cost of a transaction before broadcasting.
//!
//! Fee values are expressed in **stroops** (1 XLM = 10_000_000 stroops).
//! Conversions to XLM strings use integer math to avoid float drift.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// One whole XLM expressed in stroops.
pub const STROOPS_PER_XLM: u64 = 10_000_000;

/// Soroban networks the DevKit can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NetworkKind {
    #[default]
    Testnet,
    Mainnet,
    Standalone,
}

impl fmt::Display for NetworkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            NetworkKind::Testnet => "testnet",
            NetworkKind::Mainnet => "mainnet",
            NetworkKind::Standalone => "standalone",
        };
        write!(f, "{s}")
    }
}

impl FromStr for NetworkKind {
    type Err = FeeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "testnet" => Ok(NetworkKind::Testnet),
            "mainnet" => Ok(NetworkKind::Mainnet),
            "standalone" => Ok(NetworkKind::Standalone),
            other => Err(FeeError::UnknownNetwork(other.to_string())),
        }
    }
}

impl NetworkKind {
    /// Platform-default fee multiplier applied on top of the ledger base fee.
    pub fn fee_multiplier(self) -> f64 {
        match self {
            NetworkKind::Testnet => 1.0,
            NetworkKind::Mainnet => 1.25,
            NetworkKind::Standalone => 0.10,
        }
    }
}

/// Error type returned by the fee engine.
#[derive(Debug, PartialEq)]
pub enum FeeError {
    EmptyLedger,
    UnknownNetwork(String),
    Conversion,
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeeError::EmptyLedger => {
                write!(f, "no ledger data supplied for fee estimation")
            }
            FeeError::UnknownNetwork(n) => {
                write!(f, "unknown Soroban network: {n}")
            }
            FeeError::Conversion => {
                write!(f, "integer conversion failure")
            }
        }
    }
}

impl std::error::Error for FeeError {}

/// Fee engine configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FeeConfig {
    pub network: NetworkKind,
    pub multiplier_override: Option<f64>,
}

impl FeeConfig {
    pub fn testnet() -> Self {
        Self {
            network: NetworkKind::Testnet,
            multiplier_override: None,
        }
    }
    pub fn mainnet() -> Self {
        Self {
            network: NetworkKind::Mainnet,
            multiplier_override: None,
        }
    }
    pub fn standalone() -> Self {
        Self {
            network: NetworkKind::Standalone,
            multiplier_override: None,
        }
    }

    /// Effective multiplier (override or network default).
    pub fn multiplier(&self) -> f64 {
        self.multiplier_override
            .unwrap_or_else(|| self.network.fee_multiplier())
    }
}

/// Raw fee reading pulled from a recent ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerFeeSample {
    pub base_fee: u32, // stroops, as reported by the ledger
}

/// Fee estimator — stateless, fed samples from recent ledgers.
pub struct FeeEstimator {
    config: FeeConfig,
}

impl FeeEstimator {
    pub fn new(config: FeeConfig) -> Self {
        Self { config }
    }

    /// Median base fee from a slice of ledger samples.
    pub fn estimate_base_fee(samples: &[LedgerFeeSample]) -> Result<u32, FeeError> {
        if samples.is_empty() {
            return Err(FeeError::EmptyLedger);
        }
        let mut sorted: Vec<u32> = samples.iter().map(|s| s.base_fee).collect();
        sorted.sort_unstable();
        // lower median for even-length samples
        let mid = (sorted.len() - 1) / 2;
        Ok(sorted[mid])
    }

    /// Final estimated fee in stroops = base_fee * multiplier.
    pub fn estimate_stroops(&self, samples: &[LedgerFeeSample]) -> Result<u64, FeeError> {
        let base = Self::estimate_base_fee(samples)? as u64;
        let mult = self.config.multiplier();
        let fee = base as f64 * mult;
        if fee > u64::MAX as f64 {
            return Err(FeeError::Conversion);
        }
        Ok(fee.round() as u64)
    }

    /// Final estimated fee in XLM string (exact integer fraction).
    pub fn estimate_xlm(&self, samples: &[LedgerFeeSample]) -> Result<String, FeeError> {
        let stroops = self.estimate_stroops(samples)?;
        let whole = stroops / STROOPS_PER_XLM;
        let frac = stroops % STROOPS_PER_XLM;
        if frac == 0 {
            Ok(format!("{whole}"))
        } else {
            let s = format!("{whole}.{frac:07}");
            Ok(s.trim_end_matches('0').trim_end_matches('.').to_string())
        }
    }

    /// Single-call convenience returning both units.
    pub fn estimate(&self, samples: &[LedgerFeeSample]) -> Result<(u64, String), FeeError> {
        let stroops = self.estimate_stroops(samples)?;
        let xlm = self.estimate_xlm(samples)?;
        Ok((stroops, xlm))
    }
}

impl Default for FeeEstimator {
    fn default() -> Self {
        Self::new(FeeConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(s: u32) -> LedgerFeeSample {
        LedgerFeeSample { base_fee: s }
    }

    #[test]
    fn test_stroops_per_xlm() {
        assert_eq!(STROOPS_PER_XLM, 10_000_000);
    }

    #[test]
    fn test_network_display() {
        assert_eq!(NetworkKind::Testnet.to_string(), "testnet");
        assert_eq!(NetworkKind::Mainnet.to_string(), "mainnet");
        assert_eq!(NetworkKind::Standalone.to_string(), "standalone");
    }

    #[test]
    fn test_network_from_str() {
        assert_eq!(
            "testnet".parse::<NetworkKind>().unwrap(),
            NetworkKind::Testnet
        );
        assert_eq!(
            "MAINNET".parse::<NetworkKind>().unwrap(),
            NetworkKind::Mainnet
        );
        assert!("fakenet".parse::<NetworkKind>().is_err());
    }

    #[test]
    fn test_network_multipliers() {
        assert_eq!(NetworkKind::Testnet.fee_multiplier(), 1.0);
        assert_eq!(NetworkKind::Mainnet.fee_multiplier(), 1.25);
        assert_eq!(NetworkKind::Standalone.fee_multiplier(), 0.10);
    }

    #[test]
    fn test_estimate_base_fee_median() {
        let samples = vec![sample(100), sample(200), sample(300)];
        assert_eq!(FeeEstimator::estimate_base_fee(&samples).unwrap(), 200);
    }

    #[test]
    fn test_estimate_base_fee_even_count() {
        let samples = vec![sample(100), sample(200), sample(300), sample(400)];
        assert_eq!(FeeEstimator::estimate_base_fee(&samples).unwrap(), 200);
    }

    #[test]
    fn test_estimate_base_fee_empty() {
        let empty: [LedgerFeeSample; 0] = [];
        let err = FeeEstimator::estimate_base_fee(&empty).unwrap_err();
        assert_eq!(err, FeeError::EmptyLedger);
    }

    #[test]
    fn test_testnet_fee_stroops() {
        let est = FeeEstimator::new(FeeConfig::testnet());
        let samples = vec![sample(100)];
        assert_eq!(est.estimate_stroops(&samples).unwrap(), 100);
    }

    #[test]
    fn test_mainnet_fee_stroops() {
        let est = FeeEstimator::new(FeeConfig::mainnet());
        let samples = vec![sample(100)];
        assert_eq!(est.estimate_stroops(&samples).unwrap(), 125);
    }

    #[test]
    fn test_standalone_fee_stroops() {
        let est = FeeEstimator::new(FeeConfig::standalone());
        let samples = vec![sample(100)];
        assert_eq!(est.estimate_stroops(&samples).unwrap(), 10);
    }

    #[test]
    fn test_override_multiplier() {
        let cfg = FeeConfig {
            network: NetworkKind::Testnet,
            multiplier_override: Some(2.5),
        };
        let est = FeeEstimator::new(cfg);
        let samples = vec![sample(100)];
        assert_eq!(est.estimate_stroops(&samples).unwrap(), 250);
    }

    #[test]
    fn test_estimate_xlm_exact() {
        let est = FeeEstimator::new(FeeConfig::standalone());
        let samples = vec![sample(100)];
        assert_eq!(est.estimate_xlm(&samples).unwrap(), "0.000001");
    }

    #[test]
    fn test_estimate_xlm_whole() {
        let est = FeeEstimator::new(FeeConfig::mainnet());
        let samples = vec![sample(8_000_000)];
        assert_eq!(est.estimate_xlm(&samples).unwrap(), "1");
    }

    #[test]
    fn test_estimate_combined() {
        let est = FeeEstimator::new(FeeConfig::mainnet());
        let samples = vec![sample(100)];
        let (stroops, xlm) = est.estimate(&samples).unwrap();
        assert_eq!(stroops, 125);
        assert_eq!(xlm, "0.0000125");
    }

    #[test]
    fn test_real_testnet_sample() {
        let samples = vec![sample(100)];
        let est = FeeEstimator::new(FeeConfig::testnet());
        let (stroops, xlm) = est.estimate(&samples).unwrap();
        assert_eq!(stroops, 100);
        assert_eq!(xlm, "0.00001");
    }

    #[test]
    fn test_multiple_samples() {
        let samples = vec![sample(100), sample(120), sample(90), sample(110)];
        let est = FeeEstimator::new(FeeConfig::testnet());
        let (stroops, _xlm) = est.estimate(&samples).unwrap();
        assert_eq!(stroops, 100); // sorted [90,100,110,120] -> idx1=100
    }
}
