use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct BreezewikiUpdater {
    pub instances_url: String,
}

impl BreezewikiUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://docs.breezewiki.com/files/instances.json".to_string(),
        }
    }
}

impl Default for BreezewikiUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct BreezewikiInstance {
    instance: Url,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(Vec<BreezewikiInstance>);

#[async_trait]
impl ServiceUpdater for BreezewikiUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
        changes_summary: ChangesSummary,
    ) -> anyhow::Result<Vec<Instance>> {
        let response = client.get(&self.instances_url).send().await?;
        let response_str = response.text().await?;
        let parsed: InstancesResponse = serde_json::from_str(&response_str)?;

        let mut instances = current_instances.to_vec();
        let mut new_instances = Vec::new();

        for instance in parsed.0 {
            let url = instance.instance;
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "breezewiki",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
