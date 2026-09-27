use axum::{Json, extract::State, response::IntoResponse};

use super::ApiState;
use crate::{
    SessionPreferences,
    api::{EmptyJsonResponse, Result},
    limits::LimitsConfig,
};

pub async fn h_update_session_ratelimits(
    State(state): State<ApiState>,
    Json(limits): Json<LimitsConfig>,
) -> Result<impl IntoResponse> {
    state
        .api
        .session()
        .set_ratelimits_persistent(limits)
        .await
        .map_err(|e| crate::ApiError::from((http::StatusCode::BAD_REQUEST, e)))?;
    Ok(Json(EmptyJsonResponse {}))
}

pub async fn h_get_session_ratelimits(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    let config = state.api.session().ratelimits.get_config();
    Ok(Json(config))
}

pub async fn h_get_session_preferences(
    State(state): State<ApiState>,
) -> Result<impl IntoResponse> {
    Ok(Json(state.api.api_get_preferences()))
}

pub async fn h_update_session_preferences(
    State(state): State<ApiState>,
    Json(prefs): Json<SessionPreferences>,
) -> Result<impl IntoResponse> {
    state.api.api_set_preferences(prefs).await?;
    Ok(Json(EmptyJsonResponse {}))
}
