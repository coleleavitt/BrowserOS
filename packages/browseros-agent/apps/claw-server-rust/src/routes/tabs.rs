use super::wire::WireJson;
use crate::{
    AppState, error::AppResult, services::tab_activity::EnrichedTabRecord, tabs::hex_for_slug,
};
use axum::extract::State;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub(super) struct TabsActivityResponse {
    tabs: Vec<EnrichedTabRecord>,
}

pub(super) async fn activity(
    State(state): State<AppState>,
) -> AppResult<WireJson<TabsActivityResponse>> {
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
    Ok(WireJson(TabsActivityResponse { tabs: enriched }))
}
