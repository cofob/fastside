use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use reqwest::Client;

use crate::types::InstanceChecker;

pub struct DefaultInstanceChecker {
    client: Client,
}

impl DefaultInstanceChecker {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for DefaultInstanceChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl InstanceChecker for DefaultInstanceChecker {
    async fn check(&self, instance: &Instance) -> anyhow::Result<bool> {
        let response = self.client.get(&instance.url.to_string()).send().await?;
        Ok(response.status().is_success())
    }
}
