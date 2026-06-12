use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tokio::sync::OnceCell;

use brain3_core::domain::errors::TokenStoreError;
use brain3_core::ports::token_store::{StoredTokenData, StoredTokenKind, TokenStore};

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
                        kind TEXT NOT NULL DEFAULT 'access',
                        expires_at INTEGER NOT NULL
                    )",
                )
                .execute(&self.pool)
                .await
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

                let has_kind = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM pragma_table_info('access_tokens') WHERE name = 'kind'",
                )
                .fetch_one(&self.pool)
                .await
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

                if has_kind == 0 {
                    sqlx::query(
                        "ALTER TABLE access_tokens ADD COLUMN kind TEXT NOT NULL DEFAULT 'access'",
                    )
                    .execute(&self.pool)
                    .await
                    .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
                }

                Ok(())
            })
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl TokenStore for SqliteTokenStore {
    async fn store(&self, token: String, data: StoredTokenData) -> Result<(), TokenStoreError> {
        self.ensure_schema().await?;

        sqlx::query(
            "INSERT OR REPLACE INTO access_tokens (token, client_id, kind, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(token)
        .bind(data.client_id)
        .bind(token_kind_to_str(data.kind))
        .bind(system_time_to_unix_seconds(data.expires_at)?)
        .execute(&self.pool)
        .await
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

        Ok(())
    }

    async fn get(&self, token: &str) -> Result<Option<StoredTokenData>, TokenStoreError> {
        self.ensure_schema().await?;

        let row = sqlx::query(
            "SELECT client_id, kind, expires_at
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
            let kind = token_kind_from_str(
                &row.try_get::<String, _>("kind")
                    .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?,
            )?;
            let expires_at: i64 = row
                .try_get("expires_at")
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;

            Ok(StoredTokenData {
                client_id,
                kind,
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

fn token_kind_to_str(kind: StoredTokenKind) -> &'static str {
    match kind {
        StoredTokenKind::Access => "access",
        StoredTokenKind::Refresh => "refresh",
    }
}

fn token_kind_from_str(value: &str) -> Result<StoredTokenKind, TokenStoreError> {
    match value {
        "access" => Ok(StoredTokenKind::Access),
        "refresh" => Ok(StoredTokenKind::Refresh),
        other => Err(TokenStoreError::Unavailable(format!(
            "unknown token kind '{other}' in sqlite store"
        ))),
    }
}
