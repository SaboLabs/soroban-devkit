//! Network fee information retrieval from Soroban RPC.
//!
//! Provides `get_fee_stats` to fetch current network fee metrics, mapping them
//! to the `sdkt-core` fee estimators.

use crate::{RpcError, SorobanRpcClient};
use sdkt_core::{FeeConfig, FeeEstimator, LedgerFeeSample};
use serde::Deserialize;

/// Query the current network fee stats.
pub async fn get_fee_stats(client: &SorobanRpcClient) -> Result<FeeStats, RpcError> {
    client.request("getFeeStats", ()).await
}

/// Extract valid fee samples from a percentile distribution.
/// Returns up to 7 valid numeric samples (min, p10..p50, mode).
fn parse_distr_to_samples(d: &FeeDistribution) -> Vec<LedgerFeeSample> {
    let keys = [&d.min, &d.p10, &d.p20, &d.p30, &d.p40, &d.p50, &d.mode];
    let mut samples = Vec::with_capacity(keys.len());
    for s in keys {
        if let Ok(val) = s.parse::<u32>() {
            samples.push(LedgerFeeSample { base_fee: val });
        }
    }
    samples
}

/// Parse FeeStats into estimation samples.
/// Priority 1: soroban_inclusion_fee percentiles.
/// Priority 2: inclusion_fee percentiles (fallback).
/// Returns None if no valid data.
fn parse_fee_stats(stats: &FeeStats) -> Option<Vec<LedgerFeeSample>> {
    let samples = parse_distr_to_samples(&stats.soroban_inclusion_fee);
    if !samples.is_empty() && samples.iter().any(|s| s.base_fee > 0) {
        return Some(samples);
    }

    let fallback = parse_distr_to_samples(&stats.inclusion_fee);
    if !fallback.is_empty() {
        return Some(fallback);
    }

    None
}

/// Estimate fees dynamically based on the current network state via RPC.
///
/// This uses `sdkt-core`'s `FeeEstimator` internally, preventing duplication of
/// fee calculation logic. It translates RPC network fees into `LedgerFeeSample`s.
pub async fn estimate_dynamic_fee(
    client: &SorobanRpcClient,
    config: FeeConfig,
) -> Result<(u64, String), RpcError> {
    let stats = get_fee_stats(client).await?;

    let samples = match parse_fee_stats(&stats) {
        Some(s) => s,
        None => {
            return Err(RpcError::Rpc(
                "No valid fee data available from network stats".to_string(),
            ))
        }
    };

    let estimator = FeeEstimator::new(config);
    let (stroops, xlm) = estimator
        .estimate(&samples)
        .map_err(|e| RpcError::Config(format!("Fee estimation failed: {e}")))?;

    Ok((stroops, xlm))
}

/// Soroban fee statistics response.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeeStats {
    pub soroban_inclusion_fee: FeeDistribution,
    pub inclusion_fee: FeeDistribution,
    pub latest_ledger: u32,
}

/// Distribution of fees across the network.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FeeDistribution {
    pub max: String,
    pub min: String,
    pub mode: String,
    pub p10: String,
    pub p20: String,
    pub p30: String,
    pub p40: String,
    pub p50: String,
    pub p60: String,
    pub p70: String,
    pub p80: String,
    pub p90: String,
    pub p95: String,
    pub p99: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_valid_distribution() -> FeeDistribution {
        FeeDistribution {
            max: "1000".to_string(),
            min: "100".to_string(),
            mode: "150".to_string(),
            p10: "110".to_string(),
            p20: "120".to_string(),
            p30: "130".to_string(),
            p40: "140".to_string(),
            p50: "150".to_string(),
            p60: "160".to_string(),
            p70: "170".to_string(),
            p80: "180".to_string(),
            p90: "190".to_string(),
            p95: "195".to_string(),
            p99: "199".to_string(),
        }
    }

    #[test]
    fn test_valid_parse() {
        let stats = FeeStats {
            soroban_inclusion_fee: mock_valid_distribution(),
            inclusion_fee: mock_valid_distribution(),
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert_eq!(samples.len(), 7);
        assert_eq!(samples[0].base_fee, 100); // min
        assert_eq!(samples[5].base_fee, 150); // p50
    }

    #[test]
    fn test_malformed_values_are_skipped() {
        let mut valid = mock_valid_distribution();
        valid.p50 = "bad_num".to_string();
        let stats = FeeStats {
            soroban_inclusion_fee: valid,
            inclusion_fee: mock_valid_distribution(),
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert_eq!(samples.len(), 6);
    }

    #[test]
    fn test_all_zero_primary_uses_fallback() {
        let zero_distr = FeeDistribution {
            max: "0".to_string(),
            min: "0".to_string(),
            mode: "0".to_string(),
            p10: "0".to_string(),
            p20: "0".to_string(),
            p30: "0".to_string(),
            p40: "0".to_string(),
            p50: "0".to_string(),
            p60: "0".to_string(),
            p70: "0".to_string(),
            p80: "0".to_string(),
            p90: "0".to_string(),
            p95: "0".to_string(),
            p99: "0".to_string(),
        };
        let stats = FeeStats {
            soroban_inclusion_fee: zero_distr,
            inclusion_fee: mock_valid_distribution(),
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert_eq!(samples.len(), 7);
        // fallback fee distribution got parsed, meaning some are > 0
        assert!(samples.iter().any(|s| s.base_fee > 0));
    }

    #[test]
    fn test_both_all_zero_gives_some_zero_samples() {
        let zero_distr = FeeDistribution {
            max: "0".to_string(),
            min: "0".to_string(),
            mode: "0".to_string(),
            p10: "0".to_string(),
            p20: "0".to_string(),
            p30: "0".to_string(),
            p40: "0".to_string(),
            p50: "0".to_string(),
            p60: "0".to_string(),
            p70: "0".to_string(),
            p80: "0".to_string(),
            p90: "0".to_string(),
            p95: "0".to_string(),
            p99: "0".to_string(),
        };
        let stats = FeeStats {
            soroban_inclusion_fee: zero_distr.clone(),
            inclusion_fee: zero_distr,
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert!(samples.iter().all(|s| s.base_fee == 0));
    }

    #[test]
    fn test_all_malformed_gives_none() {
        let bad_distr = FeeDistribution {
            max: "aaa".to_string(),
            min: "bbb".to_string(),
            mode: "ccc".to_string(),
            p10: "".to_string(),
            p20: "".to_string(),
            p30: "".to_string(),
            p40: "".to_string(),
            p50: "".to_string(),
            p60: "".to_string(),
            p70: "".to_string(),
            p80: "".to_string(),
            p90: "".to_string(),
            p95: "".to_string(),
            p99: "zzz".to_string(),
        };
        let stats = FeeStats {
            soroban_inclusion_fee: bad_distr.clone(),
            inclusion_fee: bad_distr,
            latest_ledger: 100,
        };
        assert!(parse_fee_stats(&stats).is_none());
    }
}
