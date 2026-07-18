use super::wire::WireJson;
use crate::{
    AppState,
    error::AppResult,
    services::harness::{ConnectionState, Harness},
};
use axum::extract::{Path, State};
use serde::Serialize;
use std::str::FromStr;

#[derive(Debug, Serialize)]
pub(super) struct ConnectionsResponse {
    connections: Vec<ConnectionState>,
}

pub(super) async fn list(
    State(state): State<AppState>,
) -> AppResult<WireJson<ConnectionsResponse>> {
    Ok(WireJson(ConnectionsResponse {
        connections: state.harness.list_browseros_connections().await?,
    }))
}

pub(super) async fn connect(
    State(state): State<AppState>,
    Path(harness): Path<String>,
) -> AppResult<WireJson<ConnectionState>> {
    let harness = Harness::from_str(&harness)?;
    let result = state
        .harness
        .connect_browseros(harness, &state.config.public_mcp_url())
        .await?;
    Ok(WireJson(result))
}

pub(super) async fn disconnect(
    State(state): State<AppState>,
    Path(harness): Path<String>,
) -> AppResult<WireJson<ConnectionState>> {
    let harness = Harness::from_str(&harness)?;
    let result = state.harness.disconnect_browseros(harness).await?;
    Ok(WireJson(result))
}
