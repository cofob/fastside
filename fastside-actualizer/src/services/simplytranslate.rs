use crate::{types::ServiceUpdater, ChangesSummary};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct SimplyTranslateUpdater {
    pub instances_url: String,
}

impl SimplyTranslateUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://codeberg.org/SimpleWeb/Website/raw/branch/master/config.json"
                .to_string(),
        }
    }
}

impl Default for SimplyTranslateUpdater {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct Project {
    id: String,
    #[serde(default)]
    instances: Vec<String>,
    #[serde(default)]
    onion_instances: Vec<String>,
    #[serde(default)]
    i2p_instances: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct InstancesResponse {
    projects: Vec<Project>,
}

#[async_trait]
impl ServiceUpdater for SimplyTranslateUpdater {
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

        let st_project = parsed
            .projects
            .iter()
            .find(|p| p.id == "simplytranslate")
            .ok_or(anyhow::anyhow!(
                "No project with id 'simplytranslate' found"
            ))?;

        for domain in st_project
            .instances
            .iter()
            .map(|i| format!("https://{}", i))
            .chain(
                st_project
                    .i2p_instances
                    .iter()
                    .chain(st_project.onion_instances.iter())
                    .map(|i| format!("http://{}", i)),
            )
        {
            let url = Url::parse(domain.as_str())?;
            if current_instances.iter().any(|i| i.url == url) {
                continue;
            }
            new_instances.push(Instance::from(url.clone()));
        }

        changes_summary
            .set_new_instances_added(
                "simplytranslate",
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
