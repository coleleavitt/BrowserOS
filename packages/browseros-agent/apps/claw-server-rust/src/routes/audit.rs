use super::wire::WireJson;
use crate::{
    AppState,
    error::{AppError, AppResult},
    services::audit::{
        ListDispatchesQuery, ListDispatchesResult, ListTasksQuery, ListTasksResult, TaskDetail,
        TaskStatus,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::IntoResponse,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DispatchesQuery {
    agent_id: Option<String>,
    session_id: Option<String>,
    cursor: Option<i64>,
    limit: Option<i64>,
}

pub(super) async fn dispatches(
    State(state): State<AppState>,
    Query(query): Query<DispatchesQuery>,
) -> AppResult<WireJson<ListDispatchesResult>> {
    validate_limit(query.limit, 500)?;
    let result = state
        .audit
        .list_dispatches(ListDispatchesQuery {
            agent_id: query.agent_id,
            session_id: query.session_id,
            cursor: query.cursor,
            limit: query.limit,
        })
        .await?;
    Ok(WireJson(result))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TasksQuery {
    agent_id: Option<String>,
    status: Option<TaskStatus>,
    site: Option<String>,
    search: Option<String>,
    since: Option<i64>,
    cursor: Option<i64>,
    limit: Option<i64>,
}

pub(super) async fn tasks(
    State(state): State<AppState>,
    Query(query): Query<TasksQuery>,
) -> AppResult<WireJson<ListTasksResult>> {
    validate_limit(query.limit, 100)?;
    let result = state
        .audit
        .list_tasks(ListTasksQuery {
            agent_id: query.agent_id,
            status: query.status,
            site: query.site,
            search: query.search,
            since: query.since,
            cursor: query.cursor,
            limit: query.limit,
        })
        .await?;
    Ok(WireJson(result))
}

pub(super) async fn task_detail(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<WireJson<TaskDetail>> {
    let task = state
        .audit
        .get_task(&session_id)
        .await?
        .ok_or_else(|| AppError::not_found("not found"))?;
    Ok(WireJson(task))
}

pub(super) async fn screenshot(
    State(state): State<AppState>,
    Path(dispatch_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let bytes = state.screenshots.read(&dispatch_id).await?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400, immutable"),
            ),
        ],
        bytes,
    ))
}

fn validate_limit(limit: Option<i64>, cap: i64) -> AppResult<()> {
    if let Some(limit) = limit
        && (limit <= 0 || limit > cap)
    {
        return Err(AppError::bad_request("limit out of range"));
    }
    Ok(())
}
