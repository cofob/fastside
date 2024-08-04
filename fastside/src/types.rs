use std::collections::HashMap;

use fastside_shared::{
    config::{ProxyData, UserConfig},
    serde_types::ServicesData,
};

pub struct CompiledRegexSearch {
    pub regex: regex::Regex,
    pub url: String,
}

pub type Regexes = HashMap<String, Vec<CompiledRegexSearch>>;

#[derive(Debug)]
pub struct LoadedData {
    pub services: ServicesData,
    pub proxies: ProxyData,
    pub default_user_config: UserConfig,
}
