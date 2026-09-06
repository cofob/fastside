use async_trait::async_trait;
use fastside_shared::client_builder::ProbeClient;
use fastside_shared::serde_types::{HttpCodeRanges, Instance, Service};

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
        client: ProbeClient,
        service: &Service,
        instance: &Instance,
    ) -> anyhow::Result<bool> {
        let url = instance.url.join(&service.test_url)?;
        let response = client.probe(url).await?;
        if !response.solved
            && !response.reused
            && instance.tags.iter().any(|tag| tag == "antibot")
            && !instance.tags.iter().any(|tag| tag == "anubis")
        {
            debug!(
                "Skipping response checks for antibot instance: {}",
                instance.url
            );
            return Ok(true);
        }

        let status_code = response.status.as_u16();
        if service.allowed_http_codes.is_allowed(status_code) {
            if let Some(search_string) = &service.search_string {
                let body = response.body;
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
