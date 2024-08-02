use std::collections::HashMap;

use fastside_shared::serde_types::{ProxyData, ServicesData, UserConfig};

pub struct CompiledRegexSearch {
    pub regex: regex::Regex,
    pub url: String,
}

pub type Regexes = HashMap<String, Vec<CompiledRegexSearch>>;

#[derive(Debug)]
pub struct LoadedData {
    pub services: ServicesData,
    pub proxies: ProxyData,
    pub default_settings: UserConfig,
}
