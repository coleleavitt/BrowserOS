use crate::{
    db::audit::entities::{
        prelude::{TabClaims, TabRecordings},
        tab_claims,
    },
    error::AppResult,
    services::{
        audit::AuditService,
        recordings::{RecordedEvent, RecordingStore},
    },
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BinaryHeap, HashMap},
    sync::Arc,
};

const RELEASE_TAIL_MS: i64 = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEvent {
    pub session_id: String,
    pub target_id: String,
    #[serde(flatten)]
    pub event: RecordedEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayTargetMeta {
    pub target_id: String,
    pub tab_id: i64,
    pub first_event_at: i64,
    pub last_event_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayMeta {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_event_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<i64>,
    pub size_bytes: i64,
    pub targets: Vec<ReplayTargetMeta>,
}

/// Assembles session replays by slicing target recordings through claim windows.
pub struct ReplayService {
    recordings: Arc<RecordingStore>,
    audit: Arc<AuditService>,
}

impl ReplayService {
    #[must_use]
    pub fn new(recordings: Arc<RecordingStore>, audit: Arc<AuditService>) -> Arc<Self> {
        Arc::new(Self { recordings, audit })
    }

    pub async fn read_session(&self, session_id: &str) -> AppResult<Vec<ReplayEvent>> {
        let claims = TabClaims::find()
            .filter(tab_claims::Column::SessionId.eq(session_id))
            .all(self.audit.connection())
            .await?;
        let mut slices = Vec::with_capacity(claims.len());
        for claim in claims {
            let to = claim
                .released_at
                .map(|released_at| released_at.saturating_add(RELEASE_TAIL_MS))
                .unwrap_or(i64::MAX);
            let mut events = self
                .recordings
                .read_range(&claim.target_id, claim.claimed_at, to)
                .await?
                .into_iter()
                .map(|event| ReplayEvent {
                    session_id: session_id.to_string(),
                    target_id: claim.target_id.clone(),
                    event,
                })
                .collect::<Vec<_>>();
            events.sort_by_key(|event| event.event.ts);
            slices.push(events);
        }
        Ok(merge_slices(slices))
    }

    pub async fn meta(&self, session_id: &str) -> AppResult<ReplayMeta> {
        let claims = TabClaims::find()
            .filter(tab_claims::Column::SessionId.eq(session_id))
            .all(self.audit.connection())
            .await?;
        if claims.is_empty() {
            return Ok(empty_meta());
        }
        let recordings = TabRecordings::find()
            .all(self.audit.connection())
            .await?
            .into_iter()
            .map(|recording| (recording.target_id.clone(), recording))
            .collect::<HashMap<_, _>>();
        let mut claims_by_target = BTreeMap::<String, Vec<tab_claims::Model>>::new();
        for claim in claims {
            claims_by_target
                .entry(claim.target_id.clone())
                .or_default()
                .push(claim);
        }

        let mut targets = Vec::new();
        let mut size_bytes = 0_i64;
        for (target_id, claims) in claims_by_target {
            let Some(recording) = recordings.get(&target_id) else {
                continue;
            };
            let claimed_at = claims
                .iter()
                .map(|claim| claim.claimed_at)
                .min()
                .unwrap_or(recording.first_event_at);
            let released_at = claims
                .iter()
                .map(|claim| claim.released_at.unwrap_or(recording.last_event_at))
                .max()
                .unwrap_or(recording.last_event_at);
            let first_event_at = claimed_at.max(recording.first_event_at);
            let last_event_at = released_at.min(recording.last_event_at);
            if first_event_at > last_event_at {
                continue;
            }
            targets.push(ReplayTargetMeta {
                target_id,
                tab_id: recording.tab_id,
                first_event_at,
                last_event_at,
            });
            // Metadata intentionally reports whole target-file bytes; claim-window byte counts require file IO.
            size_bytes = size_bytes.saturating_add(recording.size_bytes);
        }
        if targets.is_empty() {
            return Ok(empty_meta());
        }
        Ok(ReplayMeta {
            exists: true,
            first_event_at: targets.iter().map(|target| target.first_event_at).min(),
            last_event_at: targets.iter().map(|target| target.last_event_at).max(),
            size_bytes,
            targets,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeapEntry {
    ts: i64,
    slice: usize,
    event: usize,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.ts, self.slice, self.event).cmp(&(other.ts, other.slice, other.event))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn merge_slices(slices: Vec<Vec<ReplayEvent>>) -> Vec<ReplayEvent> {
    let capacity = slices.iter().map(Vec::len).sum();
    let mut merged = Vec::with_capacity(capacity);
    let mut heap = BinaryHeap::new();
    for (slice, events) in slices.iter().enumerate() {
        if let Some(event) = events.first() {
            heap.push(Reverse(HeapEntry {
                ts: event.event.ts,
                slice,
                event: 0,
            }));
        }
    }
    while let Some(Reverse(entry)) = heap.pop() {
        merged.push(slices[entry.slice][entry.event].clone());
        let next = entry.event + 1;
        if let Some(event) = slices[entry.slice].get(next) {
            heap.push(Reverse(HeapEntry {
                ts: event.event.ts,
                slice: entry.slice,
                event: next,
            }));
        }
    }
    merged
}

fn empty_meta() -> ReplayMeta {
    ReplayMeta {
        exists: false,
        first_event_at: None,
        last_event_at: None,
        size_bytes: 0,
        targets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::ReplayService;
    use crate::{
        db::audit::entities::{prelude::TabClaims, tab_claims},
        services::{
            audit::AuditService,
            recordings::{RecordingEventInput, RecordingStore},
        },
    };
    use sea_orm::{
        ActiveValue::{NotSet, Set},
        EntityTrait,
    };
    use serde_json::json;
    use std::{sync::Arc, time::Duration};
    use tempfile::tempdir;

    fn event(ts: i64, id: &str) -> RecordingEventInput {
        RecordingEventInput {
            ts,
            event_type: Some(json!(3)),
            data: Some(json!({ "id": id })),
        }
    }

    #[tokio::test]
    async fn merges_claimed_slices_and_excludes_unclaimed_targets() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let audit = Arc::new(AuditService::open(dir.path().join("audit.sqlite")).await?);
        let recordings = RecordingStore::new(
            dir.path().join("recordings"),
            audit.clone(),
            50,
            Duration::from_secs(30),
        );
        recordings
            .append_batch(
                "target-a",
                11,
                &[event(90, "outside"), event(100, "a1"), event(200, "a2")],
            )
            .await?;
        recordings
            .append_batch(
                "target-b",
                22,
                &[event(160, "b1"), event(180, "b2-buffered")],
            )
            .await?;
        recordings
            .append_batch("target-c", 33, &[event(170, "unclaimed")])
            .await?;
        TabClaims::insert_many([
            claim("target-a", "session-a", 100, Some(200)),
            claim("target-b", "session-a", 150, Some(170)),
        ])
        .exec(audit.connection())
        .await?;
        let replays = ReplayService::new(recordings, audit.clone());

        let events = replays.read_session("session-a").await?;

        assert_eq!(
            events
                .iter()
                .filter_map(|event| event.event.data.as_ref()?.get("id")?.as_str())
                .collect::<Vec<_>>(),
            ["a1", "b1", "b2-buffered", "a2"]
        );
        assert_eq!(events[0].session_id, "session-a");
        assert_eq!(events[0].target_id, "target-a");

        let meta = replays.meta("session-a").await?;
        assert!(meta.exists);
        assert_eq!(meta.first_event_at, Some(100));
        assert_eq!(meta.last_event_at, Some(200));
        assert_eq!(meta.targets.len(), 2);
        assert_eq!(meta.targets[1].last_event_at, 170);
        Ok(())
    }

    #[tokio::test]
    async fn returns_empty_replay_and_metadata_without_claims() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let audit = Arc::new(AuditService::open(dir.path().join("audit.sqlite")).await?);
        let recordings = RecordingStore::new(
            dir.path().join("recordings"),
            audit.clone(),
            50,
            Duration::from_secs(30),
        );
        let replays = ReplayService::new(recordings, audit);

        assert!(replays.read_session("missing").await?.is_empty());
        assert_eq!(
            serde_json::to_value(replays.meta("missing").await?)?,
            json!({ "exists": false, "sizeBytes": 0, "targets": [] })
        );
        Ok(())
    }

    fn claim(
        target_id: &str,
        session_id: &str,
        claimed_at: i64,
        released_at: Option<i64>,
    ) -> tab_claims::ActiveModel {
        tab_claims::ActiveModel {
            id: NotSet,
            target_id: Set(target_id.to_string()),
            session_id: Set(session_id.to_string()),
            agent_id: Set("agent".to_string()),
            claimed_at: Set(claimed_at),
            released_at: Set(released_at),
        }
    }
}
