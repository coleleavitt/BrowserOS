use super::wire::WireJson;
use crate::{AppState, ids::ConvoId};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct CancelOutcome {
    cancelled: usize,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let cancelled = state
        .sessions
        .cancel_by_convo(&ConvoId::from(agent_id.as_str()))
        .await;
    if cancelled == 0 {
        // claw-app parses this 404 body as a CancelAgentResult (idle state,
        // not a failure), so it must keep the TS shape, not `{"error":...}`.
        return (
            StatusCode::NOT_FOUND,
            WireJson(CancelOutcome {
                cancelled: 0,
                ok: false,
                reason: Some("no active dispatches for this agent"),
            }),
        )
            .into_response();
    }
    tracing::info!(%agent_id, cancelled, "cancelled in-flight dispatches for agent");
    WireJson(CancelOutcome {
        cancelled,
        ok: true,
        reason: None,
    })
    .into_response()
}
