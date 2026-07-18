use super::wire::WireJson;
use crate::{
    AppState,
    error::AppResult,
    services::{browser::BrowserConnectionState, telemetry::TelemetryState},
};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HealthResponse {
    cdp: BrowserConnectionState,
    sessions: SessionCountResponse,
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct SessionCountResponse {
    count: usize,
}

pub(super) async fn health(State(state): State<AppState>) -> WireJson<HealthResponse> {
    WireJson(HealthResponse {
        cdp: state.browser.state(),
        sessions: SessionCountResponse {
            count: state.sessions.count().await,
        },
        status: "ok",
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShutdownResponse {
    drained_sessions: usize,
    status: &'static str,
}

pub(super) async fn shutdown(
    State(state): State<AppState>,
) -> AppResult<WireJson<ShutdownResponse>> {
    let drained = state.sessions.shutdown().await?;
    state.audit.drain_claim_writes().await;
    state.recordings.close().await;
    state.screencast.stop();
    state.browser.stop();
    if let Some(tx) = state.shutdown.lock().await.take() {
        let _ = tx.send(());
    }
    Ok(WireJson(ShutdownResponse {
        drained_sessions: drained,
        status: "ok",
    }))
}

#[derive(Debug, Serialize)]
pub(super) struct VersionResponse {
    name: &'static str,
    version: &'static str,
}

pub(super) async fn version() -> WireJson<VersionResponse> {
    WireJson(VersionResponse {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Serialize)]
pub(super) struct UrlResponse {
    url: String,
}

pub(super) async fn url(State(state): State<AppState>) -> WireJson<UrlResponse> {
    WireJson(UrlResponse {
        url: state.config.local_server_url(),
    })
}

pub(super) async fn telemetry(State(state): State<AppState>) -> WireJson<TelemetryState> {
    WireJson(state.telemetry.get_state().await)
}

#[derive(Debug, Deserialize)]
pub(super) struct TelemetryConsent {
    consent: bool,
}

pub(super) async fn telemetry_consent(
    State(state): State<AppState>,
    Json(input): Json<TelemetryConsent>,
) -> WireJson<TelemetryState> {
    WireJson(state.telemetry.set_consent(input.consent).await)
}
