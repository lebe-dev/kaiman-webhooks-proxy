use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::AppState;
use kwp_lib::domain::config::model::AppConfigPublicDto;

pub async fn get_config_route(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let bearer = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let bearer = match bearer {
        Some(b) => b,
        None => {
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
    };

    let authorized =
        state.config.find_channel_by_token(bearer).is_some() || state.config.is_ui_token(bearer);
    if !authorized {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let dto = AppConfigPublicDto::from(&state.config);
    (StatusCode::OK, Json(dto)).into_response()
}
