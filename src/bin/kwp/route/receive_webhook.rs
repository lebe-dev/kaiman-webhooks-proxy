use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use subtle::ConstantTimeEq;

use crate::AppState;
use kwp_lib::domain::webhook::model::WebhookChannel;

pub async fn receive_webhook_route(
    State(state): State<Arc<AppState>>,
    Path(channel_name): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    log::info!("received webhook for channel: {}", channel_name);

    let channel_config = match state.config.find_channel_by_name(&channel_name) {
        Some(c) => c,
        None => {
            log::warn!("webhook received for unknown channel: {}", channel_name);
            return (StatusCode::NOT_FOUND, "Channel not found").into_response();
        }
    };

    if let (Some(secret), Some(header_name)) = (
        &channel_config.webhook_secret,
        &channel_config.secret_header,
    ) {
        log::debug!("verifying webhook secret for channel: {}", channel_name);
        let provided = headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok());

        match provided {
            Some(token) if token.as_bytes().ct_eq(secret.as_bytes()).into() => {
                log::debug!("webhook secret verified for channel: {}", channel_name);
            }
            _ => {
                log::warn!("invalid webhook secret for channel: {}", channel_name);
                return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
            }
        }
    }

    log::debug!("filtering headers for channel: {}", channel_name);
    let hop_by_hop = [
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        "content-type",
    ];
    let forwarded_headers: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let key = k.as_str().to_lowercase();
            if hop_by_hop.contains(&key.as_str()) {
                return None;
            }
            v.to_str().ok().map(|val| (key, val.to_string()))
        })
        .collect();

    let channel = WebhookChannel::new(channel_name.clone());

    match state
        .webhook_service
        .receive_webhook(channel, forwarded_headers, payload)
        .await
    {
        Ok(()) => {
            log::info!(
                "webhook successfully processed and stored for channel: {}",
                channel_name
            );
            (StatusCode::OK, "OK").into_response()
        }
        Err(e) => {
            log::error!(
                "failed to store webhook for channel {}: {}",
                channel_name,
                e
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "Error").into_response()
        }
    }
}
