use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct LibredditUpdater {
    pub instances_url: String,
}

impl LibredditUpdater {
    pub fn new() -> Self {
        Self {
            instances_url:
                "https://raw.githubusercontent.com/redlib-org/redlib-instances/main/instances.json"
                    .to_string(),
        }
    }
}

impl Default for LibredditUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct LibredditInstance {
    url: Url,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse {
    instances: Vec<LibredditInstance>,
}

#[async_trait]
impl ServiceUpdater for LibredditUpdater {
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

        for instance in parsed.instances {
            let url = instance.url;
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "libreddit",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
