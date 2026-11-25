//! Live event polling (`sdkt events --follow`).
//!
//! Polls Soroban RPC's `getEvents` endpoint via [`get_contract_events_filtered`]
//! on a fixed interval, advancing the ledger cursor without gaps or duplicates.
//!
//! **Burst caveat**: a burst larger than one RPC response page inside a single
//! poll interval can lose events; follow mode does not claim at-least-once
//! delivery.

use std::collections::HashSet;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use sdkt_rpc::client::SorobanRpcClient;
use sdkt_rpc::events::{get_contract_events_filtered, ContractEvent, TopicFilter};

/// Maximum number of consecutive poll failures before exiting non-zero.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Tracks the ledger range cursor for `sdkt events --follow`.
///
/// Guarantees:
/// - Never skips a ledger range between polls: `next_start_ledger` only
///   advances to `max_event_ledger + 1` after events are observed.
/// - Never emits the same event twice across polls, even if consecutive
///   responses overlap on ledger numbers.
#[derive(Debug, Clone)]
pub struct FollowCursor {
    initial_start_ledger: u32,
    next_start_ledger: u32,
    last_ledger_covered: Option<u32>,
    seen_unledgered: HashSet<(String, Vec<String>, Option<String>)>,
}

impl FollowCursor {
    /// Create a new cursor starting at `start_ledger` (inclusive).
    pub fn new(start_ledger: u32) -> Self {
        Self {
            initial_start_ledger: start_ledger,
            next_start_ledger: start_ledger,
            last_ledger_covered: None,
            seen_unledgered: HashSet::new(),
        }
    }

    /// Ledger sequence number to pass as `start_ledger` on the next poll.
    pub fn next_start_ledger(&self) -> u32 {
        self.next_start_ledger
    }

    /// The initial ledger sequence number where following started.
    pub fn initial_start_ledger(&self) -> u32 {
        self.initial_start_ledger
    }

    /// Highest ledger sequence number covered so far, or `initial_start_ledger`
    /// if no events have been observed yet.
    pub fn last_ledger_covered(&self) -> u32 {
        self.last_ledger_covered
            .unwrap_or(self.initial_start_ledger)
    }

    /// Filter a raw page of events to only those not yet seen, and advance the
    /// cursor past the highest ledger observed in the accepted events.
    pub fn ingest(&mut self, events: Vec<ContractEvent>) -> Vec<ContractEvent> {
        let mut accepted = Vec::new();
        let mut max_ledger: Option<u32> = None;

        for ev in events {
            match ev.ledger {
                Some(ledger) => {
                    if ledger < self.next_start_ledger {
                        continue;
                    }
                    max_ledger = Some(max_ledger.map_or(ledger, |m| m.max(ledger)));
                    accepted.push(ev);
                }
                None => {
                    let key = (ev.contract_id.clone(), ev.topics.clone(), ev.value.clone());
                    if self.seen_unledgered.insert(key) {
                        accepted.push(ev);
                    }
                }
            }
        }

        if let Some(max_l) = max_ledger {
            self.last_ledger_covered = Some(
                self.last_ledger_covered
                    .map_or(max_l, |prev| prev.max(max_l)),
            );
            self.next_start_ledger = self.next_start_ledger.max(max_l.saturating_add(1));
        }

        accepted
    }
}

/// Configuration for [`run_follow_loop`].
pub struct FollowConfig {
    pub contract_id: String,
    pub start_ledger: Option<u32>,
    pub interval_secs: u64,
    pub max_events: Option<usize>,
    pub max_seconds: Option<u64>,
    pub topic_filters: Option<Vec<TopicFilter>>,
    pub json_output: bool,
}

/// Run the live event follow loop.
///
/// `render_event` is invoked for each newly arrived event with its 1-based
/// sequence number and should write the formatted event to stdout.
///
/// Returns `Ok(())` when stopped cleanly (Ctrl-C, `--max-events`, or
/// `--max-seconds`), or `Err(msg)` if the initial ledger lookup fails or
/// [`MAX_CONSECUTIVE_FAILURES`] consecutive poll requests fail.
pub async fn run_follow_loop<F>(
    client: &SorobanRpcClient,
    config: FollowConfig,
    mut render_event: F,
) -> Result<(), String>
where
    F: FnMut(usize, &ContractEvent) -> Result<(), String>,
{
    let start_ledger = match config.start_ledger {
        Some(l) => l,
        None => {
            let info = client
                .get_ledger()
                .await
                .map_err(|e| format!("Error fetching latest ledger: {e}"))?;
            info.sequence
        }
    };

    let startup_line = format!(
        "Following events for {} from ledger {} (poll interval: {}s)...",
        config.contract_id, start_ledger, config.interval_secs
    );
    if config.json_output {
        eprintln!("{startup_line}");
    } else {
        println!("{startup_line}");
        let _ = io::stdout().flush();
    }

    let mut cursor = FollowCursor::new(start_ledger);
    let mut events_seen: usize = 0;
    let mut consecutive_failures: u32 = 0;
    let started_at = Instant::now();
    let max_duration = config.max_seconds.map(Duration::from_secs);
    let poll_interval = Duration::from_secs(config.interval_secs);

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        if let Some(limit) = config.max_events {
            if events_seen >= limit {
                break;
            }
        }
        if let Some(max_dur) = max_duration {
            if started_at.elapsed() >= max_dur {
                break;
            }
        }

        let poll_fut = get_contract_events_filtered(
            client,
            &config.contract_id,
            Some(cursor.next_start_ledger()),
            None,
            config.topic_filters.clone(),
        );

        let poll_result = tokio::select! {
            _ = &mut ctrl_c => {
                break;
            }
            res = poll_fut => res,
        };

        match poll_result {
            Ok(raw_events) => {
                consecutive_failures = 0;
                let new_events = cursor.ingest(raw_events);
                for ev in &new_events {
                    events_seen += 1;
                    render_event(events_seen, ev)?;
                    let _ = io::stdout().flush();
                    if let Some(limit) = config.max_events {
                        if events_seen >= limit {
                            break;
                        }
                    }
                }
                if let Some(limit) = config.max_events {
                    if events_seen >= limit {
                        break;
                    }
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                eprintln!(
                    "Error polling events (attempt {consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}): {e}"
                );
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(format!(
                        "Aborting after {MAX_CONSECUTIVE_FAILURES} consecutive poll failures"
                    ));
                }
            }
        }

        // Check time budget before sleeping.
        let sleep_dur = match max_duration {
            Some(max_dur) => {
                let elapsed = started_at.elapsed();
                if elapsed >= max_dur {
                    break;
                }
                poll_interval.min(max_dur - elapsed)
            }
            None => poll_interval,
        };

        tokio::select! {
            _ = &mut ctrl_c => {
                break;
            }
            _ = tokio::time::sleep(sleep_dur) => {}
        }
    }

    let summary_line = format!(
        "Stopped following events: {} event(s) seen, ledgers {}..={} covered.",
        events_seen,
        cursor.initial_start_ledger(),
        cursor.last_ledger_covered()
    );
    if config.json_output {
        eprintln!("{summary_line}");
    } else {
        println!("{summary_line}");
        let _ = io::stdout().flush();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(ledger: u32, val: &str) -> ContractEvent {
        ContractEvent {
            contract_id: "CCVVW7N4R3KNY72QJQKQY3T753C2H34E6XJIVJQOQSQE3C3M3U72QJQK".to_string(),
            ledger: Some(ledger),
            topics: vec!["AAAAAwAAACo=".to_string()],
            value: Some(val.to_string()),
        }
    }

    #[test]
    fn cursor_advances_without_gaps_or_duplicates_across_overlapping_pages() {
        let mut cursor = FollowCursor::new(100);
        assert_eq!(cursor.next_start_ledger(), 100);

        // Page 1: events in ledgers 100 and 101.
        let page1 = vec![make_event(100, "v1"), make_event(101, "v2")];
        let emitted1 = cursor.ingest(page1);
        assert_eq!(emitted1.len(), 2);
        assert_eq!(emitted1[0].ledger, Some(100));
        assert_eq!(emitted1[1].ledger, Some(101));
        assert_eq!(cursor.next_start_ledger(), 102);
        assert_eq!(cursor.last_ledger_covered(), 101);

        // Page 2: overlaps on ledger 101 and adds ledgers 102 and 103.
        let page2 = vec![
            make_event(101, "v2"),
            make_event(102, "v3"),
            make_event(103, "v4"),
        ];
        let emitted2 = cursor.ingest(page2);
        assert_eq!(emitted2.len(), 2);
        assert_eq!(emitted2[0].ledger, Some(102));
        assert_eq!(emitted2[1].ledger, Some(103));
        assert_eq!(cursor.next_start_ledger(), 104);
        assert_eq!(cursor.last_ledger_covered(), 103);
    }

    #[test]
    fn cursor_does_not_skip_ledgers_on_empty_poll() {
        let mut cursor = FollowCursor::new(500);
        let emitted = cursor.ingest(vec![]);
        assert!(emitted.is_empty());
        assert_eq!(cursor.next_start_ledger(), 500);
        assert_eq!(cursor.last_ledger_covered(), 500);
    }
}
