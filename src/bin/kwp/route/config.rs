use std::sync::Arc;

use axum::{Extension, Json, extract::State, response::IntoResponse};

use super::auth::ApiAccess;
use crate::AppState;
use crate::middleware::request_id::RequestId;
use kwp_lib::domain::config::model::AppConfigPublicDto;

pub async fn get_config_route(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    _access: ApiAccess,
) -> impl IntoResponse {
    // The UI fetches this on every page load and it changes nothing, so the success
    // path stays out of the INFO narrative.
    log::debug!("[req:{}] served public configuration", request_id);

    let dto = AppConfigPublicDto::from(&state.config);
    Json(dto)
}
