use crate::{types::ServiceUpdater, ChangesSummary};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct TentUpdater {
    pub instances_url: String,
}

impl TentUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://forgejo.sny.sh/sun/Tent/raw/branch/main/instances.json"
                .to_string(),
        }
    }
}

impl Default for TentUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct TentInstance {
    url: Url,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(Vec<TentInstance>);

#[async_trait]
impl ServiceUpdater for TentUpdater {
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
            let url = instance.url;
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "tent",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
