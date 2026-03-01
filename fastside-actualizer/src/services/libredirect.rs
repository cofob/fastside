use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

pub const LIBREDIRECT_SERVICES: &[&str] = &[
    "invidious",
    "materialious",
    "piped",
    "pipedmaterial",
    "cloudtube",
    "proxitok",
    "send",
    "nitter",
    "redlib",
    "scribe",
    "quetre",
    "libremdb",
    "simplytranslate",
    "lingva",
    "libretranslate",
    "searxng",
    "searx",
    "whoogle",
    "rimgo",
    "pixivfe",
    "safetwitch",
    "hyperpipe",
    "osm",
    "breezewiki",
    "binternet",
    "privatebin",
    "neuters",
    "ruraldictionary",
    "libmedium",
    "dumb",
    "anonymousoverflow",
    "wikiless",
    "biblioreads",
    "suds",
    "poke",
    "gothub",
    "jitsi",
    "tent",
    "laboratory",
    "twineo",
    "priviblur",
    "mozhi",
    "skunkyart",
    "koub",
    "translite",
    "soundcloak",
    "vixipy",
    "litexiv",
];

pub struct LibredirectUpdater {
    pub instances_url: String,
    pub service_name: String,
}

impl LibredirectUpdater {
    pub fn new(service_name: &str) -> Self {
        Self {
            instances_url:
                "https://raw.githubusercontent.com/libredirect/instances/refs/heads/main/data.json"
                    .to_string(),
            service_name: service_name.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LibredirectServiceData {
    #[serde(default)]
    clearnet: Vec<String>,
    #[serde(default)]
    tor: Vec<String>,
    #[serde(default)]
    i2p: Vec<String>,
    #[serde(default)]
    loki: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LibredirectResponse(HashMap<String, LibredirectServiceData>);

#[async_trait]
impl ServiceUpdater for LibredirectUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
        changes_summary: ChangesSummary,
    ) -> anyhow::Result<Vec<Instance>> {
        let response = client.get(&self.instances_url).send().await?;
        let response_str = response.text().await?;
        let parsed: LibredirectResponse = serde_json::from_str(&response_str)?;

        // Find the service data by checking all service names (case-insensitive)
        let service_data = parsed
            .0
            .iter()
            .find(|(key, _)| key.to_lowercase() == self.service_name.to_lowercase())
            .map(|(_, value)| value);

        let Some(service_data) = service_data else {
            // Service not found in libredirect data, return current instances unchanged
            return Ok(current_instances.to_vec());
        };

        let mut instances = current_instances.to_vec();
        let mut new_instances = Vec::new();

        // Collect all URLs from all network types
        let all_urls = service_data
            .clearnet
            .iter()
            .chain(&service_data.tor)
            .chain(&service_data.i2p)
            .chain(&service_data.loki);

        for url_str in all_urls {
            if let Ok(url) = Url::parse(url_str) {
                if current_instances.iter().any(|i| i.url == url) {
                    continue;
                }
                new_instances.push(Instance::from(url));
            }
        }

        changes_summary
            .set_new_instances_added(
                &self.service_name,
                new_instances.iter().map(|i| i.url.clone()).collect(),
            )
            .await;

        instances.extend(new_instances);

        Ok(instances)
    }
}
