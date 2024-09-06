use crate::{types::ServiceUpdater, ChangesSummary};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct LibrexUpdater {
    pub instances_url: String,
}

impl LibrexUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://raw.githubusercontent.com/Ahwxorg/LibreY/main/instances.json"
                .to_string(),
        }
    }
}

impl Default for LibrexUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct LibrexInstance {
    #[serde(default)]
    clearnet: Option<Url>,
    #[serde(default)]
    tor: Option<Url>,
    #[serde(default)]
    i2p: Option<Url>,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse {
    instances: Vec<LibrexInstance>,
}

#[async_trait]
impl ServiceUpdater for LibrexUpdater {
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

        let mut parsed_urls = Vec::new();
        for instance in parsed.instances {
            if let Some(url) = instance.clearnet {
                parsed_urls.push(url);
            }
            if let Some(url) = instance.tor {
                parsed_urls.push(url);
            }
            if let Some(url) = instance.i2p {
                parsed_urls.push(url);
            }
        }

        for url in parsed_urls {
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "librex",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
