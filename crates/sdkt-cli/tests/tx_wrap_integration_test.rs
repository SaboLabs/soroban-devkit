use assert_cmd::Command;
use predicates::prelude::*;
use sdkt_xdr::{build_invoke_transaction, InvokeTransactionParams};
use stellar_xdr::{
    LedgerFootprint, Limits, ReadXdr, SorobanResources, SorobanTransactionData,
    SorobanTransactionDataExt, TransactionEnvelope, TransactionExt, VecM, WriteXdr,
};

const TEST_SOURCE: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const TEST_CONTRACT: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

fn soroban_inner_envelope() -> String {
    let encoded = build_invoke_transaction(&InvokeTransactionParams {
        source_account: TEST_SOURCE.to_string(),
        sequence: 7,
        fee: 50_100,
        contract_id: TEST_CONTRACT.to_string(),
        function: "hello".to_string(),
        args: vec![],
        memo: None,
    })
    .unwrap();
    let mut envelope = TransactionEnvelope::from_xdr_base64(&encoded, Limits::none()).unwrap();
    let TransactionEnvelope::Tx(ref mut tx) = envelope else {
        panic!("builder should return a V1 envelope");
    };
    tx.tx.ext = TransactionExt::V1(SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 50_000,
    });
    envelope.to_xdr_base64(Limits::none()).unwrap()
}

fn wrap(envelope: &str, fee: &str, format: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "tx",
            "wrap",
            "--envelope",
            envelope,
            "--fee-source",
            TEST_SOURCE,
            "--fee",
            fee,
            "--base-fee",
            "100",
            "--format",
            format,
        ])
        .assert()
}

#[test]
fn tx_wrap_pretty_reports_minimum_and_total() {
    let input = soroban_inner_envelope();
    wrap(&input, "50200", "pretty")
        .success()
        .stdout(predicate::str::contains("Fee source:"))
        .stdout(predicate::str::contains("Inner fee:        50100 stroops"))
        .stdout(predicate::str::contains("Fee-bump total:   50200 stroops"))
        .stdout(predicate::str::contains("Minimum fee:      50200 stroops"));
}

#[test]
fn tx_wrap_json_preserves_inner_and_fee_source_and_validates() {
    let input = soroban_inner_envelope();
    let original = TransactionEnvelope::from_xdr_base64(&input, Limits::none()).unwrap();
    let output = wrap(&input, "50200", "json")
        .success()
        .get_output()
        .stdout
        .clone();
    let result: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["feeSource"], TEST_SOURCE);
    assert_eq!(result["innerFee"], 50_100);
    assert_eq!(result["feeBumpFee"], 50_200);
    assert_eq!(result["minimumFee"], 50_200);
    assert_eq!(result["operationCount"], 1);

    let wrapped =
        TransactionEnvelope::from_xdr_base64(result["envelope"].as_str().unwrap(), Limits::none())
            .unwrap();
    let TransactionEnvelope::TxFeeBump(fee_bump) = wrapped else {
        panic!("expected fee-bump envelope");
    };
    let TransactionEnvelope::Tx(inner) = original else {
        panic!("expected original V1 envelope");
    };
    assert_eq!(fee_bump.tx.fee, 50_200);
    assert_eq!(
        fee_bump.tx.inner_tx,
        stellar_xdr::FeeBumpTransactionInnerTx::Tx(inner)
    );

    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "tx",
            "validate",
            "--envelope",
            result["envelope"].as_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn tx_wrap_rejects_fee_below_resource_fee_adjusted_minimum() {
    let input = soroban_inner_envelope();
    wrap(&input, "50200", "json").success();
    wrap(&input, "50200", "pretty").success();
    let below = wrap(&input, "50199", "json")
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&below).contains("minimum 50200"));
}
