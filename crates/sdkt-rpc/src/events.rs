use serde::{Deserialize, Serialize};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractEvent {
    pub contract_id: String,
    pub ledger: Option<u32>,
    pub topics: Vec<String>,
    pub value: Option<String>,
}

/// One RPC topic filter: a list of per-position segment matchers, where each
/// segment is a base64 XDR `ScVal`, [`TOPIC_WILDCARD_ONE`] or (last position
/// only) [`TOPIC_WILDCARD_REST`].
pub type TopicFilter = Vec<String>;

/// Segment matcher that matches exactly one topic of any value.
pub const TOPIC_WILDCARD_ONE: &str = "*";
/// Trailing segment matcher that matches zero or more remaining topics.
pub const TOPIC_WILDCARD_REST: &str = "**";

/// A `getEvents` filter. Build it with [`EventFilter::contract`] (and
/// [`EventFilter::with_topics`]); it is `#[non_exhaustive]` so new RPC filter
/// fields can be added without breaking downstream struct literals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EventFilter {
    #[serde(rename = "type")]
    pub filter_type: String,
    pub contract_ids: Vec<String>,
    /// Topic matchers; an event matches if any one filter matches. Omitted from
    /// the request when `None`, so unfiltered queries are unchanged on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<TopicFilter>>,
}

impl EventFilter {
    /// Filter on contract events emitted by any of `contract_ids`, with no
    /// topic matcher.
    pub fn contract<I, S>(contract_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            filter_type: "contract".to_string(),
            contract_ids: contract_ids.into_iter().map(Into::into).collect(),
            topics: None,
        }
    }

    /// Restrict the filter to events matching any of `topics`.
    pub fn with_topics(mut self, topics: Vec<TopicFilter>) -> Self {
        self.topics = Some(topics);
        self
    }
}

/// Topic filter matching events whose first topic (the event name symbol) is
/// `first_topic_b64`, regardless of how many topics follow it.
pub fn first_topic_filter(first_topic_b64: impl Into<String>) -> TopicFilter {
    vec![first_topic_b64.into(), TOPIC_WILDCARD_REST.to_string()]
}

pub const DEFAULT_PAGE_CAP: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventPagination {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl EventPagination {
    pub fn new(cursor: Option<String>, limit: Option<u32>) -> Self {
        Self { cursor, limit }
    }

    pub fn cursor(cursor: impl Into<String>) -> Self {
        Self {
            cursor: Some(cursor.into()),
            limit: None,
        }
    }

    pub fn limit(limit: u32) -> Self {
        Self {
            cursor: None,
            limit: Some(limit),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ledger: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ledger: Option<u32>,
    pub filters: Vec<EventFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<EventPagination>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsResponse {
    pub events: Vec<RpcEvent>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub latest_ledger: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcEvent {
    pub ledger: u32,
    pub contract_id: String,
    #[serde(default)]
    pub topic: Vec<String>,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaginatedEvents {
    pub events: Vec<ContractEvent>,
    pub cursor: Option<String>,
    pub latest_ledger: Option<u32>,
    pub pages_fetched: usize,
    pub reached_page_cap: bool,
}

/// Validate that cursor pagination is not mixed with startLedger or endLedger.
///
/// Under the Soroban RPC protocol, providing a `cursor` encapsulates the stream
/// continuation point and must omit `startLedger` and `endLedger`. Supplying both
/// is rejected client-side before any RPC dispatch.
pub fn validate_cursor_and_ledger_range(
    cursor: Option<&str>,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
) -> Result<(), RpcError> {
    if let Some(c) = cursor {
        if !c.is_empty() && (start_ledger.is_some() || end_ledger.is_some()) {
            return Err(RpcError::Rpc(
                "Cannot specify cursor together with start ledger or end ledger: cursor pagination continues from an existing stream position and cannot be combined with an explicit ledger range".to_string(),
            ));
        }
    }
    Ok(())
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

pub async fn get_contract_events(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
) -> Result<Vec<ContractEvent>, RpcError> {
    get_contract_events_filtered(client, contract_id, start_ledger, end_ledger, None).await
}

/// Like [`get_contract_events`], but sends `topics` as the server-side topic
/// matcher of the contract filter so only matching events are returned.
pub async fn get_contract_events_filtered(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
) -> Result<Vec<ContractEvent>, RpcError> {
    let paginated = get_contract_events_paginated(
        client,
        contract_id,
        start_ledger,
        end_ledger,
        topics,
        None,
        Some(1),
    )
    .await?;
    Ok(paginated.events)
}

/// Query contract events with cursor-aware pagination and bounded page following.
///
/// Follows cursor across pages until:
/// 1. The page is exhausted (no cursor returned, empty events, or cursor unchanged).
/// 2. The caller-supplied `page_cap` is reached.
/// 3. The caller-supplied `end_ledger` is reached/passed (events beyond `end_ledger` are discarded).
/// 4. The events pass `latestLedger` (an event's ledger exceeds the observed `latestLedger`).
///
/// Client-side validation enforces that requests carrying a `cursor` omit `start_ledger`
/// and `end_ledger` prior to making any network call.
pub async fn get_contract_events_paginated(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
    pagination: Option<EventPagination>,
    page_cap: Option<usize>,
) -> Result<PaginatedEvents, RpcError> {
    let mut current_cursor = pagination
        .as_ref()
        .and_then(|p| p.cursor.clone())
        .filter(|c| !c.is_empty());
    validate_cursor_and_ledger_range(current_cursor.as_deref(), start_ledger, end_ledger)?;

    if let (Some(start), Some(end)) = (start_ledger, end_ledger) {
        if start > end {
            return Err(RpcError::Rpc(format!(
                "Invalid ledger range: start ledger ({start}) cannot be greater than end ledger ({end})"
            )));
        }
    }

    let limit = pagination.as_ref().and_then(|p| p.limit);

    // If cursor is provided, start/end ledger are not resolved or sent.
    let (resolved_start, request_end_ledger) = if current_cursor.is_some() {
        (None, None)
    } else {
        let (start, end) = if start_ledger.is_none() && end_ledger.is_none() {
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

        let req_end = match end {
            Some(e) => Some(e.checked_add(1).ok_or_else(|| {
                RpcError::Rpc(format!(
                    "End ledger ({e}) exceeds maximum supported ledger sequence"
                ))
            })?),
            None => None,
        };

        (Some(start), req_end)
    };

    let mut accumulated_events = Vec::new();
    let mut pages_fetched = 0;
    let mut target_latest_ledger: Option<u32> = None;
    let mut reached_page_cap = false;

    loop {
        if let Some(cap) = page_cap {
            if pages_fetched >= cap {
                reached_page_cap = true;
                break;
            }
        }

        let (req_start, req_end) = if current_cursor.is_some() {
            (None, None)
        } else {
            (resolved_start, request_end_ledger)
        };

        let req_pagination = if current_cursor.is_some() || limit.is_some() {
            Some(EventPagination {
                cursor: current_cursor.clone(),
                limit,
            })
        } else {
            None
        };

        let request_body = GetEventsRequest {
            start_ledger: req_start,
            end_ledger: req_end,
            filters: vec![EventFilter {
                topics: topics.clone(),
                ..EventFilter::contract([contract_id])
            }],
            pagination: req_pagination,
        };

        let result: GetEventsResponse = client.request("getEvents", request_body).await?;
        pages_fetched += 1;

        if target_latest_ledger.is_none() {
            target_latest_ledger = result.latest_ledger;
        }

        let page_empty = result.events.is_empty();
        let mut passed_end_ledger = false;

        for ev in result.events {
            if let Some(max_ledger) = end_ledger {
                if ev.ledger > max_ledger {
                    passed_end_ledger = true;
                    break;
                }
            }
            accumulated_events.push(ContractEvent {
                contract_id: ev.contract_id,
                ledger: Some(ev.ledger),
                topics: ev.topic,
                value: Some(ev.value),
            });
        }

        let next_cursor = result.cursor.filter(|c| !c.is_empty());

        // Termination Condition 1: No further cursor / page exhausted
        if page_empty || next_cursor.is_none() || next_cursor == current_cursor {
            current_cursor = next_cursor;
            break;
        }

        // Termination Condition 2: Caller-supplied page cap reached
        if let Some(cap) = page_cap {
            if pages_fetched >= cap {
                reached_page_cap = true;
                current_cursor = next_cursor;
                break;
            }
        }

        // Termination Condition 3: Passing caller's end_ledger
        if passed_end_ledger {
            current_cursor = next_cursor;
            break;
        }

        // Termination Condition 4: Passing latestLedger
        let passed_latest = if let Some(latest) = target_latest_ledger.or(result.latest_ledger) {
            accumulated_events
                .last()
                .and_then(|ev| ev.ledger)
                .is_some_and(|ledger| ledger > latest)
        } else {
            false
        };

        current_cursor = next_cursor;

        if passed_latest {
            break;
        }
    }

    Ok(PaginatedEvents {
        events: accumulated_events,
        cursor: current_cursor,
        latest_ledger: target_latest_ledger,
        pages_fetched,
        reached_page_cap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            start_ledger: Some(1000),
            end_ledger: Some(2000),
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
            pagination: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["startLedger"], 1000);
        assert_eq!(value["endLedger"], 2000);
        assert_eq!(value["filters"][0]["type"], "contract");
        assert_eq!(
            value["filters"][0]["contractIds"][0],
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW"
        );
        assert!(value.get("pagination").is_none());
    }

    #[test]
    fn test_get_events_request_serialization_omits_none_end_ledger() {
        let req = GetEventsRequest {
            start_ledger: Some(1000),
            end_ledger: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
            pagination: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"startLedger\":1000"));
        assert!(!json.contains("endLedger"));
        assert!(!json.contains("pagination"));

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["startLedger"], 1000);
        assert!(value.get("endLedger").is_none());
        assert!(value.get("pagination").is_none());
    }

    #[test]
    fn test_get_events_request_serialization_with_pagination() {
        let req = GetEventsRequest {
            start_ledger: None,
            end_ledger: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
            pagination: Some(EventPagination {
                cursor: Some("0000001000-0000000001".to_string()),
                limit: Some(50),
            }),
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("startLedger"));
        assert!(!json.contains("endLedger"));

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["pagination"]["cursor"], "0000001000-0000000001");
        assert_eq!(value["pagination"]["limit"], 50);
        assert!(value.get("startLedger").is_none());
        assert!(value.get("endLedger").is_none());
    }

    #[test]
    fn test_get_events_request_without_pagination_is_byte_identical() {
        let req = GetEventsRequest {
            start_ledger: Some(1000),
            end_ledger: Some(2000),
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
            pagination: None,
        };
        let expected = r#"{"startLedger":1000,"endLedger":2000,"filters":[{"type":"contract","contractIds":["CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW"]}]}"#;
        assert_eq!(serde_json::to_string(&req).unwrap(), expected);
    }

    #[test]
    fn test_get_events_response_deserialization_with_cursor_and_latest_ledger() {
        let json = r#"{
            "events": [
                {
                    "ledger": 1000,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAACo="],
                    "value": "AAAAAwAAACo="
                }
            ],
            "cursor": "0000001000-0000000001",
            "latestLedger": 2500
        }"#;

        let resp: GetEventsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.events.len(), 1);
        assert_eq!(resp.cursor.as_deref(), Some("0000001000-0000000001"));
        assert_eq!(resp.latest_ledger, Some(2500));
        assert_eq!(resp.events[0].ledger, 1000);
        assert_eq!(
            resp.events[0].contract_id,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW"
        );
    }

    #[test]
    fn test_get_events_response_deserialization_omits_optional_cursor_and_latest_ledger() {
        let json = r#"{"events":[]}"#;

        let resp: GetEventsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.events.is_empty());
        assert_eq!(resp.cursor, None);
        assert_eq!(resp.latest_ledger, None);
    }

    #[test]
    fn test_validate_cursor_and_ledger_range() {
        assert!(validate_cursor_and_ledger_range(None, Some(1000), Some(2000)).is_ok());
        assert!(validate_cursor_and_ledger_range(None, None, None).is_ok());
        assert!(
            validate_cursor_and_ledger_range(Some("0000001000-0000000001"), None, None).is_ok()
        );

        let err1 =
            validate_cursor_and_ledger_range(Some("0000001000-0000000001"), Some(1000), None)
                .unwrap_err();
        match err1 {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("Cannot specify cursor together with start ledger or end ledger")
                );
            }
            other => panic!("Unexpected error: {other:?}"),
        }

        let err2 =
            validate_cursor_and_ledger_range(Some("0000001000-0000000001"), None, Some(2000))
                .unwrap_err();
        match err2 {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("Cannot specify cursor together with start ledger or end ledger")
                );
            }
            other => panic!("Unexpected error: {other:?}"),
        }

        let err3 =
            validate_cursor_and_ledger_range(Some("0000001000-0000000001"), Some(1000), Some(2000))
                .unwrap_err();
        match err3 {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("Cannot specify cursor together with start ledger or end ledger")
                );
            }
            other => panic!("Unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_event_filter_omits_topics_when_absent() {
        let filter = EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec![
                "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
            ],
            topics: None,
        };
        // Byte-identical to the pre-`topics` wire format.
        assert_eq!(
            serde_json::to_string(&filter).unwrap(),
            r#"{"type":"contract","contractIds":["CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW"]}"#
        );
    }

    #[test]
    fn test_event_filter_serializes_topics_when_present() {
        let filter = EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec![
                "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
            ],
            topics: Some(vec![first_topic_filter("AAAADwAAAAhUcmFuc2Zlcg==")]),
        };
        let value = serde_json::to_value(&filter).unwrap();
        assert_eq!(
            value["topics"],
            serde_json::json!([["AAAADwAAAAhUcmFuc2Zlcg==", "**"]])
        );
        assert_eq!(value["type"], "contract");

        // Round-trips, and a filter without `topics` deserializes to `None`.
        let back: EventFilter = serde_json::from_value(value).unwrap();
        assert_eq!(back, filter);
        let legacy: EventFilter =
            serde_json::from_str(r#"{"type":"contract","contractIds":[]}"#).unwrap();
        assert_eq!(legacy.topics, None);
    }

    #[test]
    fn test_event_filter_constructors() {
        let id = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW";
        let plain = EventFilter::contract([id]);
        assert_eq!(plain.filter_type, "contract");
        assert_eq!(plain.contract_ids, vec![id.to_string()]);
        assert_eq!(plain.topics, None);

        let filtered = EventFilter::contract([id]).with_topics(vec![first_topic_filter("AAAA")]);
        assert_eq!(
            filtered.topics,
            Some(vec![vec!["AAAA".to_string(), "**".to_string()]])
        );
    }

    #[test]
    fn test_first_topic_filter_matches_any_trailing_topics() {
        assert_eq!(
            first_topic_filter("AAAA"),
            vec!["AAAA".to_string(), TOPIC_WILDCARD_REST.to_string()]
        );
    }

    #[tokio::test]
    async fn test_get_contract_events_rejects_inverted_range_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(5000),
            Some(1000),
        )
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
        let result = get_contract_events(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            Some(u32::MAX),
        )
        .await;

        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(msg.contains("exceeds maximum supported ledger sequence"));
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_rejects_cursor_with_ledger_range_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            None,
            None,
            Some(EventPagination::cursor("0000001000-0000000001")),
            None,
        )
        .await;

        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => {
                assert!(
                    msg.contains("Cannot specify cursor together with start ledger or end ledger")
                );
            }
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    async fn setup_mock_events_server(
        responses: Vec<serde_json::Value>,
    ) -> (String, tokio::sync::mpsc::Receiver<serde_json::Value>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            let mut resp_iter = responses.into_iter();
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&data);
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let header_part = &text[..pos];
                        let len = header_part
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                if k.trim().eq_ignore_ascii_case("content-length") {
                                    v.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        if data.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                let body_str = String::from_utf8_lossy(&data);
                let body = body_str
                    .split_once("\r\n\r\n")
                    .map(|(_, b)| b)
                    .unwrap_or("");
                let req_json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
                let _ = tx.send(req_json).await;

                if let Some(next_resp) = resp_iter.next() {
                    let resp_body = serde_json::to_string(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": next_resp
                    }))
                    .unwrap();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            }
        });

        (format!("http://{}", addr), rx)
    }

    #[tokio::test]
    async fn test_follow_loop_terminates_on_no_further_cursor() {
        let (server_url, mut rx) = setup_mock_events_server(vec![
            serde_json::json!({
                "events": [
                    {
                        "ledger": 1000,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAACo="],
                        "value": "AAAAAwAAACo="
                    }
                ],
                "cursor": "0000001000-0000000001",
                "latestLedger": 2000
            }),
            serde_json::json!({
                "events": [
                    {
                        "ledger": 1001,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAAAc="],
                        "value": "AAAAAwAAAAc="
                    }
                ],
                "cursor": null,
                "latestLedger": 2000
            }),
        ])
        .await;

        let client = SorobanRpcClient::new(&server_url);
        let res = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            Some(1005),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(res.events.len(), 2);
        assert_eq!(res.pages_fetched, 2);
        assert_eq!(res.cursor, None);
        assert!(!res.reached_page_cap);

        // Check request 1 had startLedger and endLedger
        let req1 = rx.recv().await.unwrap();
        assert_eq!(req1["params"]["startLedger"], 1000);
        assert_eq!(req1["params"]["endLedger"], 1006);
        assert!(req1["params"].get("pagination").is_none());

        // Check request 2 had cursor and omitted startLedger/endLedger
        let req2 = rx.recv().await.unwrap();
        assert!(req2["params"].get("startLedger").is_none());
        assert!(req2["params"].get("endLedger").is_none());
        assert_eq!(
            req2["params"]["pagination"]["cursor"],
            "0000001000-0000000001"
        );
    }

    #[tokio::test]
    async fn test_follow_loop_terminates_on_page_cap() {
        let (server_url, _rx) = setup_mock_events_server(vec![
            serde_json::json!({
                "events": [{
                    "ledger": 1000,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAACo="],
                    "value": "AAAAAwAAACo="
                }],
                "cursor": "c1",
                "latestLedger": 2000
            }),
            serde_json::json!({
                "events": [{
                    "ledger": 1001,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAAAc="],
                    "value": "AAAAAwAAAAc="
                }],
                "cursor": "c2",
                "latestLedger": 2000
            }),
            serde_json::json!({
                "events": [{
                    "ledger": 1002,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAAEg="],
                    "value": "AAAAAwAAAEg="
                }],
                "cursor": "c3",
                "latestLedger": 2000
            }),
        ])
        .await;

        let client = SorobanRpcClient::new(&server_url);
        let res = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            Some(1005),
            None,
            None,
            Some(2),
        )
        .await
        .unwrap();

        assert_eq!(res.events.len(), 2);
        assert_eq!(res.pages_fetched, 2);
        assert_eq!(res.cursor.as_deref(), Some("c2"));
        assert!(res.reached_page_cap);
    }

    #[tokio::test]
    async fn test_follow_loop_terminates_on_passing_latest_ledger() {
        let (server_url, _rx) = setup_mock_events_server(vec![
            serde_json::json!({
                "events": [{
                    "ledger": 100,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAACo="],
                    "value": "AAAAAwAAACo="
                }],
                "cursor": "c1",
                "latestLedger": 100
            }),
            serde_json::json!({
                "events": [{
                    "ledger": 101,
                    "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                    "topic": ["AAAAAwAAAAc="],
                    "value": "AAAAAwAAAAc="
                }],
                "cursor": "c2",
                "latestLedger": 100
            }),
        ])
        .await;

        let client = SorobanRpcClient::new(&server_url);
        let res = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(90),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Page 2 has event with ledger 101 > latestLedger (100), terminating the loop
        assert_eq!(res.events.len(), 2);
        assert_eq!(res.pages_fetched, 2);
        assert_eq!(res.cursor.as_deref(), Some("c2"));
        assert!(!res.reached_page_cap);
    }

    #[tokio::test]
    async fn test_empty_cursor_normalized_with_start_ledger() {
        let (server_url, mut rx) = setup_mock_events_server(vec![serde_json::json!({
            "events": [{
                "ledger": 1000,
                "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                "topic": ["AAAAAwAAACo="],
                "value": "AAAAAwAAACo="
            }],
            "cursor": null,
            "latestLedger": 2000
        })])
        .await;

        let client = SorobanRpcClient::new(&server_url);
        let res = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            Some(1005),
            None,
            Some(EventPagination::cursor("")),
            Some(1),
        )
        .await
        .unwrap();

        assert_eq!(res.events.len(), 1);
        let req = rx.recv().await.unwrap();
        // startLedger must be preserved and not dropped
        assert_eq!(req["params"]["startLedger"], 1000);
        assert_eq!(req["params"]["endLedger"], 1006);
        // pagination with empty cursor must be omitted
        assert!(req["params"].get("pagination").is_none());
    }

    #[tokio::test]
    async fn test_follow_loop_enforces_end_ledger_on_subsequent_pages() {
        let (server_url, mut rx) = setup_mock_events_server(vec![
            serde_json::json!({
                "events": [
                    {
                        "ledger": 1000,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAACo="],
                        "value": "AAAAAwAAACo="
                    },
                    {
                        "ledger": 1001,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAAAc="],
                        "value": "AAAAAwAAAAc="
                    }
                ],
                "cursor": "0000001001-0000000001",
                "latestLedger": 2000
            }),
            serde_json::json!({
                "events": [
                    {
                        "ledger": 1002,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAAEg="],
                        "value": "AAAAAwAAAEg="
                    },
                    {
                        "ledger": 1003,
                        "contractId": "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
                        "topic": ["AAAAAwAAAEk="],
                        "value": "AAAAAwAAAEk="
                    }
                ],
                "cursor": "0000001003-0000000001",
                "latestLedger": 2000
            }),
        ])
        .await;

        let client = SorobanRpcClient::new(&server_url);
        // Caller specifies end_ledger: 1002
        let res = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(1000),
            Some(1002),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Must include ledgers 1000, 1001, 1002, but discard ledger 1003
        assert_eq!(res.events.len(), 3);
        assert_eq!(res.events[0].ledger, Some(1000));
        assert_eq!(res.events[1].ledger, Some(1001));
        assert_eq!(res.events[2].ledger, Some(1002));
        assert_eq!(res.pages_fetched, 2);

        // Verify request 1 sent startLedger 1000 and endLedger 1003 (1002 + 1)
        let req1 = rx.recv().await.unwrap();
        assert_eq!(req1["params"]["startLedger"], 1000);
        assert_eq!(req1["params"]["endLedger"], 1003);

        // Verify request 2 resumed with cursor and omitted startLedger and endLedger per RPC spec
        let req2 = rx.recv().await.unwrap();
        assert!(req2["params"].get("startLedger").is_none());
        assert!(req2["params"].get("endLedger").is_none());
        assert_eq!(
            req2["params"]["pagination"]["cursor"],
            "0000001001-0000000001"
        );
    }
}
