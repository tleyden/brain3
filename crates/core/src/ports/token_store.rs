use std::time::SystemTime;

use async_trait::async_trait;

use crate::domain::errors::TokenStoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTokenData {
    pub client_id: String,
    pub expires_at: SystemTime,
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn store(&self, token: String, data: AccessTokenData) -> Result<(), TokenStoreError>;
    async fn get(&self, token: &str) -> Result<Option<AccessTokenData>, TokenStoreError>;
    async fn revoke(&self, token: &str) -> Result<(), TokenStoreError>;
    async fn cleanup_expired(&self) -> Result<(), TokenStoreError>;
}
