use crate::{
    AppState,
    error::{AppError, AppResult},
    ids::ConvoId,
    mcp::streamable_http_service,
    services::{
        audit::{ListDispatchesQuery, ListTasksQuery, TaskStatus},
        harness::Harness,
        recordings::RecordingEventInput,
        tab_activity::EnrichedTabRecord,
    },
    tabs::hex_for_slug,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, str::FromStr, time::Instant};
use tracing::{Instrument, info_span};
use ulid::Ulid;

const MAX_RECORDING_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_SAFE_TAB_ID: i64 = 9_007_199_254_740_991;

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/system/health", get(system_health))
        .route("/system/shutdown", post(system_shutdown))
        .route("/system/version", get(system_version))
        .route("/system/url", get(system_url))
        .route(
            "/system/telemetry",
            get(system_telemetry).post(system_telemetry_consent),
        )
        .route("/agents/{agent_id}/cancel", post(agents_cancel))
        .route("/tabs/activity", get(tabs_activity))
        .route("/connections", get(connections_list))
        .route("/connections/{harness}/connect", post(connections_connect))
        .route(
            "/connections/{harness}/disconnect",
            post(connections_disconnect),
        )
        .route("/audit/dispatches", get(audit_dispatches))
        .route("/audit/tasks", get(audit_tasks))
        .route("/audit/tasks/{session_id}", get(audit_task_detail))
        .route("/audit/screenshot/{dispatch_id}", get(audit_screenshot))
        .route("/recordings/health", get(recordings_health))
        .route(
            "/recordings/tabs/{tab_id}/events",
            post(recordings_post_events),
        )
        .route("/audit/replays/{session_id}", get(audit_replay_get))
        .route("/audit/replays/{session_id}/meta", get(audit_replay_meta))
        .nest_service(
            "/mcp",
            Router::new()
                .fallback_service(streamable_http_service(state))
                .layer(middleware::from_fn(mcp_request_hygiene)),
        )
        .fallback(route_fallback)
}

/// Enforces the header conventions native MCP clients follow (parity with
/// the TS server's mcp-request-hygiene middleware). A browser-page fetch
/// against the loopback MCP endpoint always carries `origin` or
/// `sec-fetch-site`; native MCP clients never do.
async fn mcp_request_hygiene(req: Request, next: Next) -> Response {
    // The nested /mcp service shadows the router's `/{*path}` preflight
    // route, and the TS server's cors layer answers OPTIONS before its
    // hygiene runs — mirror both so preflight stays 204 here too.
    if *req.method() == Method::OPTIONS {
        return StatusCode::NO_CONTENT.into_response();
    }
    let headers = req.headers();
    if headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return AppError::forbidden("unsupported request").into_response();
    }
    let needs_json = match *req.method() {
        Method::POST | Method::PUT | Method::PATCH => true,
        // rmcp's DELETE /mcp session teardown carries no body and no
        // content-type; the TS server never sees that shape (its clients
        // always send application/json), so exempt only that case.
        Method::DELETE => headers.contains_key(header::CONTENT_TYPE),
        _ => false,
    };
    if needs_json {
        let is_json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("application/json"));
        if !is_json {
            return AppError::unsupported_media_type("unsupported content type").into_response();
        }
    }
    next.run(req).await
}

pub async fn request_context(req: Request, next: Next) -> Response {
    let request_id = Ulid::new().to_string();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let span = info_span!("http_request", %request_id, %method, %path);
    async move {
        let start = Instant::now();
        let mut response = next.run(req).await;
        // One structured line per failed request; sub-400 traffic stays
        // unlogged on purpose (claw-app polls several endpoints).
        let status = response.status().as_u16();
        if status >= 400 {
            let duration_ms = start.elapsed().as_millis() as u64;
            if status >= 500 {
                tracing::error!(%method, %path, status, duration_ms, "request failed");
            } else {
                tracing::warn!(%method, %path, status, duration_ms, "request failed");
            }
        }
        let headers = response.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET,POST,PATCH,DELETE,OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(
                "accept,content-type,authorization,mcp-session-id,mcp-protocol-version,last-event-id,x-recording-batch-id",
            ),
        );
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            headers.insert("x-request-id", value);
        }
        response
    }
    .instrument(span)
    .await
}

async fn route_fallback(request: Request) -> StatusCode {
    if *request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn system_health(State(state): State<AppState>) -> Json<Value> {
    let cdp = state.browser.state();
    Json(json!({
        "status": "ok",
        "cdp": cdp,
        "sessions": {
            "count": state.sessions.count().await
        }
    }))
}

async fn system_shutdown(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let drained = state.sessions.shutdown().await?;
    state.audit.drain_claim_writes().await;
    state.recordings.close().await;
    state.screencast.stop();
    state.browser.stop();
    if let Some(tx) = state.shutdown.lock().await.take() {
        let _ = tx.send(());
    }
    Ok(Json(json!({ "status": "ok", "drainedSessions": drained })))
}

async fn system_version() -> Json<Value> {
    Json(json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn system_url(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "url": state.config.local_server_url() }))
}

async fn system_telemetry(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.telemetry.get_state().await))
}

#[derive(Debug, Deserialize)]
struct TelemetryConsent {
    consent: bool,
}

async fn system_telemetry_consent(
    State(state): State<AppState>,
    Json(input): Json<TelemetryConsent>,
) -> Json<Value> {
    Json(json!(state.telemetry.set_consent(input.consent).await))
}

async fn agents_cancel(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let cancelled = state
        .sessions
        .cancel_by_convo(&ConvoId::from(agent_id.as_str()))
        .await;
    if cancelled == 0 {
        // claw-app parses this 404 body as a CancelAgentResult (idle state,
        // not a failure), so it must keep the TS shape, not `{"error":...}`.
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "cancelled": 0,
                "reason": "no active dispatches for this agent",
            })),
        )
            .into_response();
    }
    tracing::info!(%agent_id, cancelled, "cancelled in-flight dispatches for agent");
    Json(json!({ "ok": true, "cancelled": cancelled })).into_response()
}

async fn tabs_activity(State(state): State<AppState>) -> AppResult<Json<Value>> {
    state.screencast.note_read();
    let profiles = state.agents.list_profiles().await?;
    let live_sessions = state.sessions.snapshot().await;
    let sessions_by_agent_id = live_sessions
        .iter()
        .map(|session| (session.convo_id().as_str(), session))
        .collect::<HashMap<_, _>>();
    let tabs = state.tab_activity.snapshot().await;
    let mut enriched = Vec::with_capacity(tabs.len());
    for record in tabs {
        let session = sessions_by_agent_id.get(record.agent_id.as_str()).copied();
        let profile = session
            .and_then(|session| session.agent().profile_id())
            .and_then(|profile_id| {
                profiles
                    .iter()
                    .find(|profile| profile.id == profile_id.as_str())
            });
        enriched.push(EnrichedTabRecord {
            agent_label: profile
                .map(|profile| profile.name.clone())
                .or_else(|| session.map(|session| session.agent().label().to_string()))
                .unwrap_or_else(|| record.slug.clone()),
            harness: profile.map(|profile| profile.harness.to_string()),
            color: Some(hex_for_slug(&record.slug).to_string()),
            screencast: state.screencast.frame_for(record.page_id).await,
            record,
        });
    }
    Ok(Json(json!({ "tabs": enriched })))
}

async fn connections_list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "connections": state.harness.list_browseros_connections().await?
    })))
}

async fn connections_connect(
    State(state): State<AppState>,
    Path(harness): Path<String>,
) -> AppResult<Json<Value>> {
    let harness = Harness::from_str(&harness)?;
    let result = state
        .harness
        .connect_browseros(harness, &state.config.public_mcp_url())
        .await?;
    Ok(Json(serde_json::to_value(result)?))
}

async fn connections_disconnect(
    State(state): State<AppState>,
    Path(harness): Path<String>,
) -> AppResult<Json<Value>> {
    let harness = Harness::from_str(&harness)?;
    let result = state.harness.disconnect_browseros(harness).await?;
    Ok(Json(serde_json::to_value(result)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchesQuery {
    agent_id: Option<String>,
    session_id: Option<String>,
    cursor: Option<i64>,
    limit: Option<i64>,
}

async fn audit_dispatches(
    State(state): State<AppState>,
    Query(query): Query<DispatchesQuery>,
) -> AppResult<Json<Value>> {
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
    Ok(Json(serde_json::to_value(result)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TasksQuery {
    agent_id: Option<String>,
    status: Option<TaskStatus>,
    site: Option<String>,
    search: Option<String>,
    since: Option<i64>,
    cursor: Option<i64>,
    limit: Option<i64>,
}

async fn audit_tasks(
    State(state): State<AppState>,
    Query(query): Query<TasksQuery>,
) -> AppResult<Json<Value>> {
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
    Ok(Json(serde_json::to_value(result)?))
}

async fn audit_task_detail(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<Json<Value>> {
    let task = state
        .audit
        .get_task(&session_id)
        .await?
        .ok_or_else(|| AppError::not_found("not found"))?;
    Ok(Json(serde_json::to_value(task)?))
}

async fn audit_screenshot(
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

async fn recordings_health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn recordings_post_events(
    State(state): State<AppState>,
    Path(tab_id): Path<String>,
    request: Request,
) -> Response {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_RECORDING_BODY_BYTES)
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let batch_id = request
        .headers()
        .get("x-recording-batch-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = match to_bytes(request.into_body(), MAX_RECORDING_BODY_BYTES + 1).await {
        Ok(bytes) if bytes.len() <= MAX_RECORDING_BODY_BYTES => bytes,
        Ok(_) | Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let tab_id = tab_id
        .chars()
        .all(|ch| ch.is_ascii_digit())
        .then(|| tab_id.parse::<i64>().ok())
        .flatten()
        .filter(|tab_id| *tab_id <= MAX_SAFE_TAB_ID);
    let target_id = match tab_id {
        Some(tab_id) => {
            let browser = state.browser.state();
            state
                .tab_targets
                .resolve(tab_id, state.browser.session().await, browser.epoch)
                .await
        }
        None => None,
    };
    let (Some(tab_id), Some(target_id)) = (tab_id, target_id) else {
        return Json(json!({
            "ok": false,
            "reason": "unknown tab",
            "accepted": 0,
        }))
        .into_response();
    };
    let events = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(parse_recording_event)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return Json(json!({ "ok": true, "accepted": 0 })).into_response();
    }
    let appended = match state
        .recordings
        .append_batch_with_id(&target_id, tab_id, &events, batch_id.as_deref())
        .await
    {
        Ok(appended) => appended,
        Err(error) => {
            tracing::warn!(tab_id, target_id, error = %error, "recording batch append failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "reason": "append failed",
                    "accepted": 0,
                })),
            )
                .into_response();
        }
    };
    if !appended {
        return Json(json!({ "ok": true, "accepted": 0 })).into_response();
    }
    Json(json!({ "ok": true, "accepted": events.len() })).into_response()
}

fn parse_recording_event(line: &str) -> Option<RecordingEventInput> {
    if line.trim().is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(line).ok()?;
    let event = value.as_object()?;
    Some(RecordingEventInput {
        ts: event.get("ts")?.as_i64()?,
        event_type: event.get("type").cloned(),
        data: event.get("data").cloned(),
    })
}

async fn audit_replay_get(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<Response> {
    let events = state.replays.read_session(&session_id).await?;
    if events.is_empty() {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "reason": "no replay for this session",
            })),
        )
            .into_response());
    }
    let mut body = String::new();
    for event in events {
        body.push_str(&serde_json::to_string(&event)?);
        body.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

async fn audit_replay_meta(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<Json<Value>> {
    Ok(Json(serde_json::to_value(
        state.replays.meta(&session_id).await?,
    )?))
}

fn validate_limit(limit: Option<i64>, cap: i64) -> AppResult<()> {
    if let Some(limit) = limit
        && (limit <= 0 || limit > cap)
    {
        return Err(AppError::bad_request("limit out of range"));
    }
    Ok(())
}
