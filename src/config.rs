//! Application configuration.

use std::time::Duration;

use anyhow::{Context, Result};
use config::Config;
use serde::{Deserialize, Serialize};

/// Crawler GeoDB settings.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CrawlerGeoDBConfig {}

const fn default_ping_interval() -> Duration {
    // Every 5 minutes
    Duration::from_secs(60 * 5)
}

const fn default_request_timeout() -> Duration {
    Duration::from_secs(5)
}

/// Crawler configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CrawlerConfig {
    #[serde(default = "default_ping_interval")]
    pub ping_interval: Duration,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(default)]
    pub geodb_config: Option<CrawlerGeoDBConfig>,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            ping_interval: default_ping_interval(),
            request_timeout: default_request_timeout(),
            geodb_config: None,
        }
    }
}

/// Application configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub crawler: CrawlerConfig,
}

/// Load application configuration.
pub fn load_config() -> Result<AppConfig> {
    let config = Config::builder()
        .add_source(
            config::Environment::with_prefix("FS")
                .separator("__")
                .list_separator(","),
        )
        .build()
        .context("failed to load config")?;

    debug!("Raw configuration: {:#?}", config);

    let app: AppConfig = config
        .try_deserialize()
        .context("failed to deserialize config")?;

    debug!("Loaded application configuration: {:#?}", app);

    Ok(app)
}
