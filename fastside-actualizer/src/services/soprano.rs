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
    #[serde(default)]
    clearnet: String,
    #[serde(default)]
    tor: String,
    #[serde(default)]
    i2p: String,
}

impl SopranoInstance {
    fn into_urls(self) -> Vec<Url> {
        [self.clearnet, self.tor, self.i2p]
            .into_iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| Url::parse(&s).ok())
            .collect()
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

            match serde_json::from_str::<Vec<SopranoInstance>>(&response_text) {
                Ok(instances) => {
                    parsed = Some(instances);
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

        for soprano_instance in parsed {
            for url in soprano_instance.into_urls() {
                if current_instances.iter().any(|instance| instance.url == url) {
                    continue;
                }
                new_instances.push(Instance::from(url));
            }
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
