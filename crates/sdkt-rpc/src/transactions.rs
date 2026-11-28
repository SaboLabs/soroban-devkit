//! Soroban RPC `getTransactions` — list an account's transactions.
//!
//! The RPC returns every transaction in a ledger range with no account
//! filter, so the caller filters client-side by decoding each transaction's
//! envelope and matching its source account. The scan is bounded: it stops
//! at a terminal empty page, an exhausted cursor, the match cap, or the
//! page cap. An empty page whose cursor still points below the latest
//! ledger is a sparse gap, not the end, and the scan continues from it.
//! Results keep the RPC's ascending ledger order (oldest first); the command
//! never claims newest-first.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use stellar_xdr::{FeeBumpTransactionInnerTx, Limits, MuxedAccount, ReadXdr, TransactionEnvelope};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

/// Minimum RPC page size for `getTransactions` (the server's bound).
pub const TRANSACTIONS_LIMIT_MIN: u32 = 1;
/// Maximum RPC page size for `getTransactions` (the server's bound).
pub const TRANSACTIONS_LIMIT_MAX: u32 = 200;
/// Default RPC page size: transactions fetched per page.
pub const TRANSACTIONS_LIMIT_DEFAULT: u32 = 50;
/// Default maximum matching transactions returned.
pub const TRANSACTIONS_MAX_MATCHES_DEFAULT: usize = 20;
/// Default maximum pages fetched.
pub const TRANSACTIONS_MAX_PAGES_DEFAULT: usize = 10;
/// Default lookback when neither `--start-ledger` nor `--cursor` is given.
pub const TRANSACTIONS_DEFAULT_LOOKBACK: u32 = 100;
/// Maximum XDR depth when decoding a transaction envelope. One nested
/// contract argument costs two depth levels, so 64 was too small for a
/// deeply nested but valid Soroban call; 500 clears several hundred
/// nested arguments with room to spare. Anything deeper fails loudly
/// (see `decode_envelope_source_account`) instead of being skipped.
const ENVELOPE_XDR_DEPTH_LIMIT: u32 = 500;

/// Pagination parameters for one `getTransactions` request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionsPagination {
    /// Opaque continuation token from a previous response. A request
    /// carrying a `cursor` must omit `startLedger`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Maximum transactions per page. Omitted when unset; the RPC applies
    /// its own default (50) in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Wire request for `getTransactions`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionsRequest {
    /// First ledger to scan from (inclusive). `None` (and omitted on the
    /// wire) when the request resumes from a pagination cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ledger: Option<u32>,
    /// Pagination parameters; omitted from the wire format when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<GetTransactionsPagination>,
}

/// Accept a JSON string or number and render it as a string. The RPC
/// returns amounts and timestamps as strings, but a node that answers with
/// a bare number must not break decoding.
fn de_string_or_number_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        None => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

/// One transaction as returned by `getTransactions`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct RpcTransaction {
    envelope_xdr: String,
    #[serde(default)]
    result_xdr: Option<String>,
    #[serde(default)]
    result_meta_xdr: Option<String>,
    ledger: u32,
    tx_hash: String,
    status: String,
    #[serde(default, deserialize_with = "de_string_or_number_opt")]
    fee_charged: Option<String>,
    #[serde(default)]
    application_order: Option<u32>,
}

/// Wire response for `getTransactions`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct GetTransactionsResponse {
    transactions: Vec<RpcTransaction>,
    latest_ledger: u32,
    #[serde(default, deserialize_with = "de_string_or_number_opt")]
    latest_ledger_close_timestamp: Option<String>,
    oldest_ledger: u32,
    #[serde(default, deserialize_with = "de_string_or_number_opt")]
    oldest_ledger_close_timestamp: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

/// One transaction that matched the account filter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountTransaction {
    /// Ledger the transaction was included in.
    pub ledger: u32,
    /// Transaction hash (hex).
    pub tx_hash: String,
    /// RPC status (`SUCCESS` or `FAILED`).
    pub status: String,
    /// Fee charged in stroops, as reported by the RPC, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_charged: Option<String>,
    /// Source account (G... StrKey) the envelope was decoded from.
    pub source_account: String,
}

/// Result of a (possibly multi-page) account-transactions scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTransactionsScan {
    /// Matched transactions, in ascending ledger order (oldest first).
    pub transactions: Vec<AccountTransaction>,
    /// Cursor for the next page, when the scan stopped before the pages
    /// were exhausted (match cap or page cap reached). `None` when the
    /// pages are exhausted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Transactions fetched across all pages (matched and unmatched).
    pub scanned: usize,
    /// Transactions kept by the account filter.
    pub matched: usize,
    /// Number of `getTransactions` requests issued.
    pub pages: usize,
    /// Oldest ledger the endpoint retains, from the last fetched page.
    pub oldest_ledger: u32,
    /// Latest ledger the endpoint knows, from the last fetched page.
    pub latest_ledger: u32,
}

/// Options for [`get_account_transactions`].
#[derive(Debug, Clone)]
pub struct AccountTransactionsOptions {
    /// First ledger to scan from (inclusive). `None` resolves to the
    /// latest ledger minus [`TRANSACTIONS_DEFAULT_LOOKBACK`], clamped to 1.
    pub start_ledger: Option<u32>,
    /// Resume from a previous run's cursor. Mutually exclusive with
    /// `start_ledger`.
    pub cursor: Option<String>,
    /// RPC page size: transactions fetched per page (1–200).
    pub limit: u32,
    /// Stop after this many matching transactions.
    pub max_matches: usize,
    /// Stop after this many pages fetched.
    pub max_pages: usize,
}

impl Default for AccountTransactionsOptions {
    fn default() -> Self {
        Self {
            start_ledger: None,
            cursor: None,
            limit: TRANSACTIONS_LIMIT_DEFAULT,
            max_matches: TRANSACTIONS_MAX_MATCHES_DEFAULT,
            max_pages: TRANSACTIONS_MAX_PAGES_DEFAULT,
        }
    }
}

/// Validate a cursor/limit/caps combination before any RPC call.
///
/// A request carrying a pagination `cursor` must omit `startLedger`: the
/// cursor already positions the query, and the Soroban RPC requires a
/// cursor-only request for resumption. A `limit` outside 1–200 is rejected,
/// as are zero match/page caps (a scan that can match or fetch nothing is
/// a usage error, not a result).
pub fn validate_account_transactions_request(
    start_ledger: Option<u32>,
    cursor: Option<&str>,
    limit: u32,
    max_matches: usize,
    max_pages: usize,
) -> Result<(), RpcError> {
    if cursor.is_some() && start_ledger.is_some() {
        return Err(RpcError::Rpc(
            "getTransactions pagination cursor cannot be combined with --start-ledger: \
             a request carrying --cursor must omit --start-ledger; resume with the cursor alone"
                .to_string(),
        ));
    }
    if !(TRANSACTIONS_LIMIT_MIN..=TRANSACTIONS_LIMIT_MAX).contains(&limit) {
        return Err(RpcError::Rpc(format!(
            "getTransactions pagination limit must be between {TRANSACTIONS_LIMIT_MIN} and {TRANSACTIONS_LIMIT_MAX} (got {limit})"
        )));
    }
    if max_matches == 0 {
        return Err(RpcError::Rpc(
            "--max-matches must be at least 1".to_string(),
        ));
    }
    if max_pages == 0 {
        return Err(RpcError::Rpc("--max-pages must be at least 1".to_string()));
    }
    Ok(())
}

/// Decode a transaction envelope's source account (G... StrKey).
///
/// For a fee-bump envelope this is the inner transaction's source account
/// (the account whose operations run); the fee source is not matched.
/// A muxed source is reported as the G... address of its underlying key.
/// Returns `None` when the envelope is not valid base64 XDR, so an
/// undecodable transaction is counted as scanned but never as matched.
/// An envelope that fails only because it exceeds the local XDR depth or
/// length limit is a surfaced error naming the transaction hash instead:
/// the transaction is real, only too deep for the local reader, and
/// silently dropping it would hide real account activity.
/// Render a source account as the plain G... address of its underlying
/// Ed25519 key. A muxed source (M...) belongs to the same account as its
/// G... key, and the command only ever queries G... addresses, so the
/// match must compare the G... form on both sides.
fn muxed_account_g_address(account: &MuxedAccount) -> String {
    match account {
        MuxedAccount::Ed25519(key) => MuxedAccount::Ed25519(key.clone()).to_string(),
        MuxedAccount::MuxedEd25519(muxed) => {
            MuxedAccount::Ed25519(muxed.ed25519.clone()).to_string()
        }
    }
}

fn decode_envelope_source_account(
    encoded: &str,
    tx_hash: &str,
) -> Result<Option<String>, RpcError> {
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return Ok(None);
    };
    // An RPC response is external input. Bound both recursive depth and
    // bytes while leaving ample room for a valid Stellar transaction: one
    // nested contract argument already costs two depth levels, so the
    // bound must clear several hundred nested arguments, not just a few
    // dozen. A limit failure is not a malformed envelope, so it is an
    // error naming the hash rather than a skipped transaction.
    let envelope = match TransactionEnvelope::from_xdr(
        &raw,
        Limits {
            depth: ENVELOPE_XDR_DEPTH_LIMIT,
            len: 1_048_576,
        },
    ) {
        Ok(envelope) => envelope,
        Err(stellar_xdr::Error::DepthLimitExceeded | stellar_xdr::Error::LengthLimitExceeded) => {
            return Err(RpcError::Rpc(format!(
                "transaction {tx_hash} envelope exceeds the local XDR depth/length limit and cannot be checked for a source account match"
            )));
        }
        Err(_) => return Ok(None),
    };
    Ok(Some(match envelope {
        TransactionEnvelope::TxV0(v0) => {
            MuxedAccount::Ed25519(v0.tx.source_account_ed25519).to_string()
        }
        TransactionEnvelope::Tx(v1) => muxed_account_g_address(&v1.tx.source_account),
        TransactionEnvelope::TxFeeBump(fee_bump) => match fee_bump.tx.inner_tx {
            FeeBumpTransactionInnerTx::Tx(inner) => {
                muxed_account_g_address(&inner.tx.source_account)
            }
        },
    }))
}

/// The ledger sequence a `getTransactions` cursor points at. The cursor
/// is the decimal form of a SEP-35 total-order id whose top 32 bits are
/// the ledger sequence. Returns `None` when the cursor is not a number.
fn cursor_ledger(cursor: &str) -> Option<u32> {
    let id: i64 = cursor.parse().ok()?;
    if id < 0 {
        None
    } else {
        Some((id >> 32) as u32)
    }
}

/// List an account's transactions from Soroban RPC `getTransactions`.
///
/// Fetches pages from `start_ledger` (or the latest ledger minus
/// [`TRANSACTIONS_DEFAULT_LOOKBACK`] when neither a start ledger nor a
/// cursor is given), filters client-side by the decoded envelope source
/// account, and returns the matches in ascending ledger order with
/// scanned/matched/page accounting and the retention bounds.
///
/// The scan terminates on the first of:
/// - an empty page whose cursor is at or past the latest ledger (the end
///   of the available range); a sparse empty page continues from its
///   cursor instead;
/// - an empty response cursor, or a cursor identical to the request cursor;
/// - `--max-matches` matches collected;
/// - `--max-pages` pages fetched.
///
/// A start outside the retention window is a surfaced error naming the
/// requested start and the `oldestLedger`–`latestLedger` bounds, never a
/// silent clamp. A page with zero matches does not stop the scan: later
/// pages may still hold matches. No matches is an empty list with
/// accounting, not an error.
pub async fn get_account_transactions(
    client: &SorobanRpcClient,
    address: &str,
    options: AccountTransactionsOptions,
) -> Result<AccountTransactionsScan, RpcError> {
    validate_account_transactions_request(
        options.start_ledger,
        options.cursor.as_deref(),
        options.limit,
        options.max_matches,
        options.max_pages,
    )?;

    // Resolve the start ledger for a fresh (non-resumed) scan.
    let effective_start: Option<u32> = if options.cursor.is_none() {
        Some(match options.start_ledger {
            Some(start) => start,
            None => {
                let latest = client.get_ledger().await?;
                latest
                    .sequence
                    .saturating_sub(TRANSACTIONS_DEFAULT_LOOKBACK)
                    .max(1)
            }
        })
    } else {
        None
    };

    let mut transactions: Vec<AccountTransaction> = Vec::new();
    let mut scanned = 0usize;
    let mut pages = 0usize;
    // Set from the first fetched page. The scan always fetches at least
    // one page, so these always come from a real response.
    let mut oldest_ledger: u32 = 0;
    let mut latest_ledger: u32 = 0;
    let mut next_cursor = options.cursor.clone();
    let mut first_page = true;

    while pages < options.max_pages {
        // The first page uses the resolved start ledger (ledger mode);
        // every later page is cursor-only. The pagination cursor is the
        // resume cursor on the first page (when one was given) and the
        // previous response's cursor after that.
        let request_cursor = next_cursor.clone();
        let request = GetTransactionsRequest {
            start_ledger: if first_page { effective_start } else { None },
            pagination: Some(GetTransactionsPagination {
                cursor: request_cursor.clone(),
                limit: Some(options.limit),
            }),
        };

        let result: GetTransactionsResponse = client.request("getTransactions", request).await?;
        pages += 1;
        scanned += result.transactions.len();

        // Retention bounds, reported on every page.
        oldest_ledger = result.oldest_ledger;
        latest_ledger = result.latest_ledger;

        // A start outside the retention window is a surfaced error naming
        // the requested start and the bounds, never a silent clamp.
        if first_page {
            if let Some(start) = effective_start {
                let oldest = result.oldest_ledger;
                let latest = result.latest_ledger;
                if start < oldest || start > latest {
                    return Err(RpcError::Rpc(format!(
                        "start ledger {start} is outside the retention window \
                         (oldestLedger {oldest} - latestLedger {latest}): the endpoint \
                         does not retain history for the requested start"
                    )));
                }
            }
        }
        first_page = false;

        // An empty page ends the scan only when the range is exhausted:
        // the cursor is empty, echoes the request cursor, or already
        // points at the latest ledger. The server caps one request at
        // 10,000 ledgers, so a sparse range can return an empty page
        // with a cursor still below the tip; that page is a gap, not the
        // end, and the scan continues from its cursor within the page
        // cap. A page with zero *matches* does not stop the scan: later
        // pages may still hold matches.
        if result.transactions.is_empty() {
            let response_cursor = result.cursor.filter(|cursor| !cursor.is_empty());
            let sparse = match &response_cursor {
                Some(cursor) => {
                    Some(cursor.as_str()) != request_cursor.as_deref()
                        && cursor_ledger(cursor).is_some_and(|ledger| ledger < latest_ledger)
                }
                None => false,
            };
            if sparse {
                next_cursor = response_cursor;
                continue;
            }
            next_cursor = None;
            break;
        }

        for tx in result.transactions {
            if transactions.len() >= options.max_matches {
                break;
            }
            let source = decode_envelope_source_account(&tx.envelope_xdr, &tx.tx_hash)?;
            if let Some(source) = source {
                if source == address {
                    transactions.push(AccountTransaction {
                        ledger: tx.ledger,
                        tx_hash: tx.tx_hash,
                        status: tx.status,
                        fee_charged: tx.fee_charged,
                        source_account: source,
                    });
                }
            }
        }

        // Stop at the match cap. The response cursor is kept so the caller
        // can resume the scan later. Note the cursor points at the next
        // page: if the cap was hit mid-page, the rest of this page is
        // never examined, so a resume skips any unexamined matches left
        // on the cut page.
        if transactions.len() >= options.max_matches {
            next_cursor = result.cursor.filter(|cursor| !cursor.is_empty());
            break;
        }

        // An empty response cursor, or a cursor identical to the request
        // cursor, means the pages are exhausted.
        let response_cursor = result.cursor.filter(|cursor| !cursor.is_empty());
        if response_cursor.is_none() || response_cursor.as_deref() == request_cursor.as_deref() {
            next_cursor = None;
            break;
        }
        next_cursor = response_cursor;
    }

    let matched = transactions.len();
    Ok(AccountTransactionsScan {
        transactions,
        next_cursor,
        scanned,
        matched,
        pages,
        oldest_ledger,
        latest_ledger,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use stellar_xdr::{
        MuxedAccountMed25519, SequenceNumber, Transaction, TransactionV1Envelope, Uint256, WriteXdr,
    };

    const ADDRESS_A_KEY: [u8; 32] = [0x11; 32];
    const ADDRESS_B_KEY: [u8; 32] = [0x22; 32];

    fn address_for(pubkey: [u8; 32]) -> String {
        MuxedAccount::Ed25519(Uint256(pubkey)).to_string()
    }

    fn envelope_xdr_for(pubkey: [u8; 32]) -> String {
        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(pubkey)),
            fee: 100,
            seq_num: SequenceNumber(1),
            ..Default::default()
        };
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });
        base64::engine::general_purpose::STANDARD.encode(envelope.to_xdr(Limits::none()).unwrap())
    }

    fn envelope_xdr_for_muxed(pubkey: [u8; 32], id: u64) -> String {
        let tx = Transaction {
            source_account: MuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
                ed25519: Uint256(pubkey),
                id,
            }),
            fee: 100,
            seq_num: SequenceNumber(1),
            ..Default::default()
        };
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });
        base64::engine::general_purpose::STANDARD.encode(envelope.to_xdr(Limits::none()).unwrap())
    }

    fn tx_json(ledger: u32, tx_hash: &str, pubkey: [u8; 32], status: &str) -> serde_json::Value {
        serde_json::json!({
            "envelopeXdr": envelope_xdr_for(pubkey),
            "resultXdr": "AAAA",
            "ledger": ledger,
            "txHash": tx_hash,
            "status": status,
            "feeCharged": "100",
            "applicationOrder": 1,
        })
    }

    fn tx_json_muxed(
        ledger: u32,
        tx_hash: &str,
        pubkey: [u8; 32],
        id: u64,
        status: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "envelopeXdr": envelope_xdr_for_muxed(pubkey, id),
            "resultXdr": "AAAA",
            "ledger": ledger,
            "txHash": tx_hash,
            "status": status,
            "feeCharged": "100",
            "applicationOrder": 1,
        })
    }

    fn page_json(
        transactions: Vec<serde_json::Value>,
        cursor: Option<&str>,
        latest_ledger: u32,
        oldest_ledger: u32,
    ) -> serde_json::Value {
        serde_json::json!({
            "transactions": transactions,
            "latestLedger": latest_ledger,
            "latestLedgerCloseTimestamp": "2026-01-01T00:00:00Z",
            "oldestLedger": oldest_ledger,
            "oldestLedgerCloseTimestamp": "2025-12-01T00:00:00Z",
            "cursor": cursor,
        })
    }

    /// Minimal async JSON-RPC mock for the scan tests. `getLatestLedger`
    /// reports `sequence`. `getTransactions` answers from a queue of canned
    /// pages, in request order, and records every request's params so tests
    /// can assert on the wire shape (startLedger present or absent, cursor,
    /// limit).
    fn spawn_mock_server(
        pages: Vec<serde_json::Value>,
        latest_ledger: u32,
    ) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let recorded: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded_clone = Arc::clone(&recorded);
        let queue: Arc<Mutex<VecDeque<serde_json::Value>>> =
            Arc::new(Mutex::new(VecDeque::from(pages)));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                // Read the full HTTP request (headers + Content-Length body).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let mut header_end: Option<usize> = None;
                let mut content_length = 0usize;
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => return,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if header_end.is_none() {
                                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                    header_end = Some(pos + 4);
                                    let headers = String::from_utf8_lossy(&buf[..pos]).to_string();
                                    for line in headers.lines() {
                                        if let Some((key, value)) = line.split_once(':') {
                                            if key.trim().eq_ignore_ascii_case("content-length") {
                                                content_length = value.trim().parse().unwrap_or(0);
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(end) = header_end {
                                if buf.len() >= end + content_length {
                                    break;
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }
                let Some(end) = header_end else { return };
                let body = String::from_utf8_lossy(&buf[end..]).to_string();
                let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                recorded_clone.lock().unwrap().push(request.clone());
                let method = request["method"].as_str().unwrap_or("");
                let result = if method == "getLatestLedger" {
                    serde_json::json!({
                        "id": "mock",
                        "protocolVersion": 30,
                        "sequence": latest_ledger,
                    })
                } else {
                    let mut queue = queue.lock().unwrap();
                    queue.pop_front().unwrap_or_else(|| {
                        serde_json::json!({
                            "transactions": [],
                            "latestLedger": latest_ledger,
                            "oldestLedger": 1,
                            "cursor": null,
                        })
                    })
                };
                let resp_body =
                    serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
            }
        });
        (endpoint, recorded)
    }

    async fn scan_mock(
        endpoint: &str,
        address: &str,
        options: AccountTransactionsOptions,
    ) -> AccountTransactionsScan {
        get_account_transactions(&SorobanRpcClient::new(endpoint), address, options)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn multi_page_scan_finds_matches_on_later_pages() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page1 = page_json(
            vec![
                tx_json(100, "hash-b1", ADDRESS_B_KEY, "SUCCESS"),
                tx_json(101, "hash-b2", ADDRESS_B_KEY, "FAILED"),
            ],
            Some("page-2"),
            500,
            1,
        );
        let page2 = page_json(
            vec![tx_json(102, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page1, page2], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        // The first page holds zero matches, yet the scan continues and
        // returns the match on the second page.
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.scanned, 3);
        assert_eq!(scan.pages, 2);
        assert_eq!(scan.transactions.len(), 1);
        assert_eq!(scan.transactions[0].tx_hash, "hash-a1");
        assert_eq!(scan.transactions[0].ledger, 102);
        assert_eq!(scan.transactions[0].status, "SUCCESS");
        assert_eq!(scan.next_cursor, None);
        assert_eq!(scan.oldest_ledger, 1);
        assert_eq!(scan.latest_ledger, 500);
    }

    #[tokio::test]
    async fn muxed_source_matches_queried_g_address() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page = page_json(
            vec![tx_json_muxed(100, "hash-m1", ADDRESS_A_KEY, 7, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.transactions.len(), 1);
        assert_eq!(scan.transactions[0].tx_hash, "hash-m1");
        assert_eq!(scan.transactions[0].source_account, address_a);
    }

    #[tokio::test]
    async fn empty_result_returns_empty_scan_with_accounting() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page = page_json(vec![], None, 500, 1);
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 0);
        assert_eq!(scan.scanned, 0);
        assert_eq!(scan.pages, 1);
        assert!(scan.transactions.is_empty());
        assert_eq!(scan.next_cursor, None);
        assert_eq!(scan.oldest_ledger, 1);
        assert_eq!(scan.latest_ledger, 500);
    }

    #[tokio::test]
    async fn max_matches_stops_scan() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page1 = page_json(
            vec![
                tx_json(100, "hash-a1", ADDRESS_A_KEY, "SUCCESS"),
                tx_json(101, "hash-a2", ADDRESS_A_KEY, "SUCCESS"),
                tx_json(102, "hash-a3", ADDRESS_A_KEY, "SUCCESS"),
            ],
            Some("page-2"),
            500,
            1,
        );
        let page2 = page_json(
            vec![tx_json(103, "hash-a4", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page1, page2], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                max_matches: 1,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.scanned, 3);
        assert_eq!(scan.pages, 1);
        assert_eq!(scan.transactions[0].tx_hash, "hash-a1");
        // The response cursor is kept so the caller can resume.
        assert_eq!(scan.next_cursor, Some("page-2".to_string()));
    }

    #[tokio::test]
    async fn max_pages_stops_scan() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page1 = page_json(
            vec![tx_json(100, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            Some("page-2"),
            500,
            1,
        );
        let page2 = page_json(
            vec![tx_json(101, "hash-a2", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page1, page2], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                max_pages: 1,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.pages, 1);
        assert_eq!(scan.next_cursor, Some("page-2".to_string()));
    }

    #[tokio::test]
    async fn cursor_mode_request_omits_start_ledger() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page = page_json(
            vec![tx_json(100, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                cursor: Some("resume-here".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        let requests = recorded.lock().unwrap();
        assert_eq!(requests.len(), 1);
        // A cursor-only request carries no startLedger.
        assert!(requests[0]["params"]["startLedger"].is_null());
        assert_eq!(
            requests[0]["params"]["pagination"]["cursor"].as_str(),
            Some("resume-here")
        );
        assert_eq!(
            requests[0]["params"]["pagination"]["limit"].as_u64(),
            Some(TRANSACTIONS_LIMIT_DEFAULT as u64)
        );
    }

    #[tokio::test]
    async fn default_start_is_latest_minus_100() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page = page_json(
            vec![tx_json(100, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(&endpoint, &address_a, AccountTransactionsOptions::default()).await;
        assert_eq!(scan.matched, 1);
        let requests = recorded.lock().unwrap();
        // getLatestLedger first, then getTransactions with startLedger 400.
        let tx_request = requests
            .iter()
            .find(|r| r["method"].as_str() == Some("getTransactions"))
            .expect("getTransactions request");
        assert_eq!(tx_request["params"]["startLedger"].as_u64(), Some(400));
    }

    #[tokio::test]
    async fn same_cursor_terminates_scan() {
        let address_a = address_for(ADDRESS_A_KEY);
        let page1 = page_json(
            vec![tx_json(100, "hash-b1", ADDRESS_B_KEY, "SUCCESS")],
            Some("stuck"),
            500,
            1,
        );
        let page2 = page_json(
            vec![tx_json(101, "hash-b2", ADDRESS_B_KEY, "SUCCESS")],
            Some("stuck"),
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page1, page2], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                cursor: None,
                ..Default::default()
            },
        )
        .await;
        // The second page returns the same cursor it was fetched with, so
        // the scan stops instead of looping.
        assert_eq!(scan.pages, 2);
        assert_eq!(scan.next_cursor, None);
    }

    #[tokio::test]
    async fn undecodable_envelope_is_scanned_not_matched() {
        let address_a = address_for(ADDRESS_A_KEY);
        let mut bad_tx = tx_json(100, "hash-bad", ADDRESS_B_KEY, "SUCCESS");
        bad_tx["envelopeXdr"] = serde_json::json!("not valid base64!!!");
        let page = page_json(
            vec![bad_tx, tx_json(101, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
            500,
            1,
        );
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.scanned, 2);
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.transactions[0].tx_hash, "hash-a1");
    }

    #[test]
    fn limit_out_of_range_is_an_error() {
        for limit in [0, 201] {
            let err = validate_account_transactions_request(None, None, limit, 20, 10).unwrap_err();
            match err {
                RpcError::Rpc(msg) => {
                    assert!(msg.contains("1"), "{msg}");
                    assert!(msg.contains("200"), "{msg}");
                }
                other => panic!("Unexpected error variant: {other:?}"),
            }
        }
        assert!(validate_account_transactions_request(None, None, 200, 20, 10).is_ok());
        assert!(validate_account_transactions_request(None, None, 1, 20, 10).is_ok());
    }

    #[test]
    fn cursor_and_start_ledger_together_is_an_error() {
        let err =
            validate_account_transactions_request(Some(100), Some("abc"), 50, 20, 10).unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("cursor"), "{msg}");
                assert!(msg.contains("start-ledger"), "{msg}");
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn zero_match_or_page_caps_are_errors() {
        assert!(validate_account_transactions_request(None, None, 50, 0, 10).is_err());
        assert!(validate_account_transactions_request(None, None, 50, 20, 0).is_err());
    }

    #[tokio::test]
    async fn start_below_oldest_ledger_errors_naming_bounds() {
        let address_a = address_for(ADDRESS_A_KEY);
        // The endpoint's retention starts at 5000, but the scan asks for 100.
        let page = page_json(vec![], None, 6000, 5000);
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 6000);
        let err = get_account_transactions(
            &SorobanRpcClient::new(&endpoint),
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("100"), "{msg}");
                assert!(msg.contains("5000"), "{msg}");
                assert!(msg.contains("6000"), "{msg}");
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn response_missing_transactions_list_is_an_error() {
        let address_a = address_for(ADDRESS_A_KEY);
        // A response with no transactions member at all is malformed,
        // not an account with no history.
        let page = serde_json::json!({
            "latestLedger": 500,
            "oldestLedger": 1,
            "cursor": null,
        });
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let err = get_account_transactions(
            &SorobanRpcClient::new(&endpoint),
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        // A decode failure, not an empty scan: the response is rejected.
        match err {
            RpcError::Reqwest(e) => {
                assert!(e.to_string().contains("decoding response body"), "{e}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn response_missing_retention_bounds_is_an_error() {
        let address_a = address_for(ADDRESS_A_KEY);
        // The retention bounds are part of the contract too: a page
        // without them cannot be checked against the requested start.
        let page = serde_json::json!({
            "transactions": [],
            "cursor": null,
        });
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let err = get_account_transactions(
            &SorobanRpcClient::new(&endpoint),
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        // Same rejection for a page that omits the retention bounds.
        match err {
            RpcError::Reqwest(e) => {
                assert!(e.to_string().contains("decoding response body"), "{e}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn decode_source_account_handles_all_envelope_kinds() {
        use stellar_xdr::{
            FeeBumpTransaction, FeeBumpTransactionEnvelope, FeeBumpTransactionInnerTx,
            TransactionV0, TransactionV0Envelope,
        };

        // V1
        let v1 = envelope_xdr_for(ADDRESS_A_KEY);
        assert_eq!(
            decode_envelope_source_account(&v1, "hash-v1").unwrap(),
            Some(address_for(ADDRESS_A_KEY))
        );

        // V0
        let v0tx = TransactionV0 {
            source_account_ed25519: Uint256(ADDRESS_B_KEY),
            ..Default::default()
        };
        let v0 = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: v0tx,
            signatures: vec![].try_into().unwrap(),
        });
        let v0_xdr =
            base64::engine::general_purpose::STANDARD.encode(v0.to_xdr(Limits::none()).unwrap());
        assert_eq!(
            decode_envelope_source_account(&v0_xdr, "hash-v0").unwrap(),
            Some(address_for(ADDRESS_B_KEY))
        );

        // Fee bump: the inner transaction's source is matched, not the fee source.
        let inner = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(ADDRESS_A_KEY)),
            ..Default::default()
        };
        let fee_bump = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: FeeBumpTransaction {
                fee_source: MuxedAccount::Ed25519(Uint256(ADDRESS_B_KEY)),
                fee: 200,
                inner_tx: FeeBumpTransactionInnerTx::Tx(TransactionV1Envelope {
                    tx: inner,
                    signatures: vec![].try_into().unwrap(),
                }),
                ..Default::default()
            },
            signatures: vec![].try_into().unwrap(),
        });
        let fb_xdr = base64::engine::general_purpose::STANDARD
            .encode(fee_bump.to_xdr(Limits::none()).unwrap());
        assert_eq!(
            decode_envelope_source_account(&fb_xdr, "hash-fb").unwrap(),
            Some(address_for(ADDRESS_A_KEY))
        );

        // Muxed source: the G... address of the underlying key is
        // matched, not the M... address.
        let muxed = Transaction {
            source_account: MuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
                ed25519: Uint256(ADDRESS_A_KEY),
                id: 7,
            }),
            ..Default::default()
        };
        let muxed_envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx: muxed,
            signatures: vec![].try_into().unwrap(),
        });
        let muxed_xdr = base64::engine::general_purpose::STANDARD
            .encode(muxed_envelope.to_xdr(Limits::none()).unwrap());
        assert_eq!(
            decode_envelope_source_account(&muxed_xdr, "hash-muxed").unwrap(),
            Some(address_for(ADDRESS_A_KEY))
        );

        // Fee bump with a muxed inner source: the inner account's G...
        // key is matched, not the fee source and not the M... address.
        let muxed_inner = Transaction {
            source_account: MuxedAccount::MuxedEd25519(MuxedAccountMed25519 {
                ed25519: Uint256(ADDRESS_A_KEY),
                id: 7,
            }),
            ..Default::default()
        };
        let fee_bump_muxed = TransactionEnvelope::TxFeeBump(FeeBumpTransactionEnvelope {
            tx: FeeBumpTransaction {
                fee_source: MuxedAccount::Ed25519(Uint256(ADDRESS_B_KEY)),
                fee: 200,
                inner_tx: FeeBumpTransactionInnerTx::Tx(TransactionV1Envelope {
                    tx: muxed_inner,
                    signatures: vec![].try_into().unwrap(),
                }),
                ..Default::default()
            },
            signatures: vec![].try_into().unwrap(),
        });
        let fb_muxed_xdr = base64::engine::general_purpose::STANDARD
            .encode(fee_bump_muxed.to_xdr(Limits::none()).unwrap());
        assert_eq!(
            decode_envelope_source_account(&fb_muxed_xdr, "hash-fb-muxed").unwrap(),
            Some(address_for(ADDRESS_A_KEY))
        );

        // Garbage is never a match.
        assert_eq!(
            decode_envelope_source_account("not base64!!!", "hash-bad").unwrap(),
            None
        );
        assert_eq!(
            decode_envelope_source_account("AAAA", "hash-bad").unwrap(),
            None
        );
    }

    fn envelope_xdr_for_deep_args(pubkey: [u8; 32], depth: usize) -> String {
        use stellar_xdr::{
            ContractId, Hash, HostFunction, InvokeContractArgs, InvokeHostFunctionOp, Operation,
            OperationBody, ScAddress, ScSymbol, ScVal, ScVec,
        };

        // One nested contract argument: each ScVal::Vec level costs two
        // XDR depth levels, so 70 nesting is roughly 150 deep.
        let mut arg = ScVal::U32(1);
        for _ in 0..depth {
            arg = ScVal::Vec(Some(ScVec(vec![arg].try_into().unwrap())));
        }
        let op = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(ContractId(Hash([0x33; 32]))),
                    function_name: ScSymbol("call".try_into().unwrap()),
                    args: vec![arg].try_into().unwrap(),
                }),
                auth: vec![].try_into().unwrap(),
            }),
        };
        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(pubkey)),
            fee: 100,
            seq_num: SequenceNumber(1),
            operations: vec![op].try_into().unwrap(),
            ..Default::default()
        };
        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: vec![].try_into().unwrap(),
        });
        base64::engine::general_purpose::STANDARD.encode(envelope.to_xdr(Limits::none()).unwrap())
    }

    /// A deeply nested but valid envelope still decodes and matches:
    /// 70 nested arguments is far past the old depth bound of 64.
    #[tokio::test]
    async fn deeply_nested_envelope_matches() {
        let address_a = address_for(ADDRESS_A_KEY);
        let mut deep_tx = tx_json(100, "hash-deep", ADDRESS_A_KEY, "SUCCESS");
        deep_tx["envelopeXdr"] = serde_json::json!(envelope_xdr_for_deep_args(ADDRESS_A_KEY, 70));
        let page = page_json(vec![deep_tx], None, 500, 1);
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.transactions.len(), 1);
        assert_eq!(scan.transactions[0].tx_hash, "hash-deep");
    }

    /// An envelope past the depth bound fails the scan naming its hash,
    /// instead of silently dropping a real transaction.
    #[tokio::test]
    async fn envelope_past_depth_limit_errors_naming_hash() {
        let address_a = address_for(ADDRESS_A_KEY);
        let mut deep_tx = tx_json(100, "hash-too-deep", ADDRESS_A_KEY, "SUCCESS");
        deep_tx["envelopeXdr"] = serde_json::json!(envelope_xdr_for_deep_args(ADDRESS_A_KEY, 300));
        let page = page_json(vec![deep_tx], None, 500, 1);
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let err = get_account_transactions(
            &SorobanRpcClient::new(&endpoint),
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("hash-too-deep"), "{msg}");
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// A cursor is the decimal SEP-35 total-order id: ledger in the top
    /// 32 bits.
    #[test]
    fn cursor_ledger_reads_top_32_bits() {
        assert_eq!(cursor_ledger("64424509440000"), Some(15000));
        assert_eq!(cursor_ledger("2147483648000"), Some(500));
        assert_eq!(cursor_ledger("0"), Some(0));
        assert_eq!(cursor_ledger("page-2"), None);
        assert_eq!(cursor_ledger(""), None);
        assert_eq!(cursor_ledger("-1"), None);
    }

    /// An empty page whose cursor still points below the latest ledger is
    /// a sparse gap, not the end: the scan continues from the cursor and
    /// finds the match on the later page.
    #[tokio::test]
    async fn sparse_empty_page_continues_from_cursor() {
        let address_a = address_for(ADDRESS_A_KEY);
        let sparse_cursor = ((15000i64) << 32).to_string();
        let page1 = page_json(vec![], Some(&sparse_cursor), 20000, 1);
        let page2 = page_json(
            vec![tx_json(15001, "hash-a1", ADDRESS_A_KEY, "SUCCESS")],
            None,
            20000,
            1,
        );
        let (endpoint, recorded) = spawn_mock_server(vec![page1, page2], 20000);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 1);
        assert_eq!(scan.scanned, 1);
        assert_eq!(scan.pages, 2);
        assert_eq!(scan.transactions[0].tx_hash, "hash-a1");
        assert_eq!(scan.next_cursor, None);
        // The second request resumed from the sparse page's cursor.
        let requests = recorded.lock().unwrap();
        let tx_requests: Vec<_> = requests
            .iter()
            .filter(|r| r["method"].as_str() == Some("getTransactions"))
            .collect();
        assert_eq!(tx_requests.len(), 2);
        assert_eq!(
            tx_requests[1]["params"]["pagination"]["cursor"].as_str(),
            Some(sparse_cursor.as_str())
        );
    }

    /// An empty page whose cursor already points at the latest ledger is
    /// the end of the range: the scan stops there.
    #[tokio::test]
    async fn empty_page_at_latest_ledger_stops() {
        let address_a = address_for(ADDRESS_A_KEY);
        let tip_cursor = ((500i64) << 32).to_string();
        let page = page_json(vec![], Some(&tip_cursor), 500, 1);
        let (endpoint, _recorded) = spawn_mock_server(vec![page], 500);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 0);
        assert_eq!(scan.pages, 1);
        assert_eq!(scan.next_cursor, None);
    }

    /// When the page cap stops a scan on a sparse empty page, the cursor
    /// is kept so the caller can resume past the gap.
    #[tokio::test]
    async fn page_cap_on_sparse_empty_page_keeps_cursor() {
        let address_a = address_for(ADDRESS_A_KEY);
        let sparse_cursor = ((15000i64) << 32).to_string();
        let page1 = page_json(vec![], Some(&sparse_cursor), 20000, 1);
        let (endpoint, _recorded) = spawn_mock_server(vec![page1], 20000);
        let scan = scan_mock(
            &endpoint,
            &address_a,
            AccountTransactionsOptions {
                start_ledger: Some(100),
                max_pages: 1,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(scan.matched, 0);
        assert_eq!(scan.pages, 1);
        assert_eq!(scan.next_cursor, Some(sparse_cursor));
    }
}
