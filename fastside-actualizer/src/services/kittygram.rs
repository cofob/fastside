use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

const INSTANCES_URL: &str =
    "https://codeberg.org/irelephant/kittygram/raw/branch/main/instances.json";

pub struct KittygramUpdater {
    pub instances_url: String,
}

impl KittygramUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: INSTANCES_URL.to_string(),
        }
    }
}

impl Default for KittygramUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct KittygramInstance {
    url: Option<Url>,
    tor: Option<Url>,
    i2p: Option<Url>,
    yggdrasil: Option<Url>,
}

fn parse_instance_urls(response_str: &str) -> anyhow::Result<Vec<Url>> {
    let parsed: Vec<KittygramInstance> = serde_json::from_str(response_str)?;

    Ok(parsed
        .into_iter()
        .flat_map(|instance| {
            [instance.url, instance.tor, instance.i2p, instance.yggdrasil]
                .into_iter()
                .flatten()
        })
        .collect())
}

#[async_trait]
impl ServiceUpdater for KittygramUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
        changes_summary: ChangesSummary,
    ) -> anyhow::Result<Vec<Instance>> {
        let response = client.get(&self.instances_url).send().await?;
        let response_str = response.text().await?;
        let parsed_urls = parse_instance_urls(&response_str)?;

        let mut instances = current_instances.to_vec();
        let mut new_instances = Vec::new();

        for url in parsed_urls {
            if current_instances.iter().any(|instance| instance.url == url) {
                continue;
            }

            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "kittygram",
                new_instances
                    .iter()
                    .map(|instance| instance.url.clone())
                    .collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
