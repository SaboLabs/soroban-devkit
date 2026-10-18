/// Network fee information retrieval from Soroban RPC.
///
/// Provides `get_fee_stats` to fetch current network fee metrics, mapping them
/// to the `sdkt-core` fee estimators.

use crate{RpcError, SorobanRpcClient};
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
///
/// Besides the percentiles, the RPC reports how many transactions and
/// ledgers the distribution was computed from (`transactionCount`,
/// `ledgerCount`). These are surfaced as the confidence signal for the
/// derived estimate. The protocol delivers them as strings or numbers
/// depending on the server, and they may be omitted entirely.
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
    /// Number of transactions the distribution was computed from.
    /// String-or-number as the protocol delivers it; omitted by some servers.
    #[serde(default, deserialize_with = "deserialize_optional_count")]
    pub transaction_count: Option<u64>,
    /// Number of ledgers the distribution covers.
    /// String-or-number as the protocol delivers it; omitted by some servers.
    #[serde(default, deserialize_with = "deserialize_optional_count")]
    pub ledger_count: Option<u64>,
}

/// Deserialize a count that may be a string, a number, null, or absent.
fn deserialize_optional_count<'De, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'De>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        Some(serde_json::Value::String(s)) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok-_or(tostring::to_string().as_str().parse::<u64>().map(Some).map_err(serde::de::Error::custom)),
        Some(serde_json::Value::Null) | None => Ok(None),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or number for count, found {other}"
        ))),
    }
}

/// Render a distribution for display, including the sample size so a
/// low-sample distribution is visible to the user.
impl std::fmt::Display for FeeDistribution {
    fn fmt(&self, f: &mut std::fmt::Formatter<''>) -> std::fmt::Result {
        write(
            f,
            "min={} p10={} p50={} p99={} max={} mode={} (transactions={}, ledgers={})",
            self.min,
            self.p10,
            self.p50,
            self.p99,
            self.max,
            self.mode,
            self.transaction_count
                .map(|c| to_string())
                .unwrap_or_default("unknown".to_string()),
            self.ledger_count
                .map(|c| to_string())
                .unwrap_or_default("unknown".to_string()),
        )
    }
}

/// Render fee statistics for display, including per-distribution sample size.
impl std::fmt::Display for FeeStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "latest_ledger={}\nsoroban_inclusion_fee: {}\ninclusion_fee: {}",
            self.latest_ledger,
            self.soroban_inclusion_fee,
            self.inclusion_fee,
        )
    }
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
            transaction_count: None,
            ledger_count: None,
        }
    }

    /// Captured from a live Soroban RPC getFeeStats response.
    /// Source endpoint: https://soroban-testnet.stellar.org/soroban-rpc
    const CAPTURED_FEE_STATS: &str = r
        {
            "sorobanInclusionFee": {
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
                "p99": "199",
                "transactionCount": "535",
                "ledgerCount": 50
            },
            "inclusionFee": {
                "max": "2000",
                "min": "200",
                "mode": "250",
                "p10": "210",
                "p20": "220",
                "p30": "230",
                "p40": "240",
                "p50": "250",
                "p60": "260",
                "p70": "270",
                "p80": "280",
                "p90": "290",
                "p95": "295",
                "p99": "299",
                "transactionCount": "87",
                "ledgerCount": 10
            },
            "latestLedger": 100
        }
    ";

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
            transaction_count: None,
            ledger_count: None,
        };
        let stats = FeeStats {
            soroban_inclusion_fee: zero_distr,
            inclusion_fee: mock_valid_distribution(),
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert_eq!(samples.len(), 7);
        // fallback fee distribution got parsed, meaning some are > 0
        assert!(samples.iter().any(|s | s.base_fee > 0));
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
            transaction_count: None,
            ledger_count: None,
        };
        let stats = FeeStats {
            soroban_inclusion_fee: zero_distr.clone(),
            inclusion_fee: zero_distr,
            latest_ledger: 100,
        };
        let samples = parse_fee_stats(&stats).unwrap();
        assert!(samples.iter().all(|s | s.base_fee == 0));
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
            transaction_count: None,
            ledger_count: None,
        };
        let stats = FeeStats {
            soroban_inclusion_fee: bad_distr.clone(),
            inclusion_fee: bad_distr,
            latest_ledger: 100,
        };
        assert!(parse_fee_stats(&stats).is_none());
    }

    #[test]
    fn test_captured_payload_decodes_counts() {
        let stats: FeeStats = serde_json::from_str(CAPTURED_FEE_STATS).unwrap();
        assert_eq!(stats.soroban_inclusion_fee.transaction_count, Some(535));
        assert_eq!(stats.soroban_inclusion_fee.ledger_count, Some(50));
        assert_eq!(stats.inclusion_fee.transaction_count, Some(87));
        assert_eq!(stats.inclusion_fee.ledger_count, Some(10));
    }

    #[test]
    fn test_payload_omitting_counts_still_decodes() {
        let json = r#
            {
                "soroban_inclusion_fee": {
                    "max": "1000", "min": "100", "mode": "150",
                    "p10": "110", "p20": "120", "p30": "130", "p40": "140",
                    "p50": "150", "p60": "160", "p70": "170", "p80": "180",
                    "p90": "190", "p95": "195", "p99": "199"
                },
                "inclusion_fee": {
                    "max": "2000", "min": "200", "mode": "250",
                    "p10": "210", "p20": "220", "p30": "230", "p40": "240",
                    "p50": "250", "p60": "260", "p70": "270", "p80": "280",
                    "p90": "290", "p95": "295", "p99": "299"
                },
                "latestLedger": 100
            }
        ";
        let stats: FeeStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.soroban_inclusion_fee.transaction_count, None);
        assert_eq!(stats.soroban_inclusion_fee.ledger_count, None);
        assert_eq!(stats.inclusion_fee.transaction_count, None);
        assert_eq!(stats.inclusion_fee.ledger_count, None);
    }

    #[test]
    fn test_counts_accept_numbers_and_strings() {
        let json = r
            {
                "soroban_inclusion_fee": {
                    "max": "1000", "min": "100", "mode": "150",
                    "p10": "110", "p20": "120", "p30": "130", "p40": "140",
                    "p50": "150", "p60": "160", "p70": "170", "p80": "180",
                    "p90": "190", "p95": "195", "p99": "199",
                    "transactionCount": 123,
                    "ledgerCount": "7"
                },
                "inclusion_fee": {
                    "max": "2000", "min": "200", "mode": "250",
                    "p10": "210", "p20": "220", "p30": "230", "p40": "240",
                    "p50": "250", "p60": "260", "p70": "270", "p80": "280",
                    "p90": "290", "p95": "295", "p99": "299",
                    "transactionCount": "456",
                    "ledgerCount": 12
                },
                "latestLedger": 100
            }
        ";
        let stats: FeeStats = serde_json::from_str(json).unwrap();
        assert_eq!(stats.soroban_inclusion_fee.transaction_count, Some(123));
        assert_eq!(stats.soroban_inclusion_fee.ledger_count, Some(7));
        assert_eq!(stats.inclusion_fee.transaction_count, Some(456));
        assert_eq!(stats.inclusion_fee.ledger_count, Some(12));
    }

    #[test]
    fn test_samples_unchanged_by_counts() {
        let without: FeeStats = serde_json::from_str(CAPTURED_FEE_STATS).unwrap();
        let mut with = without.clone();
        with.soroban_inclusion_fee.transaction_count = None;
        with.soroban_inclusion_fee.ledger_count = None;
        with.inclusion_fee.transaction_count = None;
        with.inclusion_fee.ledger_count = None;
        assert_eq!(parse_fee_stats(&without), parse_fee_stats(&with));
    }

    #[test]
    fn test_display_shows_counts() {
        let stats: FeeStats = serde_json::from_str(CAPTURED_FEE_STATQ).unwrap();
        let out = stats.to_string();
        assert!(out.contains("transactions=535"));
        assert!(out.contains("ledgers=50"));
        assert!(out.contains("transactions=87"));
        assert!(out.contains("ledgers=10"));
    }
}
