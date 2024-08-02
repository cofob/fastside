//! Application configuration.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use config::Config;
use serde::{Deserialize, Serialize};

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

/// Application configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    #[serde(default)]
    pub crawler: CrawlerConfig,
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
