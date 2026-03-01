use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct AkademikUpdater {
    pub instances_url: String,
}

impl AkademikUpdater {
    pub fn new() -> Self {
        Self {
            instances_url:
                "https://git.bloat.cat/gospodin/akademik/raw/branch/master/instances.json"
                    .to_string(),
        }
    }
}

impl Default for AkademikUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct AkademikInstance {
    url: Url,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(Vec<AkademikInstance>);

#[async_trait]
impl ServiceUpdater for AkademikUpdater {
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
                "akademik",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
