use std::collections::HashMap;

use crate::types::{InstanceChecker, ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::{Instance, Service};
use serde::Deserialize;
use url::Url;

pub struct SearxUpdater {
    pub instances_url: String,
}

impl SearxUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://raw.githubusercontent.com/searx/searx-instances/master/searxinstances/instances.yml".to_string(),
        }
    }
}

impl Default for SearxUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(HashMap<Url, serde_yaml::Value>);

#[async_trait]
impl ServiceUpdater for SearxUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
    ) -> anyhow::Result<Vec<Instance>> {
        let response = client.get(&self.instances_url).send().await?;
        let response_str = response.text().await?;
        let parsed: InstancesResponse = serde_yaml::from_str(&response_str)?;

        let mut instances = current_instances.to_vec();

        for url in parsed.0.keys() {
            if current_instances.iter().any(|i| &i.url == url) {
                continue;
            }

            instances.push(Instance::from(url.clone()));
        }

        Ok(instances)
    }
}

#[async_trait]
impl InstanceChecker for SearxUpdater {
    async fn check(
        &self,
        client: reqwest::Client,
        _service: &Service,
        instance: &Instance,
    ) -> anyhow::Result<bool> {
        let response = client.get(instance.url.clone()).send().await?;
        Ok(response.status().is_success())
    }
}
