use crate::domain::oauth::AuthCodeData;

#[async_trait::async_trait]
pub trait AuthCodeStore: Send + Sync {
    async fn store(&self, code: String, data: AuthCodeData);
    async fn take(&self, code: &str) -> Option<AuthCodeData>;
    async fn cleanup_expired(&self);
}
