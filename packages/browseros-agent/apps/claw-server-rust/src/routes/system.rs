//! Pre-contract `/system/*` diagnostics that remain outside the
//! canonical surface. `/system/health` and `/system/shutdown` belong to
//! the contract and live in `routes::api_v1`.

use super::wire::WireJson;
use crate::{AppState, telemetry::TelemetryState};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

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
