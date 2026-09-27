use axum::{Json, extract::State, response::IntoResponse};

use super::ApiState;
use crate::{
    AdminConfigUpdate,
    api::{AdminStatusResponse, EmptyJsonResponse, Result},
};

pub async fn h_admin_status(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    Ok(Json(state.api.api_admin_status(None)))
}

pub async fn h_admin_reload_preferences(
    State(state): State<ApiState>,
) -> Result<impl IntoResponse> {
    let prefs = state.api.api_reload_preferences().await?;
    Ok(Json(prefs))
}

pub async fn h_admin_update_config(
    State(state): State<ApiState>,
    Json(patch): Json<AdminConfigUpdate>,
) -> Result<impl IntoResponse> {
    let view = state.api.api_update_admin(patch).await?;
    Ok(Json(view))
}

pub async fn h_admin_restart(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    state.api.api_request_restart()?;
    Ok(Json(EmptyJsonResponse {}))
}

// silence unused import warning in some feature sets
#[allow(dead_code)]
fn _types(_: AdminStatusResponse) {}
