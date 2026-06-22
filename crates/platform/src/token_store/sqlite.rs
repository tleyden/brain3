use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use oxide_auth::primitives::generator::{RandomGenerator, TagGrant};
use oxide_auth::primitives::grant::{Extensions, Grant, Value};
use oxide_auth::primitives::issuer::{IssuedToken, Issuer, RefreshedToken, TokenType};
use oxide_auth::primitives::scope::Scope;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};

use brain3_core::domain::errors::TokenStoreError;

pub struct SqliteTokenStore {
    connect_options: SqliteConnectOptions,
    schema_ready: OnceLock<()>,
    generator: RandomGenerator,
    usage: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    Access,
    Refresh,
}

#[derive(Debug)]
struct TokenRow {
    pair_id: String,
    kind: TokenKind,
    grant: Grant,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredExtension {
    identifier: String,
    visibility: String,
    value: Option<String>,
}

impl SqliteTokenStore {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TokenStoreError> {
        let path = path.as_ref().to_path_buf();
        ensure_parent_dir(&path)?;

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        Ok(Self {
            connect_options: options,
            schema_ready: OnceLock::new(),
            generator: RandomGenerator::new(32),
            usage: 0,
        })
    }

    pub fn in_memory() -> Result<Self, TokenStoreError> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("brain3-oauth-issuer-{unique}.sqlite"));
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        Ok(Self {
            connect_options: options,
            schema_ready: OnceLock::new(),
            generator: RandomGenerator::new(32),
            usage: 0,
        })
    }

    pub async fn cleanup_expired(&self) -> Result<(), TokenStoreError> {
        self.ensure_schema()?;
        self.run_db(|mut conn| async move {
            sqlx::query(
                "DELETE FROM oauth_tokens
                 WHERE CAST(strftime('%s', expires_at) AS INTEGER) < CAST(strftime('%s', 'now') AS INTEGER)",
            )
            .execute(&mut conn)
            .await
            .map_err(sqlite_error)?;
            Ok(())
        })
    }

    fn ensure_schema(&self) -> Result<(), TokenStoreError> {
        if self.schema_ready.get().is_some() {
            return Ok(());
        }

        self.run_db(|mut conn| async move {
            sqlx::query("DROP TABLE IF EXISTS access_tokens")
                .execute(&mut conn)
                .await
                .map_err(sqlite_error)?;

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS oauth_tokens (
                    token TEXT PRIMARY KEY,
                    pair_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    owner_id TEXT NOT NULL,
                    client_id TEXT NOT NULL,
                    redirect_uri TEXT NOT NULL,
                    scope TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    extensions_json TEXT NOT NULL
                )",
            )
            .execute(&mut conn)
            .await
            .map_err(sqlite_error)?;

            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_oauth_tokens_pair_id
                 ON oauth_tokens (pair_id)",
            )
            .execute(&mut conn)
            .await
            .map_err(sqlite_error)?;

            Ok(())
        })?;

        let _ = self.schema_ready.set(());
        Ok(())
    }

    fn run_db<T, F, Fut>(&self, op: F) -> Result<T, TokenStoreError>
    where
        T: Send + 'static,
        F: FnOnce(SqliteConnection) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, TokenStoreError>> + Send + 'static,
    {
        let connect_options = self.connect_options.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
            runtime.block_on(async move {
                let conn = SqliteConnection::connect_with(&connect_options)
                    .await
                    .map_err(sqlite_error)?;
                op(conn).await
            })
        })
        .join()
        .map_err(|_| TokenStoreError::Unavailable("sqlite worker panicked".into()))?
    }

    fn issue_pair(
        &mut self,
        pair_id: String,
        access_token: String,
        refresh_token: String,
        grant: Grant,
    ) -> Result<(), TokenStoreError> {
        self.ensure_schema()?;

        let access_row = TokenRow {
            pair_id: pair_id.clone(),
            kind: TokenKind::Access,
            grant: grant.clone(),
        };
        let refresh_row = TokenRow {
            pair_id,
            kind: TokenKind::Refresh,
            grant,
        };

        self.run_db(move |mut conn| async move {
            let mut tx = conn.begin().await.map_err(sqlite_error)?;

            insert_token_row(&mut tx, &access_token, &access_row).await?;
            insert_token_row(&mut tx, &refresh_token, &refresh_row).await?;

            tx.commit().await.map_err(sqlite_error)?;
            Ok(())
        })
    }

    fn rotate_refresh_pair(
        &mut self,
        old_refresh_token: &str,
        pair_id: String,
        access_token: String,
        refresh_token: String,
        grant: Grant,
    ) -> Result<(), TokenStoreError> {
        self.ensure_schema()?;

        let access_row = TokenRow {
            pair_id: pair_id.clone(),
            kind: TokenKind::Access,
            grant: grant.clone(),
        };
        let refresh_row = TokenRow {
            pair_id,
            kind: TokenKind::Refresh,
            grant,
        };
        let old_refresh_token = old_refresh_token.to_string();

        self.run_db(move |mut conn| async move {
            let mut tx = conn.begin().await.map_err(sqlite_error)?;

            let old_pair_id = sqlx::query_scalar::<_, String>(
                "SELECT pair_id FROM oauth_tokens WHERE token = ?1 AND kind = 'refresh'",
            )
            .bind(&old_refresh_token)
            .fetch_optional(&mut *tx)
            .await
            .map_err(sqlite_error)?
            .ok_or_else(|| TokenStoreError::Unavailable("refresh token not found".into()))?;

            sqlx::query("DELETE FROM oauth_tokens WHERE pair_id = ?1")
                .bind(old_pair_id)
                .execute(&mut *tx)
                .await
                .map_err(sqlite_error)?;

            insert_token_row(&mut tx, &access_token, &access_row).await?;
            insert_token_row(&mut tx, &refresh_token, &refresh_row).await?;

            tx.commit().await.map_err(sqlite_error)?;
            Ok(())
        })
    }

    fn load_token(&self, token: &str, expected_kind: TokenKind) -> Result<Option<Grant>, TokenStoreError> {
        self.ensure_schema()?;
        let token = token.to_string();

        self.run_db(move |mut conn| async move {
            let row = sqlx::query(
                "SELECT pair_id, kind, owner_id, client_id, redirect_uri, scope, expires_at, extensions_json
                 FROM oauth_tokens
                 WHERE token = ?1",
            )
            .bind(token)
            .fetch_optional(&mut conn)
            .await
            .map_err(sqlite_error)?;

            row.map(token_row_from_sql_row).transpose().map(|maybe_row| {
                maybe_row.and_then(|row| {
                    if row.kind == expected_kind {
                        Some(row.grant)
                    } else {
                        None
                    }
                })
            })
        })
    }
}

impl Issuer for SqliteTokenStore {
    fn issue(&mut self, grant: Grant) -> Result<IssuedToken, ()> {
        let pair_id = self.generator.tag(self.usage, &grant)?;
        let access_token = self.generator.tag(self.usage.wrapping_add(1), &grant)?;
        let refresh_token = self.generator.tag(self.usage.wrapping_add(2), &grant)?;
        self.usage = self.usage.wrapping_add(3);

        self.issue_pair(
            pair_id,
            access_token.clone(),
            refresh_token.clone(),
            grant.clone(),
        )
        .map_err(|_| ())?;

        Ok(IssuedToken {
            token: access_token,
            refresh: Some(refresh_token),
            until: grant.until,
            token_type: TokenType::Bearer,
        })
    }

    fn refresh(&mut self, refresh: &str, grant: Grant) -> Result<RefreshedToken, ()> {
        let pair_id = self.generator.tag(self.usage, &grant)?;
        let access_token = self.generator.tag(self.usage.wrapping_add(1), &grant)?;
        let refresh_token = self.generator.tag(self.usage.wrapping_add(2), &grant)?;
        self.usage = self.usage.wrapping_add(3);

        self.rotate_refresh_pair(
            refresh,
            pair_id,
            access_token.clone(),
            refresh_token.clone(),
            grant.clone(),
        )
        .map_err(|_| ())?;

        Ok(RefreshedToken {
            token: access_token,
            refresh: Some(refresh_token),
            until: grant.until,
            token_type: TokenType::Bearer,
        })
    }

    fn recover_token<'a>(&'a self, token: &'a str) -> Result<Option<Grant>, ()> {
        self.load_token(token, TokenKind::Access).map_err(|_| ())
    }

    fn recover_refresh<'a>(&'a self, token: &'a str) -> Result<Option<Grant>, ()> {
        self.load_token(token, TokenKind::Refresh).map_err(|_| ())
    }
}

async fn insert_token_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    token: &str,
    row: &TokenRow,
) -> Result<(), TokenStoreError> {
    sqlx::query(
        "INSERT INTO oauth_tokens (
            token,
            pair_id,
            kind,
            owner_id,
            client_id,
            redirect_uri,
            scope,
            expires_at,
            extensions_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(token)
    .bind(&row.pair_id)
    .bind(token_kind_to_str(row.kind))
    .bind(&row.grant.owner_id)
    .bind(&row.grant.client_id)
    .bind(row.grant.redirect_uri.to_string())
    .bind(serialize_scope(&row.grant.scope))
    .bind(row.grant.until.to_rfc3339())
    .bind(serialize_extensions(&row.grant.extensions)?)
    .execute(&mut **tx)
    .await
    .map_err(sqlite_error)?;

    Ok(())
}

fn token_row_from_sql_row(row: sqlx::sqlite::SqliteRow) -> Result<TokenRow, TokenStoreError> {
    Ok(TokenRow {
        pair_id: row.try_get("pair_id").map_err(sqlite_error)?,
        kind: token_kind_from_str(&row.try_get::<String, _>("kind").map_err(sqlite_error)?)?,
        grant: Grant {
            owner_id: row.try_get("owner_id").map_err(sqlite_error)?,
            client_id: row.try_get("client_id").map_err(sqlite_error)?,
            redirect_uri: row
                .try_get::<String, _>("redirect_uri")
                .map_err(sqlite_error)?
                .parse()
                .map_err(|error| TokenStoreError::Unavailable(format!("invalid redirect URI: {error}")))?,
            scope: deserialize_scope(
                &row.try_get::<String, _>("scope").map_err(sqlite_error)?,
            )?,
            until: row
                .try_get::<String, _>("expires_at")
                .map_err(sqlite_error)?
                .parse()
                .map_err(|error| TokenStoreError::Unavailable(format!("invalid expiry timestamp: {error}")))?,
            extensions: deserialize_extensions(
                &row.try_get::<String, _>("extensions_json").map_err(sqlite_error)?,
            )?,
        },
    })
}

fn serialize_scope(scope: &Scope) -> String {
    scope.to_string()
}

fn deserialize_scope(scope: &str) -> Result<Scope, TokenStoreError> {
    scope
        .parse()
        .map_err(|error| TokenStoreError::Unavailable(format!("invalid scope: {error}")))
}

fn serialize_extensions(extensions: &Extensions) -> Result<String, TokenStoreError> {
    let mut stored = Vec::new();

    for (identifier, value) in extensions.public() {
        stored.push(StoredExtension {
            identifier: identifier.to_string(),
            visibility: "public".into(),
            value: value.map(str::to_string),
        });
    }

    for (identifier, value) in extensions.private() {
        stored.push(StoredExtension {
            identifier: identifier.to_string(),
            visibility: "private".into(),
            value: value.map(str::to_string),
        });
    }

    serde_json::to_string(&stored)
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))
}

fn deserialize_extensions(payload: &str) -> Result<Extensions, TokenStoreError> {
    let stored: Vec<StoredExtension> =
        serde_json::from_str(payload).map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    let mut extensions = Extensions::new();

    for extension in stored {
        let value = match extension.visibility.as_str() {
            "public" => Value::public(extension.value),
            "private" => Value::private(extension.value),
            other => {
                return Err(TokenStoreError::Unavailable(format!(
                    "unknown extension visibility '{other}'"
                )))
            }
        };
        extensions.set_raw(extension.identifier, value);
    }

    Ok(extensions)
}

fn ensure_parent_dir(path: &PathBuf) -> Result<(), TokenStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    }
    Ok(())
}

fn token_kind_to_str(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Access => "access",
        TokenKind::Refresh => "refresh",
    }
}

fn token_kind_from_str(value: &str) -> Result<TokenKind, TokenStoreError> {
    match value {
        "access" => Ok(TokenKind::Access),
        "refresh" => Ok(TokenKind::Refresh),
        other => Err(TokenStoreError::Unavailable(format!(
            "unknown token kind '{other}' in sqlite issuer"
        ))),
    }
}

fn sqlite_error(error: impl std::fmt::Display) -> TokenStoreError {
    TokenStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use oxide_auth::primitives::grant::{Extensions, Grant};
    use oxide_auth::primitives::issuer::Issuer;

    use super::*;

    fn sample_grant() -> Grant {
        Grant {
            owner_id: "operator".into(),
            client_id: "brain3-oauth2-client".into(),
            scope: "read".parse().expect("scope should parse"),
            redirect_uri: "https://chatgpt.com/connector/oauth/test"
                .parse()
                .expect("redirect URI should parse"),
            until: "2026-06-22T12:00:00Z"
                .parse()
                .expect("timestamp should parse"),
            extensions: Extensions::new(),
        }
    }

    #[test]
    fn issue_round_trips_access_and_refresh_tokens() {
        let mut store = SqliteTokenStore::in_memory().expect("in-memory store should initialize");
        let grant = sample_grant();

        let issued = store.issue(grant.clone()).expect("issuing token should succeed");

        let access = store
            .recover_token(&issued.token)
            .expect("access token recovery should succeed");
        let refresh = store
            .recover_refresh(issued.refresh.as_deref().expect("refresh token should exist"))
            .expect("refresh token recovery should succeed");

        assert_eq!(access, Some(grant.clone()));
        assert_eq!(refresh, Some(grant));
    }

    #[test]
    fn refresh_rotates_refresh_token_and_replaces_old_access_token() {
        let mut store = SqliteTokenStore::in_memory().expect("in-memory store should initialize");
        let original_grant = sample_grant();
        let issued = store
            .issue(original_grant)
            .expect("issuing initial token should succeed");

        let new_grant = Grant {
            until: "2026-06-22T13:00:00Z"
                .parse()
                .expect("timestamp should parse"),
            ..sample_grant()
        };

        let refreshed = store
            .refresh(
                issued.refresh.as_deref().expect("refresh token should exist"),
                new_grant.clone(),
            )
            .expect("refresh should succeed");

        let old_access = store
            .recover_token(&issued.token)
            .expect("old access token lookup should succeed");
        let old_refresh = store
            .recover_refresh(issued.refresh.as_deref().expect("refresh token should exist"))
            .expect("old refresh token lookup should succeed");
        let new_access = store
            .recover_token(&refreshed.token)
            .expect("new access token lookup should succeed");
        let new_refresh = store
            .recover_refresh(
                refreshed
                    .refresh
                    .as_deref()
                    .expect("new refresh token should exist"),
            )
            .expect("new refresh token lookup should succeed");

        assert_eq!(old_access, None);
        assert_eq!(old_refresh, None);
        assert_eq!(new_access, Some(new_grant.clone()));
        assert_eq!(new_refresh, Some(new_grant));
    }
}
