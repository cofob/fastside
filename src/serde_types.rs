use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Instance {
    pub url: Url,
    pub tags: Vec<String>,
}

fn default_test_url() -> String {
    "/".to_string()
}

const fn default_follow_redirects() -> bool {
    true
}

const fn default_allow_3xx() -> bool {
    false
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Service {
    #[serde(rename = "type")]
    pub name: String,
    #[serde(default = "default_test_url")]
    pub test_url: String,
    pub fallback: Url,
    #[serde(default = "default_follow_redirects")]
    pub follow_redirects: bool,
    #[serde(default = "default_allow_3xx")]
    pub allow_3xx: bool,
    #[serde(default)]
    pub search_string: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub instances: Vec<Instance>,
}

/// Type for `services.json` file.
pub type ServicesData = HashMap<String, Service>;
