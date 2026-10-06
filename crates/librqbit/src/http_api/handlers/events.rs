use axum::{
    extract::{Query, State},
    response::IntoResponse,
};
use serde::Deserialize;

use super::ApiState;
use crate::{ApiError, api::Result, event_log::EventQuery};

/// GET /events?kind=&torrent_id=&info_hash=&severity=&since=&since_seq=&before_seq=&limit=
pub async fn h_events(
    State(state): State<ApiState>,
    Query(q): Query<EventQuery>,
) -> Result<impl IntoResponse> {
    let events = state.api.session().events.clone();
    let page = crate::event_log::off_runtime(move || events.query(&q))
        .await
        .map_err(ApiError::from)?;
    Ok(axum::Json(page))
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    #[serde(default)]
    since_seq: Option<u64>,
}

/// GET /events/summary?since_seq= : repair counters, latest seq, unseen counts.
pub async fn h_events_summary(
    State(state): State<ApiState>,
    Query(q): Query<SummaryQuery>,
) -> Result<impl IntoResponse> {
    Ok(axum::Json(state.api.session().events.summary(q.since_seq)))
}

/// POST /events/counters/reset
pub async fn h_events_counters_reset(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    let events = state.api.session().events.clone();
    let c = crate::event_log::off_runtime(move || events.reset_counters())
        .await
        .map_err(ApiError::from)?;
    Ok(axum::Json(c))
}
