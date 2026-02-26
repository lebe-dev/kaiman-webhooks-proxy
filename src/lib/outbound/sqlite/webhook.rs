use std::collections::HashMap;

use bytes::Bytes;

use crate::domain::webhook::model::{Webhook, WebhookChannel, WebhookRepositoryError};
use crate::domain::webhook::ports::WebhookRepository;

use super::init::Sqlite;
use sqlx::Row;

impl WebhookRepository for Sqlite {
    async fn insert(&self, webhook: &Webhook) -> Result<(), WebhookRepositoryError> {
        let headers_json =
            serde_json::to_string(&webhook.headers).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO webhooks (channel, headers, payload, received_at) VALUES (?, ?, ?, ?)",
        )
        .bind(webhook.channel.as_str())
        .bind(headers_json)
        .bind(webhook.payload.as_ref())
        .bind(webhook.received_at)
        .execute(self.get_pool())
        .await
        .map_err(|e| WebhookRepositoryError::Other(e.into()))?;

        Ok(())
    }

    async fn read_and_delete_by_channel(
        &self,
        channel: &WebhookChannel,
        limit: i64,
    ) -> Result<Vec<Webhook>, WebhookRepositoryError> {
        let rows = sqlx::query(
            "DELETE FROM webhooks WHERE id IN (
                SELECT id FROM webhooks WHERE channel = ?
                ORDER BY received_at ASC LIMIT ?
            ) RETURNING id, channel, headers, payload, received_at",
        )
        .bind(channel.as_str())
        .bind(limit)
        .fetch_all(self.get_pool())
        .await
        .map_err(|e| WebhookRepositoryError::Other(e.into()))?;

        let webhooks = rows
            .into_iter()
            .filter_map(|row| {
                let id: i64 = row.try_get("id").ok()?;
                let channel: String = row.try_get("channel").ok()?;
                let headers_str: String = row.try_get("headers").ok()?;
                let payload: Vec<u8> = row.try_get("payload").ok()?;
                let received_at: i64 = row.try_get("received_at").ok()?;

                let headers: HashMap<String, String> =
                    serde_json::from_str(&headers_str).unwrap_or_default();

                Some(Webhook {
                    id: Some(id),
                    channel: WebhookChannel::new(channel),
                    headers,
                    payload: Bytes::from(payload),
                    received_at,
                })
            })
            .collect();

        Ok(webhooks)
    }

    async fn peek_oldest_by_channel(
        &self,
        channel: &WebhookChannel,
    ) -> Result<Option<Webhook>, WebhookRepositoryError> {
        let row = sqlx::query(
            "SELECT id, channel, headers, payload, received_at FROM webhooks
             WHERE channel = ? ORDER BY received_at ASC LIMIT 1",
        )
        .bind(channel.as_str())
        .fetch_optional(self.get_pool())
        .await
        .map_err(|e| WebhookRepositoryError::Other(e.into()))?;

        let webhook = row.and_then(|row| {
            let id: i64 = row.try_get("id").ok()?;
            let channel: String = row.try_get("channel").ok()?;
            let headers_str: String = row.try_get("headers").ok()?;
            let payload: Vec<u8> = row.try_get("payload").ok()?;
            let received_at: i64 = row.try_get("received_at").ok()?;

            let headers: HashMap<String, String> =
                serde_json::from_str(&headers_str).unwrap_or_default();

            Some(Webhook {
                id: Some(id),
                channel: WebhookChannel::new(channel),
                headers,
                payload: Bytes::from(payload),
                received_at,
            })
        });

        Ok(webhook)
    }

    async fn delete_by_id(&self, id: i64) -> Result<(), WebhookRepositoryError> {
        sqlx::query("DELETE FROM webhooks WHERE id = ?")
            .bind(id)
            .execute(self.get_pool())
            .await
            .map_err(|e| WebhookRepositoryError::Other(e.into()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::webhook::ports::WebhookRepository;

    async fn get_in_memory_db() -> Sqlite {
        Sqlite::new("sqlite::memory:").await.unwrap()
    }

    fn make_webhook(channel: &str, payload: &[u8], received_at: i64) -> Webhook {
        Webhook::new(
            WebhookChannel::new(channel),
            HashMap::new(),
            Bytes::copy_from_slice(payload),
            received_at,
        )
    }

    #[tokio::test]
    async fn test_insert_webhook() {
        let db = get_in_memory_db().await;

        let webhook = make_webhook("demo", b"{\"event\":\"push\"}", 1000);

        let result = db.insert(&webhook).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_and_peek_with_headers() {
        let db = get_in_memory_db().await;

        let mut headers = HashMap::new();
        headers.insert("x-custom-header".to_string(), "value123".to_string());

        let webhook = Webhook::new(
            WebhookChannel::new("demo"),
            headers.clone(),
            Bytes::from_static(b"{\"event\":\"push\"}"),
            1000,
        );
        db.insert(&webhook).await.unwrap();

        let peeked = db
            .peek_oldest_by_channel(&WebhookChannel::new("demo"))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(peeked.headers, headers);
        assert_eq!(peeked.payload, &b"{\"event\":\"push\"}"[..]);
    }

    #[tokio::test]
    async fn test_peek_oldest_fifo() {
        let db = get_in_memory_db().await;

        for i in 1i64..=3 {
            db.insert(&make_webhook(
                "demo",
                format!("{{\"seq\":{i}}}").as_bytes(),
                1000 + i,
            ))
            .await
            .unwrap();
        }

        let peeked = db
            .peek_oldest_by_channel(&WebhookChannel::new("demo"))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(peeked.payload, &b"{\"seq\":1}"[..]);
    }

    #[tokio::test]
    async fn test_peek_empty() {
        let db = get_in_memory_db().await;

        let result = db
            .peek_oldest_by_channel(&WebhookChannel::new("demo"))
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_by_id() {
        let db = get_in_memory_db().await;

        db.insert(&make_webhook("demo", b"{\"seq\":1}", 1000))
            .await
            .unwrap();

        let peeked = db
            .peek_oldest_by_channel(&WebhookChannel::new("demo"))
            .await
            .unwrap()
            .unwrap();

        let id = peeked.id.unwrap();
        db.delete_by_id(id).await.unwrap();

        let after = db
            .peek_oldest_by_channel(&WebhookChannel::new("demo"))
            .await
            .unwrap();

        assert!(after.is_none());
    }

    #[tokio::test]
    async fn test_read_and_delete_by_channel() {
        let db = get_in_memory_db().await;

        for i in 1i64..=3 {
            db.insert(&make_webhook(
                "demo",
                format!("{{\"event\":{i}}}").as_bytes(),
                1000 + i,
            ))
            .await
            .unwrap();
        }

        let webhooks = db
            .read_and_delete_by_channel(&WebhookChannel::new("demo"), 10)
            .await
            .unwrap();
        assert_eq!(webhooks.len(), 3);

        // Verify pop semantics — second read returns empty
        let webhooks2 = db
            .read_and_delete_by_channel(&WebhookChannel::new("demo"), 10)
            .await
            .unwrap();
        assert_eq!(webhooks2.len(), 0);
    }

    #[tokio::test]
    async fn test_cross_channel_isolation() {
        let db = get_in_memory_db().await;

        db.insert(&make_webhook("a", b"{\"ch\":\"a\"}", 1000))
            .await
            .unwrap();
        db.insert(&make_webhook("b", b"{\"ch\":\"b\"}", 1000))
            .await
            .unwrap();

        let a = db
            .read_and_delete_by_channel(&WebhookChannel::new("a"), 10)
            .await
            .unwrap();
        assert_eq!(a.len(), 1);

        // Channel b still intact
        let b = db
            .read_and_delete_by_channel(&WebhookChannel::new("b"), 10)
            .await
            .unwrap();
        assert_eq!(b.len(), 1);
    }
}
