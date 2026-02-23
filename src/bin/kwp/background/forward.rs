use std::time::Duration;

use kwp_lib::domain::config::model::ForwardConfig;
use kwp_lib::domain::webhook::model::WebhookChannel;
use kwp_lib::domain::webhook::ports::WebhookRepository;

/// Hop-by-hop headers that should not be forwarded.
const HOP_BY_HOP: &[&str] = &["host", "content-length", "transfer-encoding", "connection"];

pub async fn run_forwarder<R: WebhookRepository>(
    channel: WebhookChannel,
    forward_cfg: ForwardConfig,
    repo: R,
    http: reqwest::Client,
) {
    let interval = Duration::from_secs(forward_cfg.interval_seconds);

    loop {
        match repo.peek_oldest_by_channel(&channel).await {
            Err(e) => {
                log::error!("[forwarder:{}] peek failed: {}", channel.as_str(), e);
                tokio::time::sleep(interval).await;
            }
            Ok(None) => {
                log::trace!(
                    "[forwarder:{}] no pending webhooks, sleeping",
                    channel.as_str()
                );
                tokio::time::sleep(interval).await;
            }
            Ok(Some(webhook)) => {
                let id = match webhook.id {
                    Some(id) => id,
                    None => {
                        log::error!("[forwarder:{}] webhook has no id", channel.as_str());
                        tokio::time::sleep(interval).await;
                        continue;
                    }
                };

                log::debug!(
                    "[forwarder:{}] forwarding webhook id={} to {}",
                    channel.as_str(),
                    id,
                    forward_cfg.url
                );

                let timeout = Duration::from_secs(forward_cfg.timeout_seconds);
                let mut request = http
                    .post(&forward_cfg.url)
                    .timeout(timeout)
                    .json(&webhook.payload);

                for (key, value) in &webhook.headers {
                    if HOP_BY_HOP.contains(&key.as_str()) {
                        continue;
                    }
                    request = request.header(key, value);
                }

                match request.send().await {
                    Err(e) => {
                        log::warn!("[forwarder:{}] request failed: {}", channel.as_str(), e);
                        tokio::time::sleep(interval).await;
                    }
                    Ok(resp) => {
                        if resp.status().as_u16() == forward_cfg.expected_status {
                            log::info!(
                                "[forwarder:{}] successfully forwarded webhook id={} → {}",
                                channel.as_str(),
                                id,
                                forward_cfg.url
                            );
                            if let Err(e) = repo.delete_by_id(id).await {
                                log::error!(
                                    "[forwarder:{}] delete_by_id({}) failed: {}",
                                    channel.as_str(),
                                    id,
                                    e
                                );
                            }
                        } else {
                            log::warn!(
                                "[forwarder:{}] unexpected status {} from {}",
                                channel.as_str(),
                                resp.status(),
                                forward_cfg.url
                            );
                            tokio::time::sleep(interval).await;
                        }
                    }
                }
            }
        }
    }
}
