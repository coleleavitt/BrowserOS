use super::wire::WireJson;
use crate::{
    AppState,
    error::AppResult,
    services::{recordings::RecordingEventInput, replays::ReplayMeta},
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::Value;

const MAX_RECORDING_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_SAFE_TAB_ID: i64 = 9_007_199_254_740_991;

#[derive(Debug, Serialize)]
pub(super) struct RecordingHealthResponse {
    ok: bool,
}

pub(super) async fn recordings_health() -> WireJson<RecordingHealthResponse> {
    WireJson(RecordingHealthResponse { ok: true })
}

#[derive(Debug, Serialize)]
struct RecordEventsOutcome {
    accepted: usize,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub(super) async fn post_recording_events(
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
        return WireJson(RecordEventsOutcome {
            accepted: 0,
            ok: false,
            reason: Some("unknown tab"),
        })
        .into_response();
    };
    let events = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(parse_recording_event)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return WireJson(RecordEventsOutcome {
            accepted: 0,
            ok: true,
            reason: None,
        })
        .into_response();
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
                WireJson(RecordEventsOutcome {
                    accepted: 0,
                    ok: false,
                    reason: Some("append failed"),
                }),
            )
                .into_response();
        }
    };
    if !appended {
        return WireJson(RecordEventsOutcome {
            accepted: 0,
            ok: true,
            reason: None,
        })
        .into_response();
    }
    WireJson(RecordEventsOutcome {
        accepted: events.len(),
        ok: true,
        reason: None,
    })
    .into_response()
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

#[derive(Debug, Serialize)]
struct ReplayUnavailableResponse {
    ok: bool,
    reason: &'static str,
}

pub(super) async fn get(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<Response> {
    let events = state.replays.read_session(&session_id).await?;
    if events.is_empty() {
        return Ok((
            StatusCode::NOT_FOUND,
            WireJson(ReplayUnavailableResponse {
                ok: false,
                reason: "no replay for this session",
            }),
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

pub(super) async fn meta(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> AppResult<WireJson<ReplayMeta>> {
    Ok(WireJson(state.replays.meta(&session_id).await?))
}
