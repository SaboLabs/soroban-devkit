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

/// Estimate fees dynamically based on the current network state via RPC.
///
/// This uses `sdkt-core`'s `FeeEstimator` internally, preventing duplication of
/// fee calculation logic. It translates RPC network fees into `LedgerFeeSample`s.
pub async fn estimate_dynamic_fee(
    client: &SorobanRpcClient,
    config: FeeConfig,
) -> Result<(u64, String), RpcError> {
    let stats = get_fee_stats(client).await?;

    // We extract standard percentiles from soroban inclusion fees to act as our ledger samples
    let d = &stats.soroban_inclusion_fee;
    let str_samples = [
        &d.min, &d.p10, &d.p20, &d.p30, &d.p40, &d.p50, // median
        &d.mode,
    ];

    let mut samples = Vec::with_capacity(str_samples.len());
    for s in str_samples {
        if let Ok(val) = s.parse::<u32>() {
            samples.push(LedgerFeeSample { base_fee: val });
        }
    }

    if samples.is_empty() {
        return Err(RpcError::Rpc(
            "Failed to parse dynamic fee samples from RPC".to_string(),
        ));
    }

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
#[derive(Debug, Deserialize, PartialEq)]
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

    fn mock_fee_distribution() -> FeeDistribution {
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
    fn test_fee_distribution_deserialize() {
        let json = r#"{
            "max": "1000",
            "min": "100",
            "mode": "150",
            "p10": "110",
            "p20": "120",
            "p30": "130",
            "p40": "140",
            "p50": "150",
            "p60": "160",
            "p70": "170",
            "p80": "180",
            "p90": "190",
            "p95": "195",
            "p99": "199"
        }"#;
        let d: FeeDistribution = serde_json::from_str(json).unwrap();
        assert_eq!(d.p50, "150");
        assert_eq!(d.min, "100");
    }

    #[test]
    fn test_fee_samples_extraction() {
        let d = mock_fee_distribution();
        let str_samples = [&d.min, &d.p10, &d.p20, &d.p30, &d.p40, &d.p50, &d.mode];

        let mut samples = Vec::new();
        for s in str_samples {
            if let Ok(val) = s.parse::<u32>() {
                samples.push(LedgerFeeSample { base_fee: val });
            }
        }
        assert_eq!(samples.len(), 7);
        assert_eq!(samples[0].base_fee, 100);
        assert_eq!(samples[5].base_fee, 150);
    }

    #[test]
    fn test_fee_samples_with_invalid() {
        let mut d = mock_fee_distribution();
        d.p50 = "invalid".to_string();
        let str_samples = [&d.min, &d.p50];
        let mut samples = Vec::new();
        for s in str_samples {
            if let Ok(val) = s.parse::<u32>() {
                samples.push(LedgerFeeSample { base_fee: val });
            }
        }
        assert_eq!(samples.len(), 1); // Only the valid one
    }
}
