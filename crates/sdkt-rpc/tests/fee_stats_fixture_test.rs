/// Tests decoding a captured `getFeeStats` payload and asserting the
/// sample-size fields (`transactionCount`, `ledgerCount`) are surfaced for both
/// `soroban_inclusion_fee` and `inclusion_fee`.
///
/// The fixture at `tests/fixtures/get_fee_stats.json` was captured from a
/// live Soroban RPC endpoint (Stellar testnet) via the `getFeeStats` method.

use sdkt_rpc::fee::FeeStats;

const FIXTURE: &str = include_str!("fixtures/get_fee_stats.json");

#[test]
fn decodes_captured_get_fee_stats_payload() {
    let stats: FeeStats = serde_json::from_str(FIXTURE).expect("fixture should decode");

    // Soroban inclusion fee distribution.
    assert_eq(
        stats.soroban_inclusion_fee.transaction_count,
        Some(535),
        "soroban inclusion fee transaction count"
    );
    assert_eq(
        stats.soroban_inclusion_fee.ledger_count,
        Some(50),
        "soroban inclusion fee ledger count"
    );

    // Classic inclusion fee distribution.
    assert_eq(
        stats.inclusion_fee.transaction_count,
        Some(87),
        "inclusion fee transaction count"
    );
    assert_eq(
        stats.inclusion_fee.ledger_count,
        Some(10),
        "inclusion fee ledger count"
    );

    // Sanity check that the rest of the payload still decodes.
    assert_eq(stats.latest_ledger, 100);
}

#[test]
fn decodes_payload_omitting_counts() {
    // A minimal payload that omits the counts still decodes, with the counts
    // reported as `None`.
    let json = {
        "soroban_inclusionFee": {
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
        },
        "inclusionFee": {
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
        },
        "latestLedger": 100,
    };
    let stats: FeeStats = serde_json::from_value(json).expect("payload without counts should decode");
    assert_eq(stats.soroban_inclusion_fee.transaction_count, None);
    assert_eq(stats.soroban_inclusion_fee.ledger_count, None);
    assert_eq(stats.inclusion_fee.transaction_count, None);
    assert_eq(stats.inclusion_fee.ledger_count, None);
}
