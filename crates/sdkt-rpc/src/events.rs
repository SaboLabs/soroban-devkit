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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetEventsRequest {
    pub start_ledger: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ledger: Option<u32>,
    pub filters: Vec<EventFilter>,
}

#[derive(Deserialize)]
struct GetEventsResponse {
    events: Vec<RpcEvent>,
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

    let request_body = GetEventsRequest {
        start_ledger,
        end_ledger: request_end_ledger,
        filters: vec![EventFilter {
            topics,
            ..EventFilter::contract([contract_id])
        }],
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

    Ok(contract_events)
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
            start_ledger: 1000,
            end_ledger: Some(2000),
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
            start_ledger: 1000,
            end_ledger: None,
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
}
