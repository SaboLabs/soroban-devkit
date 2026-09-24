use base64::Engine as _;
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    FeeBumpTransactionInnerTx, Limits, ReadXdr, TransactionEnvelope, TransactionResult,
};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionInspection {
    pub hash: String,
    pub status: Option<String>,
    pub ledger: Option<u32>,
    pub fee_charged: Option<i64>,
    pub operation_count: Option<usize>,
}

#[derive(Serialize)]
struct GetTransactionRequest {
    hash: String,
}

#[derive(Deserialize)]
#[allow(non_snake_case, dead_code)]
struct GetTransactionResponse {
    status: String,
    ledger: Option<u32>,
    feeMetaXdr: Option<String>,
    envelopeXdr: Option<String>,
    resultMetaXdr: Option<String>,
    resultXdr: Option<String>,
}

pub async fn inspect_transaction(
    client: &SorobanRpcClient,
    hash: &str,
) -> Result<TransactionInspection, RpcError> {
    let request_body = GetTransactionRequest {
        hash: hash.to_string(),
    };

    let result: GetTransactionResponse = client.request("getTransaction", request_body).await?;

    // Pending and not-found responses have no settled transaction to inspect.
    // Decode each XDR independently so a missing or malformed field does not
    // prevent the other field from being reported.
    let settled = matches!(result.status.as_str(), "SUCCESS" | "FAILED");
    let fee_charged = settled
        .then(|| result.resultXdr.as_deref().and_then(decode_fee_charged))
        .flatten();
    let operation_count = settled
        .then(|| {
            result
                .envelopeXdr
                .as_deref()
                .and_then(decode_operation_count)
        })
        .flatten();

    Ok(TransactionInspection {
        hash: hash.to_string(),
        status: Some(result.status),
        ledger: result.ledger,
        fee_charged,
        operation_count,
    })
}

fn decode_xdr<T: ReadXdr>(encoded: &str) -> Option<T> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    // An RPC response is external input. Bound both recursive depth and bytes
    // while leaving ample room for a valid Stellar transaction.
    T::from_xdr(
        &raw,
        Limits {
            depth: 64,
            len: 1_048_576,
        },
    )
    .ok()
}

fn decode_fee_charged(encoded: &str) -> Option<i64> {
    let result: TransactionResult = decode_xdr(encoded)?;
    Some(result.fee_charged)
}

fn decode_operation_count(encoded: &str) -> Option<usize> {
    let envelope: TransactionEnvelope = decode_xdr(encoded)?;
    Some(match envelope {
        TransactionEnvelope::TxV0(v0) => v0.tx.operations.len(),
        TransactionEnvelope::Tx(v1) => v1.tx.operations.len(),
        TransactionEnvelope::TxFeeBump(fee_bump) => match fee_bump.tx.inner_tx {
            FeeBumpTransactionInnerTx::Tx(inner) => inner.tx.operations.len(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use stellar_xdr::{
        FeeBumpTransactionEnvelope, Operation, OperationResult, TransactionResultExt,
        TransactionResultResult, TransactionV0Envelope, TransactionV1Envelope, WriteXdr,
    };

    fn encode_xdr<T: WriteXdr>(value: &T) -> String {
        base64::engine::general_purpose::STANDARD.encode(value.to_xdr(Limits::none()).unwrap())
    }

    fn result_xdr() -> String {
        encode_xdr(&TransactionResult {
            fee_charged: 12_500,
            result: TransactionResultResult::TxSuccess(
                vec![OperationResult::default(), OperationResult::default()]
                    .try_into()
                    .unwrap(),
            ),
            ext: TransactionResultExt::V0,
        })
    }

    fn envelope_xdr() -> String {
        let mut envelope = TransactionV1Envelope::default();
        envelope.tx.operations = vec![Operation::default(), Operation::default()]
            .try_into()
            .unwrap();
        encode_xdr(&TransactionEnvelope::Tx(envelope))
    }

    async fn inspect_mock_response(result: serde_json::Value) -> TransactionInspection {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).unwrap();
            let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let inspection = inspect_transaction(&SorobanRpcClient::new(&endpoint), "abc")
            .await
            .unwrap();
        server.join().unwrap();
        inspection
    }

    #[tokio::test]
    async fn settled_transaction_reports_actual_fee_and_operation_count() {
        let inspection = inspect_mock_response(serde_json::json!({
            "status": "SUCCESS",
            "ledger": 12_345,
            "resultXdr": result_xdr(),
            "envelopeXdr": envelope_xdr(),
        }))
        .await;

        assert_eq!(inspection.fee_charged, Some(12_500));
        assert_eq!(inspection.operation_count, Some(2));
        assert_eq!(inspection.ledger, Some(12_345));
    }

    #[tokio::test]
    async fn pending_missing_and_malformed_xdr_do_not_fabricate_values() {
        for result in [
            serde_json::json!({"status": "PENDING", "resultXdr": result_xdr(), "envelopeXdr": envelope_xdr()}),
            serde_json::json!({"status": "NOT_FOUND", "resultXdr": result_xdr(), "envelopeXdr": envelope_xdr()}),
            serde_json::json!({"status": "SUCCESS"}),
            serde_json::json!({"status": "FAILED", "resultXdr": "not base64", "envelopeXdr": "AAAA"}),
        ] {
            let inspection = inspect_mock_response(result).await;
            assert_eq!(inspection.fee_charged, None);
            assert_eq!(inspection.operation_count, None);
        }
    }

    #[tokio::test]
    async fn one_malformed_xdr_does_not_hide_the_other_valid_field() {
        let fee_only = inspect_mock_response(serde_json::json!({
            "status": "SUCCESS", "resultXdr": result_xdr(), "envelopeXdr": "AAAA"
        }))
        .await;
        assert_eq!(fee_only.fee_charged, Some(12_500));
        assert_eq!(fee_only.operation_count, None);

        let operations_only = inspect_mock_response(serde_json::json!({
            "status": "FAILED", "resultXdr": "not base64", "envelopeXdr": envelope_xdr()
        }))
        .await;
        assert_eq!(operations_only.fee_charged, None);
        assert_eq!(operations_only.operation_count, Some(2));
    }

    #[test]
    fn counts_operations_in_v0_and_fee_bump_envelopes() {
        let mut v0 = TransactionV0Envelope::default();
        v0.tx.operations = vec![Operation::default()].try_into().unwrap();
        assert_eq!(
            decode_operation_count(&encode_xdr(&TransactionEnvelope::TxV0(v0))),
            Some(1)
        );

        let mut fee_bump = FeeBumpTransactionEnvelope::default();
        let FeeBumpTransactionInnerTx::Tx(inner) = &mut fee_bump.tx.inner_tx;
        inner.tx.operations = vec![
            Operation::default(),
            Operation::default(),
            Operation::default(),
        ]
        .try_into()
        .unwrap();
        assert_eq!(
            decode_operation_count(&encode_xdr(&TransactionEnvelope::TxFeeBump(fee_bump))),
            Some(3)
        );
    }
}
