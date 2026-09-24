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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventFilter {
    #[serde(rename = "type")]
    filter_type: String,
    contract_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topics: Option<Vec<Vec<String>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GetEventsRequest {
    start_ledger: u32,
    filters: Vec<EventFilter>,
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

pub async fn get_contract_events(
    client: &SorobanRpcClient,
    contract_id: &str,
) -> Result<Vec<ContractEvent>, RpcError> {
    get_contract_events_with_topic(client, contract_id, None).await
}

/// Fetch contract events, optionally matching the first topic position.
pub async fn get_contract_events_with_topic(
    client: &SorobanRpcClient,
    contract_id: &str,
    topic: Option<&str>,
) -> Result<Vec<ContractEvent>, RpcError> {
    // Determine a start ledger. For a robust tool, this should be configurable.
    // Here we query the latest ledger and look back up to 1000 ledgers.
    let latest_ledger_info =
        client
            .get_ledger()
            .await
            .unwrap_or_else(|_| crate::client::LedgerInfo {
                id: "".to_string(),
                protocol_version: 0,
                sequence: 1000,
            });

    let start_ledger = latest_ledger_info.sequence.saturating_sub(1000).max(1);

    let request_body = GetEventsRequest {
        start_ledger,
        filters: vec![EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec![contract_id.to_string()],
            topics: topic.map(|topic| vec![vec![topic.to_string()]]),
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
    use super::EventFilter;

    #[test]
    fn event_filter_omits_topics_when_unfiltered() {
        let filter = EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec!["C...".to_string()],
            topics: None,
        };

        assert_eq!(
            serde_json::to_value(filter).unwrap(),
            serde_json::json!({
                "type": "contract",
                "contractIds": ["C..."]
            })
        );
    }

    #[test]
    fn event_filter_serializes_first_topic_matcher() {
        let filter = EventFilter {
            filter_type: "contract".to_string(),
            contract_ids: vec!["C...".to_string()],
            topics: Some(vec![vec!["encoded-transfer".to_string()]]),
        };

        assert_eq!(
            serde_json::to_value(filter).unwrap(),
            serde_json::json!({
                "type": "contract",
                "contractIds": ["C..."],
                "topics": [["encoded-transfer"]]
            })
        );
    }
}
