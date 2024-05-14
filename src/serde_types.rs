use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use url::Url;

fn default_test_url() -> String {
    "/".to_string()
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Service {
    #[serde(rename = "type")]
    pub name: String,
    #[serde(default = "default_test_url")]
    pub test_url: String,
    pub fallback: Url,
    #[serde(default)]
    pub aliases: HashSet<String>,
    #[serde(default)]
    pub tags: HashSet<String>,
    pub instances: HashSet<Url>,
}

/// Type for `services.json` file.
pub type ServicesData = Vec<Service>;
/// Internal representation of `services.json` file.
pub type Services = HashMap<String, Service>;

pub fn load_services_file(path: &Path) -> Result<Services> {
    let services_data: ServicesData = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let services: Services = HashMap::from_iter(services_data.iter().map(|s| {
        (s.name.clone(), {
            let mut s = s.clone();
            s.aliases.insert(s.name.clone());
            s
        })
    }));
    Ok(services)
}
