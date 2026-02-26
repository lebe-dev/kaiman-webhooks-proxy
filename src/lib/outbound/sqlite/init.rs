use std::str::FromStr;

use anyhow::Context;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

#[derive(Debug, Clone)]
pub struct Sqlite {
    pool: SqlitePool,
}

impl Sqlite {
    pub fn get_pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn new(path: &str) -> Result<Sqlite, anyhow::Error> {
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::from_str(path)
                .with_context(|| format!("invalid database path {}", path))?
                .pragma("foreign_keys", "ON")
                .pragma("journal_mode", "WAL"),
        )
        .await
        .with_context(|| format!("failed to open database at {}", path))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS webhooks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                channel     TEXT NOT NULL,
                headers     TEXT NOT NULL DEFAULT '{}',
                payload     BLOB NOT NULL,
                received_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_webhooks_channel ON webhooks(channel);
            CREATE INDEX IF NOT EXISTS idx_webhooks_received_at ON webhooks(received_at);
            "#,
        )
        .execute(&pool)
        .await?;

        Ok(Sqlite { pool })
    }
}
