use serde::{Deserialize, Serialize};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Serialize, Deserialize)]
pub struct ContractEvent {
    pub contract_id: String,
    pub ledger: Option<u32>,
    pub topics: Vec<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EventFilter {
    #[serde(rename = "type")]
    pub filter_type: String,
    pub contract_ids: Vec<String>,
}

/// Request body for the Soroban RPC `getEvents` method.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsRequest {
    pub start_ledger: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ledger: Option<u32>,
    pub filters: Vec<EventFilter>,
    /// Maximum number of events to return in this page.
    ///
    /// Omitted from the wire request entirely when `None`, so callers that do
    /// not paginate keep whatever page size the RPC picks by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Continuation token from a previous page's [`EventPage::next_cursor`].
    ///
    /// Omitted when `None`, which makes the request a first-page request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The `pagingTokens` object returned by `getEvents` when the RPC has more
/// events than fit in one response.
///
/// Both tokens identify positions in the RPC's own event ordering; they are
/// echoed back to the RPC as the `cursor` request field to continue reading.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PagingTokens {
    #[serde(default)]
    pub newest: Option<String>,
    #[serde(default)]
    pub oldest: Option<String>,
}

/// A single page of contract events plus the metadata needed to continue.
#[derive(Debug)]
pub struct EventPage {
    /// Events in this page, in the order the RPC returned them.
    pub events: Vec<ContractEvent>,
    /// Token to pass as the next request's `cursor`. `None` means this is the
    /// final page: the RPC reported no further events to read.
    pub next_cursor: Option<String>,
    /// The RPC's latest ledger at the time of the request, when reported.
    pub latest_ledger: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsResponse {
    events: Vec<RpcEvent>,
    /// Present on current RPC versions; `None` when the endpoint omits it.
    #[serde(default)]
    latest_ledger: Option<u32>,
    /// Absent entirely on the final page, and may be present but empty.
    #[serde(default)]
    paging_tokens: Option<PagingTokens>,
}

#[derive(Deserialize)]
#[allow(non_snake_case, dead_code)]
struct RpcEvent {
    // The Soroban RPC `getEvents` response returns `ledger` as an integer, not a
    // string (unlike some other RPC fields). Declared as `u32` to match the wire
    // format; callers in `get_contract_events` map it into `ContractEvent.ledger`
    // (Option<u32>) directly.
    ledger: u32,
    contractId: String,
    topic: Vec<String>,
    // The RPC returns `value` as a base64-encoded XDR *string* (not an object with
    // an `xdr` field), so it maps directly onto `ContractEvent.value: Option<String>`.
    value: String,
}

/// Derive the cursor for the next page from a page's `pagingTokens`.
///
/// The RPC reports two tokens: `newest` for the most recent event in the page
/// and `oldest` for the earliest. The CLI exposes the raw token the caller
/// passed in, and paging with the `newest` token walks *backwards* through
/// ledger order, so `newest` is preferred and `oldest` is the fallback. Tokens
/// that are missing, empty, or whitespace-only mean there is no next page.
fn next_cursor_from(paging_tokens: &Option<PagingTokens>) -> Option<String> {
    let tokens = paging_tokens.as_ref()?;
    for candidate in [tokens.newest.as_deref(), tokens.oldest.as_deref()] {
        if let Some(token) = candidate {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Resolve the start and end ledger query range.
///
/// When `start_ledger` is omitted:
/// - If `end_ledger` is provided, `start_ledger` defaults to `end_ledger.saturating_sub(1000).max(1)`.
/// - If `end_ledger` is also omitted, `start_ledger` defaults to `latest_ledger.saturating_sub(1000).max(1)`.
///
/// Returns an error if `start_ledger > end_ledger`.
pub fn resolve_ledger_range(
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    latest_ledger: u32,
) -> Result<(u32, Option<u32>), RpcError> {
    let start = match start_ledger {
        Some(s) => s,
        None => {
            if let Some(end) = end_ledger {
                end.saturating_sub(1000).max(1)
            } else {
                latest_ledger.saturating_sub(1000).max(1)
            }
        }
    };

    if let Some(end) = end_ledger {
        if start > end {
            return Err(RpcError::Rpc(format!(
                "Invalid ledger range: start ledger ({start}) cannot be greater than end ledger ({end})"
            )));
        }
    }

    Ok((start, end_ledger))
}

/// Fetch one page of contract events, with optional `limit` and `cursor`.
///
/// `limit` caps how many events come back in one response. `cursor` continues a
/// previous page: pass the [`EventPage::next_cursor`] from the previous call and
/// repeat until `next_cursor` is `None`. The ledger range is resolved exactly as
/// [`get_contract_events`] does, so paging does not change which ledgers are
/// considered.
pub async fn get_contract_events_page(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<EventPage, RpcError> {
    // Fast rejection if both start and end are provided and inverted, prior to any RPC call.
    if let (Some(start), Some(end)) = (start_ledger, end_ledger) {
        if start > end {
            return Err(RpcError::Rpc(format!(
                "Invalid ledger range: start ledger ({start}) cannot be greater than end ledger ({end})"
            )));
        }
    }

    // Determine start and end ledger. When both are omitted, query latest ledger
    // and look back up to 1000 ledgers.
    let (start_ledger, end_ledger) = if start_ledger.is_none() && end_ledger.is_none() {
        let latest_ledger_info =
            client
                .get_ledger()
                .await
                .unwrap_or_else(|_| crate::client::LedgerInfo {
                    id: "".to_string(),
                    protocol_version: 0,
                    sequence: 1000,
                });
        resolve_ledger_range(start_ledger, end_ledger, latest_ledger_info.sequence)?
    } else {
        resolve_ledger_range(start_ledger, end_ledger, 0)?
    };

    // The Soroban RPC `getEvents` treats `endLedger` as exclusive. Convert the
    // user-supplied bound to inclusive by sending end + 1.
    let request_end_ledger = match end_ledger {
        Some(end) => Some(end.checked_add(1).ok_or_else(|| {
            RpcError::Rpc(format!(
                "End ledger ({end}) exceeds maximum supported ledger sequence"
            ))
        })?),
        None => None,
    };

    // An empty or whitespace-only cursor is treated as "no cursor" so callers
    // can pass a page's `next_cursor` through without a nil check.
    let cursor = cursor
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);

    let request_body = GetEventsRequest {
        start_ledger,
        end_ledger: request_end_ledger,
        filters: vec![EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec![contract_id.to_string()],
        }],
        limit,
        cursor,
    };

    let result: GetEventsResponse = client.request("getEvents", request_body).await?;

    let mut contract_events = Vec::new();

    for ev in result.events {
        contract_events.push(ContractEvent {
            contract_id: ev.contractId,
            ledger: Some(ev.ledger),
            topics: ev.topic,
            value: Some(ev.value),
        });
    }

    Ok(EventPage {
        events: contract_events,
        next_cursor: next_cursor_from(&result.paging_tokens),
        latest_ledger: result.latest_ledger,
    })
}

/// Fetch contract events in a single request.
///
/// This keeps its original signature and behaviour: it sends no `limit` and no
/// `cursor`, and returns only the events from that one response. Use
/// [`get_contract_events_page`] when you need to page through results.
pub async fn get_contract_events(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
) -> Result<Vec<ContractEvent>, RpcError> {
    let page =
        get_contract_events_page(client, contract_id, start_ledger, end_ledger, None, None).await?;
    Ok(page.events)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT_ID: &str = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW";

    #[test]
    fn test_resolve_ledger_range_defaults() {
        // When both start and end are omitted, look back up to 1000 ledgers from latest
        let (start, end) = resolve_ledger_range(None, None, 2500).unwrap();
        assert_eq!(start, 1500);
        assert_eq!(end, None);

        // When latest ledger is < 1000, start ledger saturates at 1
        let (start, end) = resolve_ledger_range(None, None, 500).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, None);
    }

    #[test]
    fn test_resolve_ledger_range_explicit_range() {
        let (start, end) = resolve_ledger_range(Some(1000), Some(2000), 5000).unwrap();
        assert_eq!(start, 1000);
        assert_eq!(end, Some(2000));

        // Single ledger range is valid
        let (start, end) = resolve_ledger_range(Some(1000), Some(1000), 5000).unwrap();
        assert_eq!(start, 1000);
        assert_eq!(end, Some(1000));
    }

    #[test]
    fn test_resolve_ledger_range_start_only() {
        // start_ledger without end_ledger leaves end_ledger as None (uses latest ledger as upper bound)
        let (start, end) = resolve_ledger_range(Some(1000), None, 5000).unwrap();
        assert_eq!(start, 1000);
        assert_eq!(end, None);
    }

    #[test]
    fn test_resolve_ledger_range_end_only() {
        // end_ledger without start_ledger computes start from end.saturating_sub(1000).max(1)
        let (start, end) = resolve_ledger_range(None, Some(2500), 5000).unwrap();
        assert_eq!(start, 1500);
        assert_eq!(end, Some(2500));

        let (start, end) = resolve_ledger_range(None, Some(500), 5000).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, Some(500));
    }

    #[test]
    fn test_resolve_ledger_range_rejects_inverted() {
        let err = resolve_ledger_range(Some(5000), Some(1000), 10000).unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("start ledger (5000) cannot be greater than end ledger (1000)")
                );
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn test_resolve_ledger_range_rejects_end_zero() {
        let err = resolve_ledger_range(None, Some(0), 10000).unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("start ledger (1) cannot be greater than end ledger (0)"));
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn test_get_events_request_serialization_with_end_ledger() {
        let req = GetEventsRequest {
            start_ledger: 1000,
            end_ledger: Some(2000),
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![CONTRACT_ID.to_string()],
            }],
            limit: None,
            cursor: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["startLedger"], 1000);
        assert_eq!(value["endLedger"], 2000);
        assert_eq!(value["filters"][0]["type"], "contract");
        assert_eq!(value["filters"][0]["contractIds"][0], CONTRACT_ID);
    }

    #[test]
    fn test_get_events_request_serialization_omits_none_end_ledger() {
        let req = GetEventsRequest {
            start_ledger: 1000,
            end_ledger: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![CONTRACT_ID.to_string()],
            }],
            limit: None,
            cursor: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"startLedger\":1000"));
        assert!(!json.contains("endLedger"));

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["startLedger"], 1000);
        assert!(value.get("endLedger").is_none());
    }

    #[test]
    fn test_get_events_request_serialization_omits_unset_pagination() {
        // No limit and no cursor: the wire request must not grow pagination keys,
        // which is what keeps the no-flags path byte-for-byte as it was.
        let req = GetEventsRequest {
            start_ledger: 1500,
            end_ledger: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![CONTRACT_ID.to_string()],
            }],
            limit: None,
            cursor: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("limit"), "unset limit must be omitted");
        assert!(!json.contains("cursor"), "unset cursor must be omitted");

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("limit").is_none());
        assert!(value.get("cursor").is_none());
    }

    #[test]
    fn test_get_events_request_serialization_includes_pagination() {
        let req = GetEventsRequest {
            start_ledger: 1500,
            end_ledger: Some(2000),
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![CONTRACT_ID.to_string()],
            }],
            limit: Some(5),
            cursor: Some("cursor-abc".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        // camelCase keys, matching the rest of the request struct.
        assert_eq!(value["limit"], 5);
        assert_eq!(value["cursor"], "cursor-abc");
        // The ledger range still travels alongside pagination.
        assert_eq!(value["startLedger"], 1500);
        assert_eq!(value["endLedger"], 2000);
    }

    #[test]
    fn test_get_events_response_parses_paging_tokens_and_latest_ledger() {
        let raw = r#"{
            "events": [
                {"ledger": 2000, "contractId": "CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK",
                 "topic": ["AAAAAwAAACo="], "value": "AAAAAwAAACo="}
            ],
            "latestLedger": 2500,
            "pagingTokens": {"newest": "token-newest-1", "oldest": "token-oldest-1"}
        }"#;

        let response: GetEventsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.latest_ledger, Some(2500));
        assert_eq!(
            next_cursor_from(&response.paging_tokens),
            Some("token-newest-1".to_string())
        );
    }

    #[test]
    fn test_get_events_response_without_paging_tokens_is_final_page() {
        let raw = r#"{
            "events": [],
            "latestLedger": 2500
        }"#;

        let response: GetEventsResponse = serde_json::from_str(raw).unwrap();
        assert!(response.events.is_empty());
        assert_eq!(response.latest_ledger, Some(2500));
        assert_eq!(next_cursor_from(&response.paging_tokens), None);
    }

    #[test]
    fn test_get_events_response_with_empty_paging_tokens_is_final_page() {
        // Some endpoints report the object but leave the tokens empty; that must
        // read as "no next page" rather than as a cursor loop.
        let raw = r#"{
            "events": [],
            "latestLedger": 2500,
            "pagingTokens": {"newest": "", "oldest": "   "}
        }"#;

        let response: GetEventsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(next_cursor_from(&response.paging_tokens), None);
    }

    #[test]
    fn test_get_events_response_null_paging_tokens_is_final_page() {
        let raw = r#"{"events": [], "latestLedger": 1, "pagingTokens": null}"#;
        let response: GetEventsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(next_cursor_from(&response.paging_tokens), None);
    }

    #[test]
    fn test_get_events_response_without_latest_ledger() {
        // `latestLedger` is optional so an endpoint that omits it still parses.
        let raw = r#"{"events": []}"#;
        let response: GetEventsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(response.latest_ledger, None);
    }

    #[test]
    fn test_next_cursor_falls_back_to_oldest_token() {
        let tokens = Some(PagingTokens {
            newest: None,
            oldest: Some("token-oldest-only".to_string()),
        });
        assert_eq!(
            next_cursor_from(&tokens),
            Some("token-oldest-only".to_string())
        );

        let tokens = Some(PagingTokens {
            newest: Some("  ".to_string()),
            oldest: Some("token-oldest-only".to_string()),
        });
        assert_eq!(
            next_cursor_from(&tokens),
            Some("token-oldest-only".to_string())
        );
    }

    #[tokio::test]
    async fn test_get_contract_events_rejects_inverted_range_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events(&client, CONTRACT_ID, Some(5000), Some(1000)).await;

        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("start ledger (5000) cannot be greater than end ledger (1000)")
                );
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_events_page_rejects_inverted_range_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result =
            get_contract_events_page(&client, CONTRACT_ID, Some(5000), Some(1000), Some(5), None)
                .await;

        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("start ledger (5000) cannot be greater than end ledger (1000)")
                );
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_events_rejects_u32_max_end_ledger_overflow() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events(&client, CONTRACT_ID, Some(1000), Some(u32::MAX)).await;

        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("exceeds maximum supported ledger sequence"));
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }
}
