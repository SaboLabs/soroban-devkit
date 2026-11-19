use serde::{Deserialize, Serialize};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// `getEvents` pagination parameters, sent as the `pagination` object of the
/// request (`{"cursor": ..., "limit": ...}`). Both fields are optional and
/// omitted from the wire format when unset, so a request without pagination
/// is byte-identical to the pre-pagination format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsPagination {
    /// Opaque continuation token returned by a previous `getEvents` response.
    /// A request carrying a `cursor` must omit `startLedger`/`endLedger`
    /// (see [`validate_pagination_request`]); the RPC resumes from the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Maximum events per page. Omitted when unset; the RPC applies its own
    /// default cap (100) in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl GetEventsPagination {
    /// Pagination resuming from `cursor` (no explicit page size).
    pub fn with_cursor(cursor: impl Into<String>) -> Self {
        Self {
            cursor: Some(cursor.into()),
            limit: None,
        }
    }

    /// Pagination requesting at most `limit` events per page.
    pub fn with_limit(limit: u32) -> Self {
        Self {
            cursor: None,
            limit: Some(limit),
        }
    }
}

/// Default page cap for `--all-pages` / [`PaginationOptions::default`]: the
/// follow loop never issues more than this many `getEvents` requests unless
/// the caller supplies a larger explicit cap.
pub const DEFAULT_ALL_PAGES_MAX: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsRequest {
    /// First ledger of the query range. `None` (and omitted on the wire) when
    /// the request resumes from a pagination cursor instead of a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ledger: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ledger: Option<u32>,
    pub filters: Vec<EventFilter>,
    /// Pagination parameters; omitted from the wire format when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<GetEventsPagination>,
}

#[derive(Deserialize)]
struct GetEventsResponse {
    events: Vec<RpcEvent>,
    /// Continuation token for the next page; absent when this was the last page.
    #[serde(default)]
    cursor: Option<String>,
    /// Latest ledger known to the RPC at query time.
    #[serde(default, rename = "latestLedger")]
    latest_ledger: Option<u32>,
}

/// Result of a (possibly multi-page) `getEvents` query.
#[derive(Debug, Clone)]
pub struct PaginatedEvents {
    /// Events accumulated across all fetched pages, in response order.
    pub events: Vec<ContractEvent>,
    /// Cursor for the next page, if the RPC returned one and fetching stopped
    /// before the pages were exhausted (page cap reached or `latestLedger`
    /// passed). `None` when the pages are exhausted.
    pub next_cursor: Option<String>,
    /// Latest ledger reported by the RPC on the last fetched page.
    pub latest_ledger: Option<u32>,
    /// Number of `getEvents` requests issued.
    pub pages_fetched: usize,
}

/// Options for [`get_contract_events_paginated`].
#[derive(Debug, Clone)]
pub struct PaginationOptions {
    /// Cursor to resume from; `None` starts from the ledger range.
    pub cursor: Option<String>,
    /// Maximum events per page (`None` leaves the RPC default in place).
    pub limit: Option<u32>,
    /// Maximum number of pages to fetch. The loop always stops at this cap,
    /// so a pathological contract cannot produce an unbounded request loop.
    pub max_pages: usize,
}

impl Default for PaginationOptions {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: None,
            max_pages: DEFAULT_ALL_PAGES_MAX,
        }
    }
}

/// Validate a cursor/limit combination before any RPC call.
///
/// A request carrying a pagination `cursor` must omit `startLedger` and
/// `endLedger`: the cursor already positions the query, and the Soroban RPC
/// requires a cursor-only request for resumption. A `limit` of 0 is rejected
/// the same way.
pub fn validate_pagination_request(
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    pagination: Option<&GetEventsPagination>,
) -> Result<(), RpcError> {
    if let Some(pagination) = pagination {
        if let Some(cursor) = &pagination.cursor {
            if start_ledger.is_some() || end_ledger.is_some() {
                return Err(RpcError::Rpc(format!(
                    "getEvents pagination cursor ({cursor}) cannot be combined with a ledger range: \
                     a request carrying --cursor must omit --start-ledger and --end-ledger; \
                     resume with the cursor alone"
                )));
            }
        }
        if let Some(limit) = pagination.limit {
            if limit == 0 {
                return Err(RpcError::Rpc(
                    "getEvents pagination limit must be at least 1".to_string(),
                ));
            }
        }
    }
    Ok(())
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

/// One raw `getEvents` page: the mapped events plus the RPC's continuation
/// token and latest ledger (both `None` when the RPC omits them).
struct EventsPage {
    events: Vec<ContractEvent>,
    next_cursor: Option<String>,
    latest_ledger: Option<u32>,
}

async fn fetch_events_page(
    client: &SorobanRpcClient,
    request: GetEventsRequest,
) -> Result<EventsPage, RpcError> {
    let result: GetEventsResponse = client.request("getEvents", request).await?;

    let mut contract_events = Vec::with_capacity(result.events.len());
    for ev in result.events {
        contract_events.push(ContractEvent {
            contract_id: ev.contractId,
            ledger: Some(ev.ledger),
            topics: ev.topic,
            value: Some(ev.value),
        });
    }

    Ok(EventsPage {
        events: contract_events,
        // An empty cursor carries no continuation; treat it as exhausted.
        next_cursor: result.cursor.filter(|cursor| !cursor.is_empty()),
        latest_ledger: result.latest_ledger,
    })
}

/// Build the wire request for one page. `range` is the (already resolved)
/// ledger range, or `None` when resuming from a cursor. The Soroban RPC
/// treats `endLedger` as exclusive, so the user-supplied inclusive end bound
/// is converted by sending end + 1.
fn build_events_request(
    contract_id: &str,
    range: Option<(u32, Option<u32>)>,
    topics: Option<Vec<TopicFilter>>,
    pagination: Option<GetEventsPagination>,
) -> Result<GetEventsRequest, RpcError> {
    let (start_ledger, end_ledger) = match range {
        Some((start, end)) => (
            Some(start),
            match end {
                Some(end) => Some(end.checked_add(1).ok_or_else(|| {
                    RpcError::Rpc(format!(
                        "End ledger ({end}) exceeds maximum supported ledger sequence"
                    ))
                })?),
                None => None,
            },
        ),
        None => (None, None),
    };

    Ok(GetEventsRequest {
        start_ledger,
        end_ledger,
        filters: vec![EventFilter {
            topics,
            ..EventFilter::contract([contract_id])
        }],
        pagination,
    })
}

/// Resolve the ledger range for a fresh (non-resumed) query, mirroring the
/// historical single-request behaviour: an inverted explicit range is
/// rejected before any RPC call, and when both bounds are omitted the latest
/// ledger is fetched and the query looks back up to 1000 ledgers.
async fn resolve_events_range(
    client: &SorobanRpcClient,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
) -> Result<(u32, Option<u32>), RpcError> {
    // Fast rejection if both start and end are provided and inverted, prior to any RPC call.
    if let (Some(start), Some(end)) = (start_ledger, end_ledger) {
        if start > end {
            return Err(RpcError::Rpc(format!(
                "Invalid ledger range: start ledger ({start}) cannot be greater than end ledger ({end})"
            )));
        }
    }

    if start_ledger.is_none() && end_ledger.is_none() {
        let latest_ledger_info =
            client
                .get_ledger()
                .await
                .unwrap_or_else(|_| crate::client::LedgerInfo {
                    id: "".to_string(),
                    protocol_version: 0,
                    sequence: 1000,
                });
        resolve_ledger_range(start_ledger, end_ledger, latest_ledger_info.sequence)
    } else {
        resolve_ledger_range(start_ledger, end_ledger, 0)
    }
}

/// Fetch a single page for a fresh query (no cursor) or a resumed query.
/// When `pagination` carries a cursor, `start_ledger`/`end_ledger` must be
/// `None` and no ledger-range resolution happens; otherwise the range is
/// resolved exactly as the historical single-request path did. The
/// cursor/range combination is validated before any RPC call.
async fn fetch_events_page_for_query(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
    pagination: Option<GetEventsPagination>,
) -> Result<EventsPage, RpcError> {
    validate_pagination_request(start_ledger, end_ledger, pagination.as_ref())?;

    let range = if pagination
        .as_ref()
        .and_then(|pagination| pagination.cursor.as_ref())
        .is_some()
    {
        None
    } else {
        Some(resolve_events_range(client, start_ledger, end_ledger).await?)
    };

    let request = build_events_request(contract_id, range, topics, pagination)?;
    fetch_events_page(client, request).await
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
///
/// This is the historical single-request path: it issues exactly one
/// `getEvents` request and returns whatever came back. It does not follow a
/// continuation cursor; use [`get_contract_events_page`] when the caller
/// needs to know whether more pages exist, or
/// [`get_contract_events_paginated`] to follow the cursor.
pub async fn get_contract_events_filtered(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
) -> Result<Vec<ContractEvent>, RpcError> {
    let page =
        fetch_events_page_for_query(client, contract_id, start_ledger, end_ledger, topics, None)
            .await?;
    Ok(page.events)
}

/// Fetch a single `getEvents` page and report whether more pages exist.
///
/// This is [`get_contract_events_filtered`] plus the continuation token: the
/// wire request is identical when `pagination` is `None`, and the returned
/// [`PaginatedEvents::next_cursor`] is `Some` exactly when the RPC returned a
/// cursor, i.e. the page stopped at a page boundary and more events remain.
/// Pass `pagination` to resume from a cursor (`--cursor`) or to cap the page
/// size (`--limit`); a cursor must not be combined with a ledger range.
pub async fn get_contract_events_page(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
    pagination: Option<GetEventsPagination>,
) -> Result<PaginatedEvents, RpcError> {
    let page = fetch_events_page_for_query(
        client,
        contract_id,
        start_ledger,
        end_ledger,
        topics,
        pagination,
    )
    .await?;
    Ok(PaginatedEvents {
        events: page.events,
        next_cursor: page.next_cursor,
        latest_ledger: page.latest_ledger,
        pages_fetched: 1,
    })
}

/// Follow the `getEvents` cursor until the pages are exhausted.
///
/// The loop terminates on the first of:
/// - no further cursor (the RPC returned no `cursor`): the pages are exhausted;
/// - an empty page: the RPC scanned the rest of the window and found
///   nothing, so the query boundary is exhausted even when the RPC still
///   returns a (nonempty) cursor;
/// - an event past the caller-supplied inclusive end ledger: the bound is
///   enforced client-side because follow-up pages are cursor-only and the
///   RPC no longer applies it — the offending event is dropped and the
///   pagination is complete within the bound (`next_cursor` is `None`);
/// - the caller-supplied page cap (`options.max_pages`) is reached — the
///   returned [`PaginatedEvents::next_cursor`] still holds the cursor so the
///   caller can resume later;
/// - an event whose ledger passes the RPC's `latestLedger`: the page set is
///   inconsistent with the ledger the RPC reported, so fetching stops instead
///   of following the cursor into unbounded pages (the cursor is still
///   returned so the caller can resume if it chooses).
///
/// A response cursor that does not advance past the request cursor is a
/// replayed page, not a new one: after the exhaustion checks above, the
/// loop returns an error instead of appending the replayed page.
///
/// The first page starts from the ledger range (`start_ledger`/`end_ledger`,
/// resolved as in [`get_contract_events_filtered`]) unless
/// `options.cursor` is set, in which case the request is cursor-only and no
/// ledger range is sent. Every later page is cursor-only, and ledger bounds
/// are never sent on a cursor request. Returns the accumulated events plus
/// the final cursor (see `PaginatedEvents`).
pub async fn get_contract_events_paginated(
    client: &SorobanRpcClient,
    contract_id: &str,
    start_ledger: Option<u32>,
    end_ledger: Option<u32>,
    topics: Option<Vec<TopicFilter>>,
    options: PaginationOptions,
) -> Result<PaginatedEvents, RpcError> {
    if options.max_pages == 0 {
        return Err(RpcError::Rpc(
            "getEvents page cap (--all-pages) must be at least 1".to_string(),
        ));
    }

    // Client-side end bound: the user-supplied inclusive end ledger.
    // Follow-up pages are cursor-only, so the RPC no longer applies the
    // bound — enforce it here. `None` means the query has no explicit end
    // and is bounded by the RPC's latest ledger instead (see below).
    let inclusive_end = end_ledger;

    let mut events = Vec::new();
    let mut next_cursor = options.cursor.clone();
    let mut latest_ledger = None;
    let mut pages_fetched = 0;

    for _ in 0..options.max_pages {
        let request_cursor = next_cursor.clone();
        // Only send a `pagination` object when a cursor or a limit was
        // actually requested; otherwise the request stays byte-identical
        // to the historical single-request wire format.
        let pagination = match (&next_cursor, options.limit) {
            (None, None) => None,
            (cursor, limit) => Some(GetEventsPagination {
                cursor: cursor.clone(),
                limit,
            }),
        };
        // Only the first page may carry the ledger range; every later page
        // resumes from the cursor alone.
        let (range_start, range_end) = if pages_fetched == 0 {
            (start_ledger, end_ledger)
        } else {
            (None, None)
        };
        let page = fetch_events_page_for_query(
            client,
            contract_id,
            range_start,
            range_end,
            topics.clone(),
            pagination,
        )
        .await?;
        pages_fetched += 1;
        latest_ledger = page.latest_ledger;

        // Drop events past the inclusive end bound. Ledgers arrive in
        // order, so once such an event appears the scan has passed the
        // bound and no later page can contribute an in-range event.
        let mut page_events = page.events;
        let mut passed_end_bound = false;
        if let Some(end) = inclusive_end {
            let kept = page_events.len();
            page_events.retain(|event| event.ledger.is_none_or(|ledger| ledger <= end));
            passed_end_bound = page_events.len() < kept;
        }

        // Termination: an event past the RPC's latest ledger means the page
        // set is inconsistent; stop instead of following the cursor further.
        let passed_latest = match latest_ledger {
            Some(latest) => page_events
                .iter()
                .any(|event| event.ledger.is_some_and(|ledger| ledger > latest)),
            None => false,
        };

        // An empty page means the RPC scanned the rest of the window and
        // found nothing: the query boundary is exhausted even when the RPC
        // still returns a (nonempty) cursor.
        let empty_page = page_events.is_empty();

        let cursor_after = page.next_cursor.clone();

        // Exhaustion takes precedence over the replay check below: a
        // terminal page stops cleanly, it is not an error.
        let exhausted = cursor_after.is_none() || passed_end_bound || empty_page;
        if !exhausted && !passed_latest {
            if let (Some(requested), Some(returned)) = (&request_cursor, &cursor_after) {
                if requested == returned {
                    return Err(RpcError::Rpc(format!(
                        "getEvents pagination cursor did not advance (cursor {returned} \
                         returned again); refusing to replay the same page"
                    )));
                }
            }
        }

        events.extend(page_events);
        if exhausted {
            next_cursor = None;
            break;
        }
        next_cursor = cursor_after;
        if passed_latest {
            break;
        }
    }

    Ok(PaginatedEvents {
        events,
        next_cursor,
        latest_ledger,
        pages_fetched,
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
            pagination: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
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
    }

    #[test]
    fn test_get_events_request_serialization_omits_none_end_ledger() {
        let req = GetEventsRequest {
            start_ledger: Some(1000),
            end_ledger: None,
            pagination: None,
            filters: vec![EventFilter {
                filter_type: "contract".to_string(),
                contract_ids: vec![
                    "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW".to_string()
                ],
                topics: None,
            }],
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"startLedger\":1000"));
        assert!(!json.contains("endLedger"));

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["startLedger"], 1000);
        assert!(value.get("endLedger").is_none());
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

    #[test]
    fn test_get_events_request_serialization_omits_pagination_when_absent() {
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
        // Byte-identical to the pre-pagination wire format.
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"startLedger":1000,"endLedger":2000,"filters":[{"type":"contract","contractIds":["CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW"]}]}"#
        );
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
            pagination: Some(GetEventsPagination {
                cursor: Some("abc123".to_string()),
                limit: Some(25),
            }),
        };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(
            value["pagination"],
            serde_json::json!({"cursor": "abc123", "limit": 25})
        );
        // A cursor-only request omits the ledger range on the wire.
        assert!(value.get("startLedger").is_none());
        assert!(value.get("endLedger").is_none());
    }

    #[test]
    fn test_get_events_response_deserializes_cursor_and_latest_ledger() {
        let response: GetEventsResponse = serde_json::from_str(
            r#"{"events":[{"ledger":100,"contractId":"C","topic":[],"value":"AAAA"}],"cursor":"next","latestLedger":500}"#,
        )
        .unwrap();
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.cursor.as_deref(), Some("next"));
        assert_eq!(response.latest_ledger, Some(500));

        // A legacy response without cursor/latestLedger still parses.
        let legacy: GetEventsResponse = serde_json::from_str(r#"{"events":[]}"#).unwrap();
        assert_eq!(legacy.cursor, None);
        assert_eq!(legacy.latest_ledger, None);
    }

    #[test]
    fn test_validate_pagination_request_rejects_cursor_with_ledger_range() {
        let pagination = GetEventsPagination::with_cursor("abc");
        for (start, end) in [(Some(100), None), (None, Some(200))] {
            let err = validate_pagination_request(start, end, Some(&pagination)).unwrap_err();
            match err {
                RpcError::Rpc(msg) => assert!(msg.contains("cannot be combined"), "{msg}"),
                other => panic!("Unexpected error variant: {other:?}"),
            }
        }

        // A cursor alone, and no pagination at all, are fine.
        assert!(validate_pagination_request(None, None, Some(&pagination)).is_ok());
        assert!(validate_pagination_request(Some(100), Some(200), None).is_ok());
    }

    #[test]
    fn test_validate_pagination_request_rejects_zero_limit() {
        let pagination = GetEventsPagination::with_limit(0);
        let err = validate_pagination_request(None, None, Some(&pagination)).unwrap_err();
        match err {
            RpcError::Rpc(msg) => assert!(msg.contains("at least 1"), "{msg}"),
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    /// Minimal async JSON-RPC mock for the paging tests. `getLatestLedger`
    /// reports sequence 500. `getEvents` answers by the `pagination.cursor`
    /// in the request:
    /// - no cursor: 1 event (ledger 100), cursor "page-2", latestLedger 500
    /// - "page-2": 1 event (ledger 200), cursor "page-3", latestLedger 500
    /// - "page-3": 1 event (ledger 201), no cursor, latestLedger 500
    /// - "loop": 1 event (ledger 100), cursor "loop", latestLedger 500
    /// - "beyond": 1 event (ledger 999, past latestLedger 500), cursor "page-2"
    /// - "cap-N": 1 event (ledger 100), cursor "cap-{N+1}" (always advances)
    /// - "empty-1": no events, cursor "empty-2" (exhausted window)
    async fn spawn_mock_paged_events_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        const CONTRACT: &str = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW";

        fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let mut header_end = None;
                    let mut content_length = 0usize;
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if header_end.is_none() {
                            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
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
                    let Some(end) = header_end else { return };
                    let body = String::from_utf8_lossy(&buf[end..]).to_string();
                    let request: serde_json::Value =
                        serde_json::from_str(&body).unwrap_or_default();
                    let method = request["method"].as_str().unwrap_or("");
                    let resp_body = if method == "getLatestLedger" {
                        r#"{"jsonrpc":"2.0","id":1,"result":{"id":"mock","protocolVersion":22,"sequence":500}}"#
                            .to_string()
                    } else if method == "getEvents" {
                        let cursor = request["params"]["pagination"]["cursor"]
                            .as_str()
                            .unwrap_or("");
                        let event_json = |ledger: u32| {
                            format!(
                                r#"{{"ledger":{ledger},"contractId":"{CONTRACT}","topic":[],"value":"AAAA"}}"#
                            )
                        };
                        // Each arm returns (events JSON fragment, cursor JSON).
                        let (events_json, cursor_json) = match cursor {
                            "page-2" => (event_json(200), r#""page-3""#.to_string()),
                            "page-3" => (event_json(201), "null".to_string()),
                            "loop" => (event_json(100), r#""loop""#.to_string()),
                            "beyond" => (event_json(999), r#""page-2""#.to_string()),
                            "empty-1" => (String::new(), r#""empty-2""#.to_string()),
                            _ if cursor.starts_with("cap-") => {
                                let n: u32 = cursor[4..].parse().unwrap_or(1);
                                (event_json(100), format!(r#""cap-{}""#, n + 1))
                            }
                            _ => (event_json(100), r#""page-2""#.to_string()),
                        };
                        format!(
                            r#"{{"jsonrpc":"2.0","id":1,"result":{{"events":[{events_json}],"cursor":{cursor_json},"latestLedger":500}}}}"#
                        )
                    } else {
                        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#
                            .to_string()
                    };
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = sock.write_all(header.as_bytes()).await;
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn test_get_contract_events_page_returns_next_cursor() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let page = get_contract_events_page(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(100),
            Some(200),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].ledger, Some(100));
        assert_eq!(page.next_cursor.as_deref(), Some("page-2"));
        assert_eq!(page.latest_ledger, Some(500));
        assert_eq!(page.pages_fetched, 1);
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_follows_cursor() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let paged = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(100),
            Some(200),
            None,
            PaginationOptions {
                cursor: None,
                limit: None,
                max_pages: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(paged.events.len(), 2);
        assert_eq!(
            paged
                .events
                .iter()
                .map(|event| event.ledger)
                .collect::<Vec<_>>(),
            vec![Some(100), Some(200)]
        );
        // The third page returns an event at ledger 201, past the end
        // bound of 200: it is fetched, dropped, and the loop stops, so the
        // pagination is complete within the bound.
        assert_eq!(paged.pages_fetched, 3);
        assert_eq!(paged.next_cursor, None);
        assert_eq!(paged.latest_ledger, Some(500));
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_enforces_end_ledger_bound() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        // The reported repro: start=100, end=200, limit=1. The first page
        // carries endLedger=201; the cursor-only follow-ups do not, so the
        // bound must be enforced client-side.
        let paged = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(100),
            Some(200),
            None,
            PaginationOptions {
                cursor: None,
                limit: Some(1),
                max_pages: 10,
            },
        )
        .await
        .unwrap();
        // Before the bound (100), exactly at it (200), and past it (201,
        // fetched then dropped): only the first two survive.
        assert_eq!(
            paged
                .events
                .iter()
                .map(|event| event.ledger)
                .collect::<Vec<_>>(),
            vec![Some(100), Some(200)]
        );
        assert_eq!(paged.pages_fetched, 3);
        assert_eq!(paged.next_cursor, None);
        assert_eq!(paged.latest_ledger, Some(500));
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_stops_at_page_cap() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let paged = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            None,
            None,
            None,
            PaginationOptions {
                cursor: Some("cap-1".to_string()),
                limit: None,
                max_pages: 3,
            },
        )
        .await
        .unwrap();
        // The mock advances the cursor on every page (cap-1 -> cap-2 ->
        // cap-3 -> cap-4); the cap must stop the loop.
        assert_eq!(paged.events.len(), 3);
        assert_eq!(paged.pages_fetched, 3);
        assert_eq!(paged.next_cursor.as_deref(), Some("cap-4"));
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_rejects_non_advancing_cursor() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let result = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            None,
            None,
            None,
            PaginationOptions {
                cursor: Some("loop".to_string()),
                limit: None,
                max_pages: 10,
            },
        )
        .await;
        // The mock returns the same cursor and the same event forever;
        // following it would replay the page until the cap. Fail instead,
        // before appending the replayed page.
        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => assert!(msg.contains("did not advance"), "{msg}"),
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_stops_on_empty_final_page() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let paged = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            None,
            None,
            None,
            PaginationOptions {
                cursor: Some("empty-1".to_string()),
                limit: None,
                max_pages: 10,
            },
        )
        .await
        .unwrap();
        // The final page is empty but still carries a nonempty cursor:
        // the window is exhausted, so the loop stops after one page and
        // reports the pagination complete instead of fetching to the cap.
        assert_eq!(paged.events.len(), 0);
        assert_eq!(paged.pages_fetched, 1);
        assert_eq!(paged.next_cursor, None);
        assert_eq!(paged.latest_ledger, Some(500));
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_stops_when_latest_ledger_passed() {
        let url = spawn_mock_paged_events_server().await;
        let client = SorobanRpcClient::new(&url);
        let paged = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            None,
            None,
            None,
            PaginationOptions {
                cursor: Some("beyond".to_string()),
                limit: None,
                max_pages: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(paged.events.len(), 1);
        assert_eq!(paged.events[0].ledger, Some(999));
        assert_eq!(paged.pages_fetched, 1);
        assert_eq!(paged.latest_ledger, Some(500));
        // The cursor is still returned so the caller can resume if it chooses.
        assert_eq!(paged.next_cursor.as_deref(), Some("page-2"));
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_rejects_cursor_with_range_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            Some(100),
            None,
            None,
            PaginationOptions {
                cursor: Some("abc".to_string()),
                limit: None,
                max_pages: 10,
            },
        )
        .await;
        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => assert!(msg.contains("cannot be combined"), "{msg}"),
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_contract_events_paginated_rejects_zero_page_cap_before_network() {
        let client = SorobanRpcClient::new("http://127.0.0.1:9999");
        let result = get_contract_events_paginated(
            &client,
            "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5CWVMOMG2P3T2YYW",
            None,
            None,
            None,
            PaginationOptions {
                cursor: None,
                limit: None,
                max_pages: 0,
            },
        )
        .await;
        let err = result.unwrap_err();
        match err {
            RpcError::Rpc(msg) => assert!(msg.contains("at least 1"), "{msg}"),
            other => panic!("Unexpected error variant: {other:?}"),
        }
    }
}
