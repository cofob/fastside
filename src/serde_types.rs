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

const fn default_regex_group() -> usize {
    1
}

pub struct CompiledRegexSearch {
    pub regex: regex::Regex,
    pub group: usize,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegexSearch {
    pub regex: String,
    #[serde(default = "default_regex_group")]
    pub group: usize,
}

pub type Regexes = HashMap<String, Vec<CompiledRegexSearch>>;

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
    pub regexes: Vec<RegexSearch>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub instances: Vec<Instance>,
}

pub type ServicesData = HashMap<String, Service>;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Proxy {
    pub url: String,
    #[serde(default)]
    pub auth: Option<ProxyAuth>,
}

pub type ProxyData = HashMap<String, Proxy>;

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
pub enum SelectMethod {
    #[default]
    Random,
    LowPing,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub required_tags: Vec<String>,
    #[serde(default)]
    pub forbidden_tags: Vec<String>,
    #[serde(default)]
    pub select_method: SelectMethod,
    #[serde(default)]
    pub ignore_fallback_warning: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct StoredData {
    pub services: Vec<Service>,
    pub proxies: ProxyData,
    #[serde(default)]
    pub default_settings: UserConfig,
}

#[derive(Debug)]
pub struct LoadedData {
    pub services: ServicesData,
    pub proxies: ProxyData,
    pub default_settings: UserConfig,
}
