use crate::types::{InstanceChecker, ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;

pub struct SearxUpdater {
    pub instances_url: String,
    pub client: reqwest::Client,
}

impl SearxUpdater {
    pub fn new(instances_url: String) -> Self {
        let client = reqwest::Client::new();
        Self { instances_url, client }
    }
}

impl Default for SearxUpdater {
    fn default() -> Self {
        Self::new("https://raw.githubusercontent.com/searx/searx-instances/master/searxinstances/instances.yml".to_string())
    }
}

#[async_trait]
impl ServiceUpdater for SearxUpdater {
    async fn update(
        &self,
        current_instances: &[Instance],
    ) -> anyhow::Result<Vec<Instance>> {
        let mut instances = Vec::new();

        let response = self.client.get(&self.url).send().await?;
        let body = response.text().await?;

        let parsed: Vec<Instance> = serde_yaml::from_str(&body)?;

        for instance in parsed {
            if !current_instances.contains(&instance) {
                instances.push(instance);
            }
        }

        Ok(instances)
    }
}

#[async_trait]
impl InstanceChecker for SearxUpdater {
    async fn check(&self, instance: &Instance) -> anyhow::Result<bool> {
        let response = self.client.get(&instance.url).send().await?;
        Ok(response.status().is_success())
    }
}
