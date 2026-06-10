use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::RwLock;

use brain3_core::domain::oauth::AuthCodeData;
use brain3_core::ports::auth_code_store::AuthCodeStore;

pub struct InMemoryAuthCodeStore {
    codes: RwLock<HashMap<String, AuthCodeData>>,
}

impl Default for InMemoryAuthCodeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAuthCodeStore {
    pub fn new() -> Self {
        Self {
            codes: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl AuthCodeStore for InMemoryAuthCodeStore {
    async fn store(&self, code: String, data: AuthCodeData) {
        self.codes.write().await.insert(code, data);
    }

    async fn take(&self, code: &str) -> Option<AuthCodeData> {
        self.codes.write().await.remove(code)
    }

    async fn cleanup_expired(&self) {
        let now = Instant::now();
        self.codes
            .write()
            .await
            .retain(|_, data| data.expires_at > now);
    }
}
