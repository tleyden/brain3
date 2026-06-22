use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use oxide_auth::primitives::issuer::{Issuer, TokenMap};
use oxide_auth::primitives::generator::RandomGenerator;
use tokio::sync::Mutex;

use brain3_core::domain::errors::TokenStoreError;
use brain3_core::ports::token_store::{StoredTokenData, StoredTokenKind, TokenStore};

/// Wraps oxide-auth's in-memory TokenMap to implement the TokenStore port.
///
/// Only get() is functional — it validates bearer tokens via TokenMap::recover_token().
/// store(), revoke(), and cleanup_expired() are no-ops because oxide-auth manages
/// the token lifecycle internally.
pub struct TokenMapStore {
    inner: Arc<Mutex<TokenMap<RandomGenerator>>>,
}

impl TokenMapStore {
    pub fn new(inner: Arc<Mutex<TokenMap<RandomGenerator>>>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl TokenStore for TokenMapStore {
    async fn store(&self, _token: String, _data: StoredTokenData) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn get(&self, token: &str) -> Result<Option<StoredTokenData>, TokenStoreError> {
        let issuer = self.inner.lock().await;
        match issuer.recover_token(token) {
            Ok(Some(grant)) => {
                let secs = grant.until.timestamp();
                let expires_at = if secs >= 0 {
                    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64)
                } else {
                    SystemTime::UNIX_EPOCH
                };
                Ok(Some(StoredTokenData {
                    client_id: grant.client_id.clone(),
                    kind: StoredTokenKind::Access,
                    expires_at,
                }))
            }
            Ok(None) => Ok(None),
            Err(()) => Ok(None),
        }
    }

    async fn revoke(&self, _token: &str) -> Result<(), TokenStoreError> {
        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<(), TokenStoreError> {
        Ok(())
    }
}
