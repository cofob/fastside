//! Application configuration.

#[cfg(feature = "native")]
use std::path::PathBuf;
use std::{collections::HashMap, fmt, time::Duration};

#[cfg(feature = "native")]
use anyhow::{Context, Result};
#[cfg(feature = "native")]
use config::Config;
use serde::{Deserialize, Serialize};

use crate::errors::UserConfigError;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DomainRequestTimeout {
    domain: String,
    timeout: Duration,
}

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
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct CrawlerConfig {
    #[serde(default = "default_ping_interval")]
    pub ping_interval: Duration,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: Duration,
    #[serde(default)]
    pub domain_request_timeouts: Vec<DomainRequestTimeout>,
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
}

impl CrawlerConfig {
    pub fn get_domain_timeout(&self, domain: &str) -> Duration {
        self.domain_request_timeouts
            .iter()
            .find(|drt| domain.ends_with(&drt.domain))
            .map(|drt| drt.timeout)
            .unwrap_or_else(|| self.request_timeout)
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

pub fn select_proxy<'a>(proxies: &'a ProxyData, tags: &[String]) -> Option<&'a Proxy> {
    proxies
        .iter()
        .find_map(|(tag, proxy)| tags.contains(tag).then_some(proxy))
}

#[cfg(test)]
mod configuration_tests {
    use super::*;

    #[test]
    fn selects_proxy_with_matching_tag() {
        let proxy = Proxy {
            url: "socks5h://proxy.example:1080".to_owned(),
            auth: None,
        };
        let proxies = ProxyData::from([("tor".to_owned(), proxy)]);

        assert_eq!(
            select_proxy(&proxies, &["tor".to_owned()]).map(|proxy| proxy.url.as_str()),
            Some("socks5h://proxy.example:1080")
        );
        assert!(select_proxy(&proxies, &["clearnet".to_owned()]).is_none());
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
pub enum SelectMethod {
    #[default]
    Random,
    LowPing,
    Weighted,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub enum StorageBackend {
    #[default]
    Auto,
    Sqlite,
    Redis,
    Cloudflare,
}

fn default_sqlite_path() -> String {
    "fastside.sqlite3".to_owned()
}

const fn default_sqlite_connections() -> u32 {
    8
}

const fn default_storage_timeout() -> Duration {
    Duration::from_secs(5)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SqliteStorageConfig {
    #[serde(default = "default_sqlite_path")]
    pub path: String,
    #[serde(default = "default_sqlite_connections")]
    pub max_connections: u32,
    #[serde(default = "default_storage_timeout")]
    pub busy_timeout: Duration,
}

impl Default for SqliteStorageConfig {
    fn default() -> Self {
        Self {
            path: default_sqlite_path(),
            max_connections: default_sqlite_connections(),
            busy_timeout: default_storage_timeout(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct RedisStorageConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_redis_prefix")]
    pub key_prefix: String,
}

fn default_redis_prefix() -> String {
    "fastside".to_owned()
}

impl Default for RedisStorageConfig {
    fn default() -> Self {
        Self {
            url: None,
            key_prefix: default_redis_prefix(),
        }
    }
}

impl fmt::Debug for RedisStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisStorageConfig")
            .field("url", &self.url.as_ref().map(|_| "[redacted]"))
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

const fn default_cloudflare_publish_interval() -> Duration {
    Duration::from_secs(5)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct CloudflareStorageConfig {
    #[serde(default = "default_cloudflare_publish_interval")]
    pub reputation_publish_interval: Duration,
}

impl Default for CloudflareStorageConfig {
    fn default() -> Self {
        Self {
            reputation_publish_interval: default_cloudflare_publish_interval(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct StorageConfig {
    #[serde(default)]
    pub backend: StorageBackend,
    #[serde(default)]
    pub sqlite: SqliteStorageConfig,
    #[serde(default)]
    pub redis: RedisStorageConfig,
    #[serde(default)]
    pub cloudflare: CloudflareStorageConfig,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub enum CaptchaEncoding {
    #[default]
    Json,
    Form,
}

fn default_captcha_token_field() -> String {
    "cap-token".to_owned()
}

fn default_captcha_secret_field() -> String {
    "secret".to_owned()
}

fn default_captcha_response_field() -> String {
    "response".to_owned()
}

fn default_captcha_success_pointer() -> String {
    "/success".to_owned()
}

#[derive(Deserialize, Serialize, Clone)]
pub struct CaptchaConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub widget_html: String,
    #[serde(default = "default_captcha_token_field")]
    pub token_field: String,
    #[serde(default)]
    pub verify_url: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub encoding: CaptchaEncoding,
    #[serde(default = "default_captcha_secret_field")]
    pub secret_field: String,
    #[serde(default = "default_captcha_response_field")]
    pub response_field: String,
    #[serde(default)]
    pub static_fields: HashMap<String, String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_storage_timeout")]
    pub timeout: Duration,
    #[serde(default = "default_captcha_success_pointer")]
    pub success_json_pointer: String,
}

impl Default for CaptchaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            widget_html: String::new(),
            token_field: default_captcha_token_field(),
            verify_url: None,
            secret: None,
            encoding: CaptchaEncoding::Json,
            secret_field: default_captcha_secret_field(),
            response_field: default_captcha_response_field(),
            static_fields: HashMap::new(),
            headers: HashMap::new(),
            timeout: default_storage_timeout(),
            success_json_pointer: default_captcha_success_pointer(),
        }
    }
}

impl fmt::Debug for CaptchaConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptchaConfig")
            .field("enabled", &self.enabled)
            .field("widget_html", &self.widget_html)
            .field("token_field", &self.token_field)
            .field(
                "verify_url",
                &self.verify_url.as_ref().map(|_| "[redacted]"),
            )
            .field("secret", &self.secret.as_ref().map(|_| "[redacted]"))
            .field("encoding", &self.encoding)
            .field("secret_field", &self.secret_field)
            .field("response_field", &self.response_field)
            .field(
                "static_fields",
                &self.static_fields.keys().collect::<Vec<_>>(),
            )
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("timeout", &self.timeout)
            .field("success_json_pointer", &self.success_json_pointer)
            .finish()
    }
}

const fn default_rate_limit_votes() -> usize {
    10
}

const fn default_rate_limit_window() -> Duration {
    Duration::from_secs(60)
}

const fn default_unique_vote_window() -> Duration {
    Duration::from_secs(30 * 60)
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct VoteRateLimitConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_rate_limit_votes")]
    pub max_votes: usize,
    #[serde(default = "default_rate_limit_window")]
    pub window: Duration,
}

impl Default for VoteRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_votes: default_rate_limit_votes(),
            window: default_rate_limit_window(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UniqueVoteConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_unique_vote_window")]
    pub window: Duration,
}

impl Default for UniqueVoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            window: default_unique_vote_window(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ClientIpConfig {
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct IpProtectionConfig {
    #[serde(default)]
    pub rate_limit: VoteRateLimitConfig,
    #[serde(default)]
    pub one_vote_per_instance: UniqueVoteConfig,
    #[serde(default)]
    pub client_ip: ClientIpConfig,
}

const fn default_minimum_weight() -> f64 {
    0.1
}

const fn default_maximum_weight() -> f64 {
    10.0
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ReputationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_minimum_weight")]
    pub minimum_weight: f64,
    #[serde(default = "default_maximum_weight")]
    pub maximum_weight: f64,
    #[serde(default)]
    pub captcha: CaptchaConfig,
    #[serde(default)]
    pub ip_protection: IpProtectionConfig,
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            minimum_weight: default_minimum_weight(),
            maximum_weight: default_maximum_weight(),
            captcha: CaptchaConfig::default(),
            ip_protection: IpProtectionConfig::default(),
        }
    }
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
    #[serde(default)]
    pub preferred_instances: Vec<String>,
}

impl UserConfig {
    pub fn to_config_string(&self) -> Result<String, UserConfigError> {
        use base64ct::{Base64, Encoding};
        let json: String = serde_json::to_string(&self).map_err(UserConfigError::Serialization)?;
        Ok(Base64::encode_string(json.as_bytes()))
    }

    pub fn from_config_string(data: &str) -> Result<Self, UserConfigError> {
        use base64ct::{Base64, Encoding};
        let decoded = Base64::decode_vec(data)?;
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
    pub storage: StorageConfig,
    #[serde(default)]
    pub reputation: ReputationConfig,
    #[serde(default)]
    pub services: Option<String>,
}

/// Load application configuration.
#[cfg(feature = "native")]
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

    let app: AppConfig = config
        .try_deserialize()
        .context("failed to deserialize config")?;

    debug!("Loaded application configuration: {:#?}", app);

    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_configuration_uses_disabled_reputation_defaults() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.storage.backend, StorageBackend::Auto);
        assert_eq!(config.storage.redis.key_prefix, "fastside");
        assert!(!config.reputation.enabled);
        assert_eq!(config.reputation.minimum_weight, 0.1);
        assert_eq!(config.reputation.maximum_weight, 10.0);
    }

    #[test]
    fn weighted_user_configuration_round_trips() {
        let config = UserConfig {
            select_method: SelectMethod::Weighted,
            ..UserConfig::default()
        };
        assert_eq!(
            UserConfig::from_config_string(&config.to_config_string().unwrap())
                .unwrap()
                .select_method,
            SelectMethod::Weighted
        );
    }

    #[test]
    fn debug_output_redacts_storage_and_captcha_secrets() {
        let mut config = AppConfig::default();
        config.storage.redis.url = Some("redis://user:redis-password@example/".to_owned());
        config.reputation.captcha.secret = Some("captcha-secret".to_owned());
        config.reputation.captcha.verify_url =
            Some("https://captcha.example/verify?key=verify-url-secret".to_owned());
        config
            .reputation
            .captcha
            .headers
            .insert("authorization".to_owned(), "header-secret".to_owned());
        config
            .reputation
            .captcha
            .static_fields
            .insert("api_key".to_owned(), "static-secret".to_owned());
        let debug = format!("{config:?}");
        assert!(!debug.contains("redis-password"));
        assert!(!debug.contains("captcha-secret"));
        assert!(!debug.contains("verify-url-secret"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("static-secret"));
        assert!(debug.contains("[redacted]"));
    }
}
