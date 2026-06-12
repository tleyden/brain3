use std::time::SystemTime;

use async_trait::async_trait;

use crate::domain::errors::TokenStoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredTokenKind {
    Access,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTokenData {
    pub client_id: String,
    pub kind: StoredTokenKind,
    pub expires_at: SystemTime,
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn store(&self, token: String, data: StoredTokenData) -> Result<(), TokenStoreError>;
    async fn get(&self, token: &str) -> Result<Option<StoredTokenData>, TokenStoreError>;
    async fn revoke(&self, token: &str) -> Result<(), TokenStoreError>;
    async fn cleanup_expired(&self) -> Result<(), TokenStoreError>;
}
