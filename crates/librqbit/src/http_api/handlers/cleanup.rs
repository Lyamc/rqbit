//! Orphaned-download cleanup. Scans are read-only GETs; apply / restore /
//! purge are POSTs (not available in read-only mode).

use axum::{
    extract::{Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use serde::Deserialize;

use super::ApiState;
use crate::{
    api::Result,
    api_error::WithStatus,
    orphan_cleanup::{ApplyRequest, PurgeRequest, RestoreRequest, ScanQuery},
};

/// GET /cleanup/roots
pub async fn h_cleanup_roots(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    Ok(axum::Json(state.api.session().cleanup_roots_response()))
}

/// GET /cleanup/scan?roots=/a,/b&min_age_minutes=60 : dry run, changes nothing.
pub async fn h_cleanup_scan(
    State(state): State<ApiState>,
    Query(q): Query<ScanQuery>,
) -> Result<impl IntoResponse> {
    let r = state
        .api
        .session()
        .cleanup_scan(q.roots, q.min_age_minutes, false)
        .await
        .with_status(StatusCode::BAD_REQUEST)?;
    Ok(axum::Json(r))
}

#[derive(Deserialize)]
pub struct ScanIdQuery {
    #[serde(default)]
    scan_id: Option<String>,
}

/// GET /cleanup/scan_result?scan_id= : a recent scan (default: the latest, e.g. scheduled).
pub async fn h_cleanup_scan_result(
    State(state): State<ApiState>,
    Query(q): Query<ScanIdQuery>,
) -> Result<impl IntoResponse> {
    let r = state
        .api
        .session()
        .cleanup_get_scan(q.scan_id.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no such scan"))
        .with_status(StatusCode::NOT_FOUND)?;
    Ok(axum::Json(r))
}

/// GET /cleanup/quarantine
pub async fn h_cleanup_quarantine(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    Ok(axum::Json(state.api.session().cleanup_quarantine()))
}

/// POST /cleanup/apply {scan_id, item_ids, action: quarantine|delete, confirm}
pub async fn h_cleanup_apply(
    State(state): State<ApiState>,
    axum::Json(req): axum::Json<ApplyRequest>,
) -> Result<impl IntoResponse> {
    let r = state
        .api
        .session()
        .cleanup_apply(req)
        .await
        .with_status(StatusCode::BAD_REQUEST)?;
    Ok(axum::Json(r))
}

/// POST /cleanup/restore {batch, items?}
pub async fn h_cleanup_restore(
    State(state): State<ApiState>,
    axum::Json(req): axum::Json<RestoreRequest>,
) -> Result<impl IntoResponse> {
    let r = state
        .api
        .session()
        .cleanup_restore(req)
        .await
        .with_status(StatusCode::BAD_REQUEST)?;
    Ok(axum::Json(serde_json::json!({ "results": r })))
}

/// POST /cleanup/purge {batch, confirm: true}
pub async fn h_cleanup_purge(
    State(state): State<ApiState>,
    axum::Json(req): axum::Json<PurgeRequest>,
) -> Result<impl IntoResponse> {
    let (n, bytes) = state
        .api
        .session()
        .cleanup_purge(req)
        .await
        .with_status(StatusCode::BAD_REQUEST)?;
    Ok(axum::Json(serde_json::json!({ "items": n, "bytes": bytes })))
}
