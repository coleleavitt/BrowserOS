use browseros_core::TargetId;
use serde::Serialize;
use std::{
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const ACTIVE_WINDOW: Duration = Duration::from_secs(30);
const RECENT_TOOLS_CAP: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    pub name: String,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabActivityRecord {
    pub target_id: String,
    // tab_id and session_id are skipped because this struct serializes
    // as the legacy `/tabs/activity` wire shape, which stays frozen;
    // the canonical `/api/v1/tabs` reads both fields in-process. tab_id
    // is the browser tab id — the join key between recorder batches
    // (which only know tab ids) and CDP-side state; session_id is the
    // MCP session currently claiming the tab, which recording ingest
    // checks before accepting a batch.
    #[serde(skip)]
    pub tab_id: i64,
    pub page_id: u32,
    pub url: String,
    pub title: String,
    #[serde(skip)]
    pub session_id: String,
    pub agent_id: String,
    pub slug: String,
    pub first_tool_at: i64,
    pub last_tool_at: i64,
    pub last_tool_name: String,
    pub tool_count: usize,
    pub recent_tools: Vec<ToolEvent>,
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichedTabRecord {
    #[serde(flatten)]
    pub record: TabActivityRecord,
    pub agent_label: String,
    pub harness: Option<String>,
    pub color: Option<String>,
    pub screencast: Option<ScreencastFrame>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreencastFrame {
    pub jpeg_base64: String,
    pub captured_at: i64,
}

#[derive(Debug, Clone)]
struct RawRecord {
    target_id: String,
    tab_id: i64,
    page_id: u32,
    url: String,
    title: String,
    session_id: String,
    agent_id: String,
    slug: String,
    first_tool_at: i64,
    last_tool_at: i64,
    last_tool_name: String,
    tool_count: usize,
    recent_tools: VecDeque<ToolEvent>,
}

#[derive(Clone, Default)]
pub struct TabActivityService {
    records: Arc<Mutex<HashMap<String, RawRecord>>>,
}

pub struct RecordToolInput {
    pub target_id: TargetId,
    pub tab_id: i64,
    pub page_id: u32,
    pub session_id: String,
    pub url: String,
    pub title: String,
    pub agent_id: String,
    pub slug: String,
    pub tool_name: String,
}

impl TabActivityService {
    pub async fn record_tool(&self, input: RecordToolInput) {
        let now = now_ms();
        let target_key = input.target_id.into_inner();
        let mut records = self.records.lock().await;
        if let Some(existing) = records.get_mut(&target_key) {
            existing.tab_id = input.tab_id;
            existing.page_id = input.page_id;
            existing.url = input.url;
            existing.title = input.title;
            existing.session_id = input.session_id;
            existing.agent_id = input.agent_id;
            existing.slug = input.slug;
            existing.last_tool_at = now;
            existing.last_tool_name = input.tool_name.clone();
            existing.tool_count += 1;
            existing.recent_tools.push_back(ToolEvent {
                name: input.tool_name,
                at: now,
            });
            while existing.recent_tools.len() > RECENT_TOOLS_CAP {
                existing.recent_tools.pop_front();
            }
            return;
        }
        let mut recent_tools = VecDeque::new();
        recent_tools.push_back(ToolEvent {
            name: input.tool_name.clone(),
            at: now,
        });
        records.insert(
            target_key.clone(),
            RawRecord {
                target_id: target_key,
                tab_id: input.tab_id,
                page_id: input.page_id,
                url: input.url,
                title: input.title,
                session_id: input.session_id,
                agent_id: input.agent_id,
                slug: input.slug,
                first_tool_at: now,
                last_tool_at: now,
                last_tool_name: input.tool_name,
                tool_count: 1,
                recent_tools,
            },
        );
    }

    pub async fn snapshot(&self) -> Vec<TabActivityRecord> {
        let now = now_ms();
        let mut rows: Vec<_> = self
            .records
            .lock()
            .await
            .values()
            .map(|record| TabActivityRecord {
                target_id: record.target_id.clone(),
                tab_id: record.tab_id,
                page_id: record.page_id,
                url: record.url.clone(),
                title: record.title.clone(),
                session_id: record.session_id.clone(),
                agent_id: record.agent_id.clone(),
                slug: record.slug.clone(),
                first_tool_at: record.first_tool_at,
                last_tool_at: record.last_tool_at,
                last_tool_name: record.last_tool_name.clone(),
                tool_count: record.tool_count,
                recent_tools: record.recent_tools.iter().cloned().collect(),
                status: if now.saturating_sub(record.last_tool_at)
                    < i64::try_from(ACTIVE_WINDOW.as_millis()).unwrap_or(30_000)
                {
                    "active"
                } else {
                    "idle"
                },
            })
            .collect();
        rows.sort_by_key(|row| Reverse(row.last_tool_at));
        rows
    }
}

fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordToolInput, TabActivityService};
    use browseros_core::TargetId;

    #[tokio::test]
    async fn latest_session_and_tab_replace_the_target_association() {
        let service = TabActivityService::default();
        for (session_id, tab_id, tool_name) in [
            ("session-1", 101, "navigate"),
            ("session-2", 202, "snapshot"),
        ] {
            service
                .record_tool(RecordToolInput {
                    target_id: TargetId::from("target-1".to_string()),
                    tab_id,
                    page_id: 7,
                    session_id: session_id.to_string(),
                    url: "https://example.com".to_string(),
                    title: "Example".to_string(),
                    agent_id: session_id.to_string(),
                    slug: "codex".to_string(),
                    tool_name: tool_name.to_string(),
                })
                .await;
        }

        let records = service.snapshot().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "session-2");
        assert_eq!(records[0].tab_id, 202);
        assert_eq!(records[0].tool_count, 2);
    }
}
