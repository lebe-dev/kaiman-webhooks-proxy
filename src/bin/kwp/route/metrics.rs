use std::sync::Arc;

use axum::{
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
};

use crate::AppState;

pub async fn metrics_route(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.metrics_handle {
        Some(handle) => {
            let body = handle.render();
            (
                StatusCode::OK,
                [(
                    header::CONTENT_TYPE,
                    "text/plain; version=0.0.4; charset=utf-8",
                )],
                body,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
