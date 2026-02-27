use crate::{ChangesSummary, types::ServiceUpdater, utils::url::default_domain_scheme};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct InvidiousUpdater {
    pub instances_url: String,
}

impl InvidiousUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://api.invidious.io/instances.json".to_string(),
        }
    }
}

impl Default for InvidiousUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct InstancesResponse(Vec<(String, serde_json::Value)>);

#[async_trait]
impl ServiceUpdater for InvidiousUpdater {
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
            let domain = instance.0;
            let scheme = default_domain_scheme(domain.as_str());
            let url = Url::parse(&format!("{}{}", scheme, domain))?;
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "invidious",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
