use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::Value;

use super::auth::ApiAccess;
use crate::AppState;
use crate::middleware::request_id::RequestId;
use kwp_lib::domain::webhook::model::WebhookChannel;

#[derive(Serialize)]
pub struct WebhookListItemDto {
    pub id: i64,
    pub headers: HashMap<String, String>,
    pub payload: Value,
    pub received_at: i64,
}

pub async fn list_webhooks_route(
    State(state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
    Path(channel_name): Path<String>,
    access: ApiAccess,
) -> impl IntoResponse {
    if let Err(rejection) = access.channel(&state, &request_id, &channel_name) {
        return rejection.into_response();
    }

    let channel = WebhookChannel::new(channel_name.clone());

    match state.webhook_service.list_webhooks(&channel).await {
        Ok(webhooks) => {
            log::debug!(
                "[req:{}] listed {} webhooks for channel: {}",
                request_id,
                webhooks.len(),
                channel_name
            );
            let dtos: Vec<WebhookListItemDto> = webhooks
                .into_iter()
                .filter_map(|w| {
                    Some(WebhookListItemDto {
                        id: w.id?,
                        headers: w.headers,
                        payload: serde_json::from_slice(&w.payload)
                            .unwrap_or(serde_json::Value::Null),
                        received_at: w.received_at,
                    })
                })
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) if e.is_busy() => {
            log::warn!(
                "[req:{}] storage busy listing webhooks for {}: {}",
                request_id,
                channel_name,
                e
            );
            crate::route::storage_busy_response()
        }
        Err(e) => {
            log::error!(
                "[req:{}] failed to list webhooks for channel {}: {}",
                request_id,
                channel_name,
                e
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response()
        }
    }
}
