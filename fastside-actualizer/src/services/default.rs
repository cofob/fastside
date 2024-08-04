use async_trait::async_trait;
use fastside_shared::serde_types::{HttpCodeRanges, Instance, Service};
use reqwest::Client;

use crate::types::InstanceChecker;

/// Default instance checker.
///
/// Implements same logic as fastside crawler.
pub struct DefaultInstanceChecker;

impl DefaultInstanceChecker {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultInstanceChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl InstanceChecker for DefaultInstanceChecker {
    async fn check(
        &self,
        client: Client,
        service: &Service,
        instance: &Instance,
    ) -> anyhow::Result<bool> {
        let response = client.get(instance.url.to_string()).send().await?;
        let status_code = response.status().as_u16();
        if service.allowed_http_codes.is_allowed(status_code) {
            if let Some(search_string) = &service.search_string {
                let body = response.text().await?;
                if body.contains(search_string) {
                    Ok(true)
                } else {
                    debug!("Search string not found: {}", search_string);
                    Ok(false)
                }
            } else {
                Ok(true)
            }
        } else {
            debug!("Invalid status code: {}", status_code);
            Ok(false)
        }
    }
}
