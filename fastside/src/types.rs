use std::{collections::HashMap, sync::Arc};

use fastside_shared::{
    config::{AppConfig, ProxyData, UserConfig},
    serde_types::ServicesData,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::{
    captcha::CaptchaVerifier, crawler::Crawler, reputation::VoteProtector, storage::StateStore,
};

pub struct CompiledRegexSearch {
    pub regex: regex::Regex,
    pub url: String,
}

pub type Regexes = HashMap<String, Vec<CompiledRegexSearch>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoadedData {
    pub services: ServicesData,
    pub proxies: ProxyData,
    pub default_user_config: UserConfig,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub crawler: Arc<Crawler>,
    pub loaded_data: Arc<RwLock<LoadedData>>,
    pub regexes: Arc<Regexes>,
    pub state_store: Arc<dyn StateStore>,
    pub captcha_verifier: Arc<dyn CaptchaVerifier>,
    pub vote_protector: Arc<VoteProtector>,
}

pub fn compile_regexes(services: &ServicesData) -> Regexes {
    services
        .iter()
        .filter_map(|(name, service)| {
            let regexes = service
                .regexes
                .iter()
                .map(|regex| {
                    Some(CompiledRegexSearch {
                        regex: regex::Regex::new(&regex.regex).ok()?,
                        url: regex.url.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some((name.clone(), regexes))
        })
        .collect()
}
