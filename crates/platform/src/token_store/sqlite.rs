use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tokio::sync::OnceCell;

use brain3_core::domain::errors::TokenStoreError;
use brain3_core::ports::token_store::{AccessTokenData, TokenStore};

pub struct SqliteTokenStore {
    pool: SqlitePool,
    schema_ready: OnceCell<()>,
}

impl SqliteTokenStore {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TokenStoreError> {
        let path = path.as_ref().to_path_buf();
        ensure_parent_dir(&path)?;

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(options);

        Ok(Self {
            pool,
            schema_ready: OnceCell::new(),
        })
    }

    pub fn in_memory() -> Result<Self, TokenStoreError> {
        let options = SqliteConnectOptions::new().filename(":memory:");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(options);

        Ok(Self {
            pool,
            schema_ready: OnceCell::new(),
        })
    }

    async fn ensure_schema(&self) -> Result<(), TokenStoreError> {
        self.schema_ready
            .get_or_try_init(|| async {
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS access_tokens (
                        token TEXT PRIMARY KEY,
                        client_id TEXT NOT NULL,
                        expires_at INTEGER NOT NULL
                    )",
                )
                .execute(&self.pool)
                .await
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

                Ok(())
            })
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl TokenStore for SqliteTokenStore {
    async fn store(&self, token: String, data: AccessTokenData) -> Result<(), TokenStoreError> {
        self.ensure_schema().await?;

        sqlx::query(
            "INSERT OR REPLACE INTO access_tokens (token, client_id, expires_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(token)
        .bind(data.client_id)
        .bind(system_time_to_unix_seconds(data.expires_at)?)
        .execute(&self.pool)
        .await
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

        Ok(())
    }

    async fn get(&self, token: &str) -> Result<Option<AccessTokenData>, TokenStoreError> {
        self.ensure_schema().await?;

        let row = sqlx::query(
            "SELECT client_id, expires_at
             FROM access_tokens
             WHERE token = ?1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

        row.map(|row| {
            let client_id: String = row
                .try_get("client_id")
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
            let expires_at: i64 = row
                .try_get("expires_at")
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

            Ok(AccessTokenData {
                client_id,
                expires_at: unix_seconds_to_system_time(expires_at)?,
            })
        })
        .transpose()
    }

    async fn revoke(&self, token: &str) -> Result<(), TokenStoreError> {
        self.ensure_schema().await?;

        sqlx::query("DELETE FROM access_tokens WHERE token = ?1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<(), TokenStoreError> {
        self.ensure_schema().await?;

        sqlx::query("DELETE FROM access_tokens WHERE expires_at < ?1")
            .bind(system_time_to_unix_seconds(SystemTime::now())?)
            .execute(&self.pool)
            .await
            .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

        Ok(())
    }
}

fn ensure_parent_dir(path: &PathBuf) -> Result<(), TokenStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    }
    Ok(())
}

fn system_time_to_unix_seconds(time: SystemTime) -> Result<i64, TokenStoreError> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    i64::try_from(duration.as_secs())
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))
}

fn unix_seconds_to_system_time(seconds: i64) -> Result<SystemTime, TokenStoreError> {
    let seconds =
        u64::try_from(seconds).map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    Ok(UNIX_EPOCH + Duration::from_secs(seconds))
}
