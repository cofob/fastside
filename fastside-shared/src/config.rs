//! Application configuration.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use config::Config;
use serde::{Deserialize, Serialize};

use crate::errors::UserConfigError;

const fn default_ping_interval() -> Duration {
    // Every 5 minutes
    Duration::from_secs(60 * 5)
}

const fn default_request_timeout() -> Duration {
    Duration::from_secs(5)
}

const fn default_max_concurrent_requests() -> usize {
    200
}

/// Crawler configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CrawlerConfig {
    #[serde(default = "default_ping_interval")]
    pub ping_interval: Duration,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            ping_interval: default_ping_interval(),
            request_timeout: default_request_timeout(),
            max_concurrent_requests: default_max_concurrent_requests(),
        }
    }
}

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

fn default_required_tags() -> Vec<String> {
    vec![
        "clearnet".to_string(),
        "https".to_string(),
        "ipv4".to_string(),
    ]
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct UserConfig {
    #[serde(default = "default_required_tags")]
    pub required_tags: Vec<String>,
    #[serde(default)]
    pub forbidden_tags: Vec<String>,
    #[serde(default)]
    pub select_method: SelectMethod,
    #[serde(default)]
    pub ignore_fallback_warning: bool,
}

impl UserConfig {
    pub fn to_config_string(&self) -> Result<String, UserConfigError> {
        use base64::prelude::*;
        let json: String = serde_json::to_string(&self).map_err(UserConfigError::Serialization)?;
        Ok(BASE64_STANDARD.encode(json.as_bytes()))
    }

    pub fn from_config_string(data: &str) -> Result<Self, UserConfigError> {
        use base64::prelude::*;
        let decoded = BASE64_STANDARD.decode(data.as_bytes())?;
        let json = String::from_utf8(decoded).unwrap();
        serde_json::from_str(&json).map_err(UserConfigError::from)
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AutoUpdaterConfig {
    pub enabled: bool,
    pub interval: Duration,
}

impl Default for AutoUpdaterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(60),
        }
    }
}

/// Application configuration.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub crawler: CrawlerConfig,
    #[serde(default)]
    pub auto_updater: AutoUpdaterConfig,
    #[serde(default)]
    pub proxies: ProxyData,
    #[serde(default)]
    pub default_user_config: UserConfig,
    #[serde(default)]
    pub services: Option<String>,
}

/// Load application configuration.
pub fn load_config(config_path: &Option<PathBuf>) -> Result<AppConfig> {
    let mut config_builder = Config::builder().add_source(
        config::Environment::with_prefix("FS")
            .separator("__")
            .list_separator(","),
    );

    match config_path {
        Some(path) => {
            config_builder =
                config_builder.add_source(config::File::from(path.clone()).required(true));
        }
        None => {
            config_builder =
                config_builder.add_source(config::File::with_name("config").required(false));
        }
    }

    let config = config_builder.build().context("failed to load config")?;

    debug!("Raw configuration: {:#?}", config);

    let app: AppConfig = config
        .try_deserialize()
        .context("failed to deserialize config")?;

    debug!("Loaded application configuration: {:#?}", app);

    Ok(app)
}
