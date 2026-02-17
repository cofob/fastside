use crate::{types::ServiceUpdater, ChangesSummary};
use anyhow::anyhow;
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct SopranoUpdater {
    pub instances_urls: [String; 2],
}

impl SopranoUpdater {
    pub fn new() -> Self {
        Self {
            instances_urls: [
                "https://git.vern.cc/cobra/Soprano/raw/branch/main/instances.json".to_string(),
                "https://codeberg.org/cobra/Soprano/raw/branch/main/instances.json".to_string(),
            ],
        }
    }
}

impl Default for SopranoUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct SopranoInstance {
    url: Url,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InstancesResponse {
    Urls(Vec<Url>),
    Objects(Vec<SopranoInstance>),
}

impl InstancesResponse {
    fn into_urls(self) -> Vec<Url> {
        match self {
            Self::Urls(urls) => urls,
            Self::Objects(instances) => {
                instances.into_iter().map(|instance| instance.url).collect()
            }
        }
    }
}

#[async_trait]
impl ServiceUpdater for SopranoUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
        changes_summary: ChangesSummary,
    ) -> anyhow::Result<Vec<Instance>> {
        let mut parsed = None;
        let mut last_error = None;

        for instances_url in &self.instances_urls {
            let response = match client.get(instances_url).send().await {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(anyhow!("failed to fetch {}: {}", instances_url, error));
                    continue;
                }
            };

            if !response.status().is_success() {
                last_error = Some(anyhow!(
                    "failed to fetch {}: HTTP {}",
                    instances_url,
                    response.status()
                ));
                continue;
            }

            let response_text = match response.text().await {
                Ok(response_text) => response_text,
                Err(error) => {
                    last_error = Some(anyhow!(
                        "failed to read response body from {}: {}",
                        instances_url,
                        error
                    ));
                    continue;
                }
            };

            match serde_json::from_str::<InstancesResponse>(&response_text) {
                Ok(response) => {
                    parsed = Some(response.into_urls());
                    break;
                }
                Err(error) => {
                    last_error = Some(anyhow!(
                        "failed to parse JSON from {}: {}",
                        instances_url,
                        error
                    ));
                }
            }
        }

        let parsed = parsed.ok_or_else(|| {
            last_error
                .unwrap_or_else(|| anyhow!("failed to load soprano instances from all sources"))
        })?;

        let mut instances = current_instances.to_vec();
        let mut new_instances = Vec::new();

        for url in parsed {
            if current_instances.iter().any(|instance| instance.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "soprano",
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
