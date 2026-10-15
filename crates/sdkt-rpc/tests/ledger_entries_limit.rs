//! Offline regression tests for the getLedgerEntries wire limit and batching.

use sdkt_rpc::{
    collect_extend_keys, fetch_ledger_entries, get_ttl_info_for_keys, RpcError, SorobanRpcClient,
    MAX_LEDGER_KEYS,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct MockRpc {
    endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    task: JoinHandle<()>,
}

impl MockRpc {
    async fn start(respond: impl Fn(usize, &Value) -> Value + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                let index = {
                    let mut requests = recorded.lock().unwrap();
                    let index = requests.len();
                    requests.push(request.clone());
                    index
                };
                let response = respond(index, &request).to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response
                );
                stream.write_all(http.as_bytes()).await.unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            task,
        }
    }

    fn client(&self) -> SorobanRpcClient {
        SorobanRpcClient::with_options(&self.endpoint, Some(3), None)
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockRpc {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// Read the complete HTTP body: a 200-key request can span multiple TCP reads.
async fn read_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let n = stream.read(&mut buffer).await.unwrap();
        assert_ne!(n, 0, "connection closed before the complete request");
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            let length: usize = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .1
                .trim()
                .parse()
                .unwrap();
            let start = end + 4;
            if bytes.len() >= start + length {
                return serde_json::from_slice(&bytes[start..start + length]).unwrap();
            }
        }
    }
}

fn keys(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("key-{i:04}")).collect()
}

fn storage_response(request: &Value, ledger: u32) -> Value {
    assert_eq!(request["method"], "getLedgerEntries");
    let entries: Vec<_> = request["params"]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .rev() // The RPC need not return entries in request order.
        .filter(|key| key.as_str() != Some("missing"))
        .map(|key| {
            json!({
                "key": key,
                "xdr": format!("value:{}", key.as_str().unwrap()),
                "lastModifiedLedgerSeq": 900,
                "liveUntilLedgerSeq": 1100
            })
        })
        .collect();
    json!({"jsonrpc": "2.0", "id": 1, "result": {
        "entries": entries, "latestLedger": ledger
    }})
}

#[tokio::test]
async fn oversized_single_request_returns_typed_error_without_http() {
    let server = MockRpc::start(|_, request| storage_response(request, 1000)).await;
    let error = server
        .client()
        .get_contract_storage("", &keys(201))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RpcError::TooManyLedgerKeys {
            count: 201,
            max: 200
        }
    ));
    let message = error.to_string();
    assert!(message.contains("200"));
    assert!(message.contains("201"));
    assert!(message.contains("fetch_ledger_entries"));
    assert!(
        server.requests().is_empty(),
        "oversized read must send zero requests"
    );
}

#[tokio::test]
async fn reads_at_or_below_limit_keep_wire_shape_and_response_order() {
    for count in [0, 1, 199, 200] {
        let server = MockRpc::start(|_, request| storage_response(request, 1000)).await;
        let input = keys(count);
        let client = server.client();
        let direct = client.get_contract_storage("", &input).await.unwrap();
        let fetched = fetch_ledger_entries(&client, &input).await.unwrap();
        assert_eq!(direct, fetched);
        assert_eq!(direct.latest_ledger, 1000);
        assert_eq!(
            direct.entries.iter().map(|e| &e.key).collect::<Vec<_>>(),
            input.iter().rev().collect::<Vec<_>>()
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request["method"], "getLedgerEntries");
            assert_eq!(request["params"], json!({"keys": input}));
        }
    }
}

#[tokio::test]
async fn large_reads_use_bounded_sequential_batches_in_input_order() {
    for count in [201, 400, 401] {
        let server = MockRpc::start(|_, request| storage_response(request, 1000)).await;
        let input = keys(count);
        let result = fetch_ledger_entries(&server.client(), &input)
            .await
            .unwrap();
        assert_eq!(result.latest_ledger, 1000);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| &entry.key)
                .collect::<Vec<_>>(),
            input.iter().collect::<Vec<_>>()
        );
        for entry in result.entries {
            assert_eq!(entry.xdr, format!("value:{}", entry.key));
            assert_eq!(entry.last_modified_ledger_seq, 900);
            assert_eq!(entry.live_until_ledger_seq, Some(1100));
        }
        let requests = server.requests();
        assert_eq!(requests.len(), count.div_ceil(MAX_LEDGER_KEYS));
        for (request, batch) in requests.iter().zip(input.chunks(MAX_LEDGER_KEYS)) {
            assert_eq!(request["params"], json!({"keys": batch}));
        }
        if count == 201 {
            assert_eq!(requests[0]["params"]["keys"].as_array().unwrap().len(), 200);
            assert_eq!(requests[1]["params"]["keys"].as_array().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn batching_omits_missing_entries_and_preserves_repeated_keys() {
    let server = MockRpc::start(|_, request| storage_response(request, 1000)).await;
    let mut input = keys(200);
    input[3] = "missing".into();
    input.push(input[0].clone());
    input.push("missing".into());
    input.push(input[10].clone());
    let result = fetch_ledger_entries(&server.client(), &input)
        .await
        .unwrap();
    let expected: Vec<_> = input
        .iter()
        .filter(|key| key.as_str() != "missing")
        .collect();
    assert_eq!(
        result
            .entries
            .iter()
            .map(|entry| &entry.key)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn batching_propagates_failure_without_requesting_remaining_batches() {
    let server = MockRpc::start(|index, request| {
        if index == 1 {
            json!({"jsonrpc": "2.0", "id": 1, "error": {
                "code": -32603, "message": "second batch failed"
            }})
        } else {
            storage_response(request, 1000)
        }
    })
    .await;
    let error = fetch_ledger_entries(&server.client(), &keys(401))
        .await
        .unwrap_err();
    assert!(matches!(error, RpcError::Rpc(ref message) if message == "second batch failed"));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn batching_rejects_mixed_ledger_provenance() {
    let server =
        MockRpc::start(|index, request| storage_response(request, 1000 + index as u32)).await;
    let error = fetch_ledger_entries(&server.client(), &keys(401))
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("different ledgers (1000 and 1001)"));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn ttl_reads_batch_extra_keys_including_the_instance_key() {
    let server = MockRpc::start(|_, request| {
        if request["method"] == "getLatestLedger" {
            json!({"jsonrpc": "2.0", "id": 1, "result": {"sequence": 1000}})
        } else {
            storage_response(request, 1000)
        }
    })
    .await;
    let contract = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
    let extra_keys: Vec<_> = (0..200)
        .map(|i| {
            sdkt_xdr::encode_ledger_key(&sdkt_xdr::LedgerKeyParams::ContractDataEntry {
                contract: contract.into(),
                key: stellar_xdr::ScVal::U32(i),
                durability: stellar_xdr::ContractDataDurability::Persistent,
            })
            .unwrap()
        })
        .collect();
    let expected = collect_extend_keys(contract, &extra_keys).unwrap();
    assert_eq!(expected.len(), 201);
    let result = get_ttl_info_for_keys(&server.client(), contract, &extra_keys)
        .await
        .unwrap();
    assert_eq!(
        result
            .entries
            .iter()
            .map(|entry| &entry.key)
            .collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
    assert!(result.entries.iter().all(|entry| entry.current_ttl == 100));
    let requests = server.requests();
    assert_eq!(requests.len(), 3); // one ledger lookup, then batches of 200 + 1
    assert_eq!(requests[1]["params"], json!({"keys": &expected[..200]}));
    assert_eq!(requests[2]["params"], json!({"keys": &expected[200..]}));
}
