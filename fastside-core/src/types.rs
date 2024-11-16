use std::{collections::HashMap, sync::Arc};

use fastside_shared::{
    config::{AppConfig, ProxyData, UserConfig},
    serde_types::ServicesData,
};
use tokio::sync::RwLock;

use crate::crawler::Crawler;

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

// Shared state type
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub crawler: Arc<Crawler>,
    pub loaded_data: Arc<RwLock<LoadedData>>,
    pub regexes: Regexes,
}
