use crate::{types::ServiceUpdater, ChangesSummary};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct ScribeUpdater {
    pub instances_url: String,
}

impl ScribeUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://git.sr.ht/~edwardloveall/scribe/blob/main/docs/instances.json"
                .to_string(),
        }
    }
}

impl Default for ScribeUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(Vec<Url>);

#[async_trait]
impl ServiceUpdater for ScribeUpdater {
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

        for url in parsed.0 {
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "Scribe",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
