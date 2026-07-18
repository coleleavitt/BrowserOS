use crate::{
    db::audit::entities::{
        prelude::{TabClaims, TabRecordings},
        tab_claims, tab_recordings,
    },
    error::{AppError, AppResult, IoPath},
    services::audit::AuditService,
};
use sea_orm::{
    ActiveValue::Set,
    ColumnTrait, EntityTrait, QueryFilter,
    sea_query::{Alias, Expr, OnConflict},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::{info, warn};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const RETENTION_INTERVAL: Duration = Duration::from_secs(60 * 60);
const BATCH_ID_LRU_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEventInput {
    pub ts: i64,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub event_type: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedEvent {
    pub tab_id: i64,
    pub ts: i64,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub event_type: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionSweepResult {
    pub recordings_deleted: u64,
    pub claims_deleted: u64,
}

struct HandleEntry {
    file: Arc<Mutex<File>>,
    active_writes: usize,
    generation: u64,
    last_used: Instant,
}

#[derive(Default)]
struct HandleCache {
    entries: HashMap<String, HandleEntry>,
    lru: VecDeque<String>,
}

/// Stores target-keyed rrweb events and keeps the SQLite catalog in sync.
pub struct RecordingStore {
    root: PathBuf,
    audit: Arc<AuditService>,
    max_open_handles: usize,
    idle_handle: Duration,
    target_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    handles: Mutex<HandleCache>,
    accepted_batch_ids: Mutex<HashMap<String, VecDeque<String>>>,
}

impl RecordingStore {
    #[must_use]
    pub fn new(
        root: PathBuf,
        audit: Arc<AuditService>,
        max_open_handles: usize,
        idle_handle: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            audit,
            max_open_handles,
            idle_handle,
            target_locks: Mutex::new(HashMap::new()),
            handles: Mutex::new(HandleCache::default()),
            accepted_batch_ids: Mutex::new(HashMap::new()),
        })
    }

    pub async fn append_batch(
        self: &Arc<Self>,
        target_id: &str,
        tab_id: i64,
        events: &[RecordingEventInput],
    ) -> AppResult<()> {
        self.append_batch_with_id(target_id, tab_id, events, None)
            .await
            .map(|_| ())
    }

    /// Returns false only when this target already accepted the batch id.
    pub async fn append_batch_with_id(
        self: &Arc<Self>,
        target_id: &str,
        tab_id: i64,
        events: &[RecordingEventInput],
        batch_id: Option<&str>,
    ) -> AppResult<bool> {
        let target_lock = self.lock_for(target_id).await;
        let target_guard = target_lock.lock().await;
        let result = async {
            if let Some(batch_id) = batch_id
                && self.has_accepted_batch_id(target_id, batch_id).await
            {
                return Ok(false);
            }
            if events.is_empty() {
                if let Some(batch_id) = batch_id {
                    self.remember_accepted_batch_id(target_id, batch_id).await;
                }
                return Ok(true);
            }
            let mut payload = String::new();
            for event in events {
                payload.push_str(&serde_json::to_string(&RecordedEvent {
                    tab_id,
                    ts: event.ts,
                    event_type: event.event_type.clone(),
                    data: event.data.clone(),
                })?);
                payload.push('\n');
            }
            let first_event_at = events
                .iter()
                .map(|event| event.ts)
                .min()
                .unwrap_or_default();
            let last_event_at = events
                .iter()
                .map(|event| event.ts)
                .max()
                .unwrap_or_default();
            let file = self.open_for_append(target_id).await?;
            let result = self
                .append_and_catalog(
                    target_id,
                    tab_id,
                    events.len(),
                    first_event_at,
                    last_event_at,
                    payload.as_bytes(),
                    &file,
                )
                .await;
            self.release_append_handle(target_id).await;
            result?;
            if let Some(batch_id) = batch_id {
                self.remember_accepted_batch_id(target_id, batch_id).await;
            }
            Ok(true)
        }
        .await;
        drop(target_guard);
        self.release_target_lock(target_id, &target_lock).await;
        result
    }

    pub async fn read_range(
        &self,
        target_id: &str,
        from: i64,
        to: i64,
    ) -> AppResult<Vec<RecordedEvent>> {
        let target_lock = self.lock_for(target_id).await;
        let target_guard = target_lock.lock().await;
        let result = async {
            let path = self.path_for(target_id);
            let text = match fs::read_to_string(&path).await {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Vec::new());
                }
                Err(source) => {
                    return Err(AppError::Io {
                        path: Some(path),
                        source,
                    });
                }
            };
            Ok(text
                .lines()
                .filter_map(|line| serde_json::from_str::<RecordedEvent>(line).ok())
                .filter(|event| event.ts >= from && event.ts <= to)
                .collect())
        }
        .await;
        drop(target_guard);
        self.release_target_lock(target_id, &target_lock).await;
        result
    }

    pub async fn sweep_retention(
        &self,
        retention_days: u64,
        now: i64,
    ) -> AppResult<RetentionSweepResult> {
        let retention_ms = i64::try_from(retention_days)
            .unwrap_or(i64::MAX)
            .saturating_mul(DAY_MS);
        let cutoff = now.saturating_sub(retention_ms);
        let expired = TabRecordings::find()
            .filter(tab_recordings::Column::LastEventAt.lt(cutoff))
            .all(self.audit.connection())
            .await?;
        let mut recordings_deleted = 0;
        for recording in expired {
            if self
                .delete_recording_if_expired(&recording.target_id, cutoff)
                .await?
            {
                recordings_deleted += 1;
            }
        }

        let claims = TabClaims::find()
            .filter(tab_claims::Column::ReleasedAt.is_not_null())
            .filter(tab_claims::Column::ReleasedAt.lt(cutoff))
            .all(self.audit.connection())
            .await?;
        TabClaims::delete_many()
            .filter(tab_claims::Column::ReleasedAt.is_not_null())
            .filter(tab_claims::Column::ReleasedAt.lt(cutoff))
            .exec(self.audit.connection())
            .await?;
        Ok(RetentionSweepResult {
            recordings_deleted,
            claims_deleted: u64::try_from(claims.len()).unwrap_or(u64::MAX),
        })
    }

    /// Runs recording retention immediately and then hourly.
    pub fn spawn_retention(self: Arc<Self>, retention_days: u64) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = interval(RETENTION_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match self
                    .sweep_retention(retention_days, super::now_epoch_ms())
                    .await
                {
                    Ok(result) => info!(
                        recordings_deleted = result.recordings_deleted,
                        claims_deleted = result.claims_deleted,
                        "recording retention sweep finished"
                    ),
                    Err(error) => warn!(error = %error, "recording retention sweep failed"),
                }
            }
        })
    }

    pub async fn close(&self) {
        let mut cache = self.handles.lock().await;
        cache.entries.clear();
        cache.lru.clear();
    }

    #[cfg(test)]
    async fn cached_handle_count(&self) -> usize {
        self.handles.lock().await.entries.len()
    }

    #[cfg(test)]
    async fn target_lock_count(&self) -> usize {
        self.target_locks.lock().await.len()
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_and_catalog(
        &self,
        target_id: &str,
        tab_id: i64,
        event_count: usize,
        first_event_at: i64,
        last_event_at: i64,
        payload: &[u8],
        file: &Arc<Mutex<File>>,
    ) -> AppResult<()> {
        let path = self.path_for(target_id);
        let mut file = file.lock().await;
        let original_size = file.metadata().await.with_path(path.clone())?.len();
        let write_result = async {
            file.write_all(payload).await.with_path(path.clone())?;
            file.flush().await.with_path(path.clone())?;
            Ok::<(), AppError>(())
        }
        .await;
        if let Err(error) = write_result {
            rollback_append(&mut file, target_id, original_size).await;
            return Err(error);
        }
        let size_bytes = i64::try_from(payload.len()).unwrap_or(i64::MAX);
        let event_count = i64::try_from(event_count).unwrap_or(i64::MAX);
        let result = TabRecordings::insert(tab_recordings::ActiveModel {
            target_id: Set(target_id.to_string()),
            tab_id: Set(tab_id),
            first_event_at: Set(first_event_at),
            last_event_at: Set(last_event_at),
            size_bytes: Set(size_bytes),
            event_count: Set(event_count),
        })
        .on_conflict(
            OnConflict::column(tab_recordings::Column::TargetId)
                .update_column(tab_recordings::Column::TabId)
                .value(
                    tab_recordings::Column::FirstEventAt,
                    min_catalog_expr(tab_recordings::Column::FirstEventAt),
                )
                .value(
                    tab_recordings::Column::LastEventAt,
                    max_catalog_expr(tab_recordings::Column::LastEventAt),
                )
                .value(
                    tab_recordings::Column::SizeBytes,
                    add_catalog_expr(tab_recordings::Column::SizeBytes),
                )
                .value(
                    tab_recordings::Column::EventCount,
                    add_catalog_expr(tab_recordings::Column::EventCount),
                )
                .to_owned(),
        )
        .exec_without_returning(self.audit.connection())
        .await;
        if let Err(error) = result {
            rollback_append(&mut file, target_id, original_size).await;
            return Err(error.into());
        }
        Ok(())
    }

    async fn delete_recording_if_expired(&self, target_id: &str, cutoff: i64) -> AppResult<bool> {
        let target_lock = self.lock_for(target_id).await;
        let target_guard = target_lock.lock().await;
        let result = self.delete_recording_locked(target_id, cutoff).await;
        drop(target_guard);
        self.release_target_lock(target_id, &target_lock).await;
        result
    }

    async fn delete_recording_locked(&self, target_id: &str, cutoff: i64) -> AppResult<bool> {
        let Some(recording) = TabRecordings::find_by_id(target_id)
            .one(self.audit.connection())
            .await?
        else {
            return Ok(false);
        };
        if recording.last_event_at >= cutoff {
            return Ok(false);
        }
        self.close_cached_handle(target_id).await;
        let path = self.path_for(target_id);
        match fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warn!(target_id, error = %error, "recording retention unlink failed");
                return Ok(false);
            }
        }
        TabRecordings::delete_by_id(target_id)
            .exec(self.audit.connection())
            .await?;
        self.accepted_batch_ids.lock().await.remove(target_id);
        Ok(true)
    }

    async fn lock_for(&self, target_id: &str) -> Arc<Mutex<()>> {
        self.target_locks
            .lock()
            .await
            .entry(target_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn release_target_lock(&self, target_id: &str, target_lock: &Arc<Mutex<()>>) {
        let mut locks = self.target_locks.lock().await;
        let removable = locks.get(target_id).is_some_and(|stored| {
            Arc::ptr_eq(stored, target_lock) && Arc::strong_count(stored) == 2
        });
        if removable {
            locks.remove(target_id);
        }
    }

    async fn has_accepted_batch_id(&self, target_id: &str, batch_id: &str) -> bool {
        let mut accepted = self.accepted_batch_ids.lock().await;
        let Some(target_ids) = accepted.get_mut(target_id) else {
            return false;
        };
        let Some(index) = target_ids
            .iter()
            .position(|candidate| candidate == batch_id)
        else {
            return false;
        };
        if let Some(batch_id) = target_ids.remove(index) {
            target_ids.push_back(batch_id);
        }
        true
    }

    async fn remember_accepted_batch_id(&self, target_id: &str, batch_id: &str) {
        let mut accepted = self.accepted_batch_ids.lock().await;
        let target_ids = accepted.entry(target_id.to_string()).or_default();
        target_ids.push_back(batch_id.to_string());
        if target_ids.len() > BATCH_ID_LRU_CAPACITY {
            target_ids.pop_front();
        }
    }

    async fn open_for_append(self: &Arc<Self>, target_id: &str) -> AppResult<Arc<Mutex<File>>> {
        {
            let mut cache = self.handles.lock().await;
            if cache.entries.contains_key(target_id) {
                let file = {
                    let entry = cache
                        .entries
                        .get_mut(target_id)
                        .unwrap_or_else(|| unreachable!());
                    entry.active_writes += 1;
                    entry.generation = entry.generation.wrapping_add(1);
                    entry.last_used = Instant::now();
                    entry.file.clone()
                };
                touch_lru(&mut cache, target_id);
                return Ok(file);
            }
        }

        fs::create_dir_all(&self.root)
            .await
            .with_path(self.root.clone())?;
        let path = self.path_for(target_id);
        let file = Arc::new(Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .with_path(path)?,
        ));
        let mut cache = self.handles.lock().await;
        cache.entries.insert(
            target_id.to_string(),
            HandleEntry {
                file: file.clone(),
                active_writes: 1,
                generation: 1,
                last_used: Instant::now(),
            },
        );
        touch_lru(&mut cache, target_id);
        evict_over_limit(&mut cache, self.max_open_handles);
        Ok(file)
    }

    async fn release_append_handle(self: &Arc<Self>, target_id: &str) {
        let generation = {
            let mut cache = self.handles.lock().await;
            let Some(entry) = cache.entries.get_mut(target_id) else {
                return;
            };
            entry.active_writes = entry.active_writes.saturating_sub(1);
            entry.generation = entry.generation.wrapping_add(1);
            entry.last_used = Instant::now();
            let generation = entry.generation;
            touch_lru(&mut cache, target_id);
            evict_over_limit(&mut cache, self.max_open_handles);
            generation
        };
        let store = Arc::downgrade(self);
        let target_id = target_id.to_string();
        let idle_handle = self.idle_handle;
        tokio::spawn(async move {
            tokio::time::sleep(idle_handle).await;
            close_if_idle(store, &target_id, generation, idle_handle).await;
        });
    }

    async fn close_cached_handle(&self, target_id: &str) {
        let mut cache = self.handles.lock().await;
        cache.entries.remove(target_id);
        cache.lru.retain(|candidate| candidate != target_id);
    }

    fn path_for(&self, target_id: &str) -> PathBuf {
        self.root
            .join(format!("{}.ndjson", sanitize_target_id(target_id)))
    }
}

async fn rollback_append(file: &mut File, target_id: &str, original_size: u64) {
    if let Err(error) = file.set_len(original_size).await {
        warn!(target_id, error = %error, "recording append rollback failed");
    }
}

fn touch_lru(cache: &mut HandleCache, target_id: &str) {
    cache.lru.retain(|candidate| candidate != target_id);
    cache.lru.push_back(target_id.to_string());
}

fn evict_over_limit(cache: &mut HandleCache, max_open_handles: usize) {
    while cache.entries.len() > max_open_handles {
        let Some(index) = cache.lru.iter().position(|target_id| {
            cache
                .entries
                .get(target_id)
                .is_some_and(|entry| entry.active_writes == 0)
        }) else {
            return;
        };
        if let Some(target_id) = cache.lru.remove(index) {
            cache.entries.remove(&target_id);
        }
    }
}

async fn close_if_idle(
    store: Weak<RecordingStore>,
    target_id: &str,
    generation: u64,
    idle_handle: Duration,
) {
    let Some(store) = store.upgrade() else {
        return;
    };
    let mut cache = store.handles.lock().await;
    let should_close = cache.entries.get(target_id).is_some_and(|entry| {
        entry.active_writes == 0
            && entry.generation == generation
            && entry.last_used.elapsed() >= idle_handle
    });
    if should_close {
        cache.entries.remove(target_id);
        cache.lru.retain(|candidate| candidate != target_id);
    }
}

fn catalog_expr(column: tab_recordings::Column) -> [sea_orm::sea_query::SimpleExpr; 2] {
    [
        Expr::col((Alias::new("tab_recordings"), column)).into(),
        Expr::col((Alias::new("excluded"), column)).into(),
    ]
}

fn min_catalog_expr(column: tab_recordings::Column) -> sea_orm::sea_query::SimpleExpr {
    Expr::cust_with_exprs("min(?, ?)", catalog_expr(column))
}

fn max_catalog_expr(column: tab_recordings::Column) -> sea_orm::sea_query::SimpleExpr {
    Expr::cust_with_exprs("max(?, ?)", catalog_expr(column))
}

fn add_catalog_expr(column: tab_recordings::Column) -> sea_orm::sea_query::SimpleExpr {
    Expr::col((Alias::new("tab_recordings"), column))
        .add(Expr::col((Alias::new("excluded"), column)))
}

fn sanitize_target_id(target_id: &str) -> String {
    target_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BATCH_ID_LRU_CAPACITY, RecordedEvent, RecordingEventInput, RecordingStore};
    use crate::{
        db::audit::entities::{
            prelude::{TabClaims, TabRecordings},
            tab_claims,
        },
        services::audit::AuditService,
    };
    use sea_orm::{
        ActiveValue::{NotSet, Set},
        ConnectionTrait, EntityTrait,
    };
    use serde_json::json;
    use std::{sync::Arc, time::Duration};
    use tempfile::tempdir;

    fn event(ts: i64, value: &str) -> RecordingEventInput {
        RecordingEventInput {
            ts,
            event_type: Some(json!(3)),
            data: Some(json!({ "value": value })),
        }
    }

    async fn store(
        root: &std::path::Path,
    ) -> anyhow::Result<(Arc<AuditService>, Arc<RecordingStore>)> {
        let audit = Arc::new(AuditService::open(root.join("audit.sqlite")).await?);
        let store = RecordingStore::new(
            root.join("recordings"),
            audit.clone(),
            50,
            Duration::from_secs(30),
        );
        Ok((audit, store))
    }

    #[tokio::test]
    async fn appends_stamped_events_and_upserts_catalog_totals() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (audit, store) = store(dir.path()).await?;
        store
            .append_batch("target-a", 11, &[event(200, "second"), event(100, "first")])
            .await?;
        store
            .append_batch("target-a", 11, &[event(300, "third")])
            .await?;

        let text = tokio::fs::read_to_string(dir.path().join("recordings/target-a.ndjson")).await?;
        let events = text
            .lines()
            .map(serde_json::from_str::<RecordedEvent>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].tab_id, 11);
        assert_eq!(events[0].ts, 200);

        let row = TabRecordings::find_by_id("target-a")
            .one(audit.connection())
            .await?
            .unwrap_or_else(|| panic!("catalog row missing"));
        assert_eq!(row.first_event_at, 100);
        assert_eq!(row.last_event_at, 300);
        assert_eq!(row.size_bytes, i64::try_from(text.len())?);
        assert_eq!(row.event_count, 3);
        assert_eq!(store.read_range("target-a", 100, 200).await?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn serializes_concurrent_target_appends_without_tearing() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (_, store) = store(dir.path()).await?;
        let first = (0..50)
            .map(|index| event(index, "first"))
            .collect::<Vec<_>>();
        let second = (50..100)
            .map(|index| event(index, "second"))
            .collect::<Vec<_>>();

        tokio::try_join!(
            store.append_batch("target-b", 22, &first),
            store.append_batch("target-b", 22, &second)
        )?;

        let text = tokio::fs::read_to_string(dir.path().join("recordings/target-b.ndjson")).await?;
        assert_eq!(text.lines().count(), 100);
        for line in text.lines() {
            serde_json::from_str::<RecordedEvent>(line)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn deduplicates_recent_batch_ids_per_target() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (_, store) = store(dir.path()).await?;
        assert!(
            store
                .append_batch_with_id("target-dedupe", 23, &[event(1, "first")], Some("batch-0"))
                .await?
        );
        assert!(
            !store
                .append_batch_with_id("target-dedupe", 23, &[event(1, "first")], Some("batch-0"))
                .await?
        );
        for index in 1..=BATCH_ID_LRU_CAPACITY {
            let batch_id = format!("batch-{index}");
            assert!(
                store
                    .append_batch_with_id(
                        "target-dedupe",
                        23,
                        &[event(i64::try_from(index + 1)?, "next")],
                        Some(&batch_id),
                    )
                    .await?
            );
        }
        assert!(
            store
                .append_batch_with_id(
                    "target-dedupe",
                    23,
                    &[event(999, "evicted")],
                    Some("batch-0")
                )
                .await?
        );

        let text =
            tokio::fs::read_to_string(dir.path().join("recordings/target-dedupe.ndjson")).await?;
        assert_eq!(text.lines().count(), BATCH_ID_LRU_CAPACITY + 2);
        Ok(())
    }

    #[tokio::test]
    async fn rolls_back_file_bytes_when_catalog_update_fails() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (audit, store) = store(dir.path()).await?;
        audit
            .connection()
            .execute_unprepared("DROP TABLE tab_recordings")
            .await?;

        assert!(
            store
                .append_batch("target-rollback", 1, &[event(1, "discard")])
                .await
                .is_err()
        );
        assert_eq!(
            tokio::fs::read(dir.path().join("recordings/target-rollback.ndjson")).await?,
            b""
        );
        Ok(())
    }

    #[tokio::test]
    async fn sanitizes_target_ids_used_as_filenames() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (_, store) = store(dir.path()).await?;
        store
            .append_batch("../target/d", 44, &[event(1, "safe")])
            .await?;
        assert!(dir.path().join("recordings/.._target_d.ndjson").exists());
        Ok(())
    }

    #[tokio::test]
    async fn bounds_the_handle_cache_and_closes_idle_entries() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let audit = Arc::new(AuditService::open(dir.path().join("audit.sqlite")).await?);
        let store = RecordingStore::new(
            dir.path().join("recordings"),
            audit,
            1,
            Duration::from_millis(10),
        );
        store
            .append_batch("target-one", 1, &[event(1, "one")])
            .await?;
        store
            .append_batch("target-two", 2, &[event(2, "two")])
            .await?;
        assert_eq!(store.cached_handle_count().await, 1);
        assert_eq!(store.target_lock_count().await, 0);

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(store.cached_handle_count().await, 0);
        Ok(())
    }

    #[tokio::test]
    async fn retention_sweeps_old_files_rows_and_only_closed_claims() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let (audit, store) = store(dir.path()).await?;
        let day = 24 * 60 * 60 * 1000;
        let now = 10 * day;
        store
            .append_batch("old-target", 1, &[event(now - 8 * day, "old")])
            .await?;
        store
            .append_batch("fresh-target", 2, &[event(now - day, "fresh")])
            .await?;
        TabClaims::insert_many([
            tab_claims::ActiveModel {
                id: NotSet,
                target_id: Set("old-target".to_string()),
                session_id: Set("old-session".to_string()),
                agent_id: Set("agent".to_string()),
                claimed_at: Set(now - 9 * day),
                released_at: Set(Some(now - 8 * day)),
            },
            tab_claims::ActiveModel {
                id: NotSet,
                target_id: Set("old-target".to_string()),
                session_id: Set("open-session".to_string()),
                agent_id: Set("agent".to_string()),
                claimed_at: Set(now - 9 * day),
                released_at: Set(None),
            },
        ])
        .exec(audit.connection())
        .await?;

        let result = store.sweep_retention(7, now).await?;

        assert_eq!(result.recordings_deleted, 1);
        assert_eq!(result.claims_deleted, 1);
        assert!(!dir.path().join("recordings/old-target.ndjson").exists());
        assert!(dir.path().join("recordings/fresh-target.ndjson").exists());
        assert!(
            TabRecordings::find_by_id("old-target")
                .one(audit.connection())
                .await?
                .is_none()
        );
        assert_eq!(TabClaims::find().all(audit.connection()).await?.len(), 1);
        Ok(())
    }
}
