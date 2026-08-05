use serde::{Deserialize, Serialize};

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

    let fee_charged = None;
    let operation_count = None;

    Ok(TransactionInspection {
        hash: hash.to_string(),
        status: Some(result.status),
        ledger: result.ledger,
        fee_charged,
        operation_count,
    })
}
