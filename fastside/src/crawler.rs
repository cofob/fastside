use std::{collections::HashMap, fmt::Debug, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fastside_shared::{
    config::{CrawlerConfig, ProxyData},
    serde_types::{HttpCodeRanges, Instance, Service, ServicesData},
};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard, RwLock};
use url::Url;

use crate::types::LoadedData;

#[derive(Error, Debug)]
pub enum CrawlerError {
    #[error("url error: `{0}`")]
    Url(#[from] url::ParseError),
    #[error("request error: `{0}`")]
    Request(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CrawledInstanceStatus {
    Ok(Duration),
    InvalidStatusCode(u16, Duration),
    StringNotFound,
    ConnectionError,
    RedirectPolicyError,
    BuilderError,
    RequestError,
    BodyError,
    DecodeError,
    TimedOut,
    Unknown,
}

impl CrawledInstanceStatus {
    /// Get the sortable latency value used by the HTML templates.
    pub fn as_isize(&self) -> isize {
        match self {
            Self::Ok(duration) => duration.as_millis() as isize,
            _ => isize::MAX,
        }
    }
}

impl std::fmt::Display for CrawledInstanceStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrawledInstance {
    pub url: Url,
    pub status: CrawledInstanceStatus,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrawledService {
    pub name: String,
    pub instances: Vec<CrawledInstance>,
}

impl CrawledService {
    pub fn get_alive_instances(&self) -> impl Iterator<Item = &CrawledInstance> {
        self.instances
            .iter()
            .filter(|instance| matches!(instance.status, CrawledInstanceStatus::Ok(_)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrawledServices {
    pub services: HashMap<String, CrawledService>,
    pub time: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CrawledData {
    CrawledServices(CrawledServices),
    InitialLoading,
    ReloadingServices(CrawledServices),
    InitializedFromDefaults(CrawledServices),
}

impl CrawledData {
    pub fn get_services(&self) -> Option<&CrawledServices> {
        match self {
            Self::CrawledServices(services)
            | Self::ReloadingServices(services)
            | Self::InitializedFromDefaults(services) => Some(services),
            Self::InitialLoading => None,
        }
    }

    pub fn is_reloading(&self) -> bool {
        matches!(self, Self::ReloadingServices(_))
    }

    pub fn is_initialized_from_defaults(&self) -> bool {
        matches!(self, Self::InitializedFromDefaults(_))
    }

    #[cfg(feature = "native")]
    fn make_reloading(&mut self) {
        let current = match self {
            Self::CrawledServices(services) | Self::InitializedFromDefaults(services) => {
                services.clone()
            }
            Self::InitialLoading | Self::ReloadingServices(_) => return,
        };
        *self = Self::ReloadingServices(current);
    }
}

#[derive(Debug)]
pub struct InstanceResponse {
    pub status_code: u16,
    pub duration: Duration,
    pub body: Option<String>,
}

#[derive(Debug)]
pub enum InstanceRequest {
    Response(InstanceResponse),
    Failed(CrawledInstanceStatus),
}

#[cfg_attr(feature = "native", async_trait)]
#[cfg_attr(not(feature = "native"), async_trait(?Send))]
pub trait InstanceClient: Debug + Send + Sync {
    async fn request(
        &self,
        config: &CrawlerConfig,
        proxies: &ProxyData,
        service: &Service,
        instance: &Instance,
        test_url: Url,
    ) -> Result<InstanceRequest, CrawlerError>;
}

/// Select a stable range of instances across all services.
pub fn select_instance_batch(
    services: &ServicesData,
    offset: usize,
    limit: usize,
) -> (ServicesData, usize) {
    let mut names = services.keys().collect::<Vec<_>>();
    names.sort_unstable();

    let mut skip = offset;
    let mut remaining = limit;
    let mut batch = ServicesData::new();
    for name in names {
        let service = &services[name];
        if skip >= service.instances.len() {
            skip -= service.instances.len();
            continue;
        }

        let count = remaining.min(service.instances.len() - skip);
        if count == 0 {
            break;
        }
        let mut selected = service.clone();
        selected.instances = service.instances[skip..skip + count].to_vec();
        batch.insert(name.clone(), selected);
        remaining -= count;
        skip = 0;
        if remaining == 0 {
            break;
        }
    }

    (batch, limit - remaining)
}

#[cfg(feature = "native")]
#[derive(Debug, Default)]
pub struct ReqwestInstanceClient;

#[cfg(feature = "native")]
#[async_trait]
impl InstanceClient for ReqwestInstanceClient {
    async fn request(
        &self,
        config: &CrawlerConfig,
        proxies: &ProxyData,
        service: &Service,
        instance: &Instance,
        test_url: Url,
    ) -> Result<InstanceRequest, CrawlerError> {
        use std::time::Instant;

        use fastside_shared::client_builder::build_client;

        let client = build_client(service, config, proxies, instance)
            .map_err(|error| CrawlerError::Request(error.to_string()))?;
        let start = Instant::now();
        let response = match client.get(test_url).send().await {
            Ok(response) => response,
            Err(error) => {
                let status = match error {
                    _ if error.is_timeout() => CrawledInstanceStatus::TimedOut,
                    _ if error.is_builder() => CrawledInstanceStatus::BuilderError,
                    _ if error.is_redirect() => CrawledInstanceStatus::RedirectPolicyError,
                    _ if error.is_request() => CrawledInstanceStatus::RequestError,
                    _ if error.is_body() => CrawledInstanceStatus::BodyError,
                    _ if error.is_decode() => CrawledInstanceStatus::DecodeError,
                    _ if error.is_connect() => CrawledInstanceStatus::ConnectionError,
                    _ => CrawledInstanceStatus::Unknown,
                };
                return Ok(InstanceRequest::Failed(status));
            }
        };

        let duration = start.elapsed();
        let status_code = response.status().as_u16();
        let body = if should_read_response_body(service, instance, status_code) {
            Some(
                response
                    .text()
                    .await
                    .map_err(|error| CrawlerError::Request(error.to_string()))?,
            )
        } else {
            None
        };
        Ok(InstanceRequest::Response(InstanceResponse {
            status_code,
            duration,
            body,
        }))
    }
}

#[derive(Debug)]
pub struct Crawler {
    loaded_data: Arc<RwLock<LoadedData>>,
    config: Arc<CrawlerConfig>,
    client: Arc<dyn InstanceClient>,
    data: RwLock<CrawledData>,
    crawler_lock: Mutex<()>,
}

impl Crawler {
    pub fn new(
        loaded_data: Arc<RwLock<LoadedData>>,
        config: CrawlerConfig,
        client: Arc<dyn InstanceClient>,
    ) -> Self {
        Self::with_data(loaded_data, config, client, CrawledData::InitialLoading)
    }

    pub fn with_data(
        loaded_data: Arc<RwLock<LoadedData>>,
        config: CrawlerConfig,
        client: Arc<dyn InstanceClient>,
        data: CrawledData,
    ) -> Self {
        Self {
            loaded_data,
            config: Arc::new(config),
            client,
            data: RwLock::new(data),
            crawler_lock: Mutex::new(()),
        }
    }

    #[cfg(feature = "native")]
    pub async fn save_ping_data_to_file(
        &self,
        file_path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let data = self.data.read().await;
        if let Some(crawled_services) = data.get_services() {
            let json = serde_json::to_string_pretty(crawled_services)?;
            tokio::fs::write(file_path, json).await?;
            debug!("Saved ping data to file: {file_path:?}");
        }
        Ok(())
    }

    #[cfg(feature = "native")]
    pub async fn load_ping_data_from_file(
        &self,
        file_path: &std::path::Path,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !file_path.exists() {
            return Ok(false);
        }

        let content = tokio::fs::read_to_string(file_path).await?;
        let crawled_services = serde_json::from_str(&content)?;
        *self.data.write().await = CrawledData::InitializedFromDefaults(crawled_services);
        info!("Loaded ping data from file: {file_path:?}");
        Ok(true)
    }

    pub async fn initialize_with_defaults(&self) {
        let loaded_data = self.loaded_data.read().await;
        let services = loaded_data
            .services
            .iter()
            .map(|(name, service)| {
                let instances = service
                    .instances
                    .iter()
                    .map(|instance| CrawledInstance {
                        url: instance.url.clone(),
                        status: CrawledInstanceStatus::Ok(Duration::ZERO),
                        tags: instance.tags.clone(),
                    })
                    .collect();
                (
                    name.clone(),
                    CrawledService {
                        name: name.clone(),
                        instances,
                    },
                )
            })
            .collect();

        *self.data.write().await = CrawledData::InitializedFromDefaults(CrawledServices {
            services,
            time: Utc::now(),
        });
        info!("Initialized crawler with default data from services.json");
    }

    #[inline]
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, CrawledData> {
        self.data.read().await
    }

    async fn crawl_single_instance(
        client: Arc<dyn InstanceClient>,
        config: Arc<CrawlerConfig>,
        proxies: Arc<ProxyData>,
        service: Arc<Service>,
        instance: Instance,
    ) -> Result<(CrawledInstance, String), CrawlerError> {
        let test_url = instance.url.join(&service.test_url)?;
        let status = match client
            .request(&config, &proxies, &service, &instance, test_url)
            .await?
        {
            InstanceRequest::Failed(status) => status,
            InstanceRequest::Response(response) => classify_response(&service, &instance, response),
        };

        let result = (
            CrawledInstance {
                url: instance.url,
                tags: instance.tags,
                status,
            },
            service.name.clone(),
        );
        debug!("Crawled instance: {result:?}");
        Ok(result)
    }

    async fn crawl<'a>(
        &self,
        crawler_guard: Option<MutexGuard<'a, ()>>,
    ) -> Result<(), CrawlerError> {
        let crawler_guard = match crawler_guard {
            Some(guard) => guard,
            None => {
                let Ok(guard) = self.crawler_lock.try_lock() else {
                    warn!("Crawler lock is already acquired, skipping crawl");
                    return Ok(());
                };
                guard
            }
        };

        let (mut crawled_services, jobs, proxies) = {
            let loaded_data = self.loaded_data.read().await;
            let crawled_services: HashMap<String, CrawledService> = loaded_data
                .services
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        CrawledService {
                            name: name.clone(),
                            instances: Vec::new(),
                        },
                    )
                })
                .collect();
            let jobs = loaded_data
                .services
                .values()
                .flat_map(|service| {
                    let service = Arc::new(service.clone());
                    service
                        .instances
                        .clone()
                        .into_iter()
                        .map(move |instance| (service.clone(), instance))
                })
                .collect::<Vec<_>>();
            (
                crawled_services,
                jobs,
                Arc::new(loaded_data.proxies.clone()),
            )
        };

        let results = stream::iter(jobs)
            .map(|(service, instance)| {
                Self::crawl_single_instance(
                    self.client.clone(),
                    self.config.clone(),
                    proxies.clone(),
                    service,
                    instance,
                )
            })
            .buffer_unordered(self.config.max_concurrent_requests)
            .collect::<Vec<_>>()
            .await;

        for result in results {
            let (crawled_instance, name) = match result {
                Ok(result) => result,
                Err(error) => {
                    error!("Error occurred during crawling: {error}");
                    continue;
                }
            };
            crawled_services
                .get_mut(&name)
                .expect("service exists in crawler map")
                .instances
                .push(crawled_instance);
        }

        *self.data.write().await = CrawledData::CrawledServices(CrawledServices {
            services: crawled_services,
            time: Utc::now(),
        });
        debug!("Finished crawl");
        drop(crawler_guard);
        Ok(())
    }

    pub async fn crawl_once(&self) -> Result<(), CrawlerError> {
        self.crawl(None).await
    }

    #[cfg(feature = "native")]
    pub async fn update_crawl(
        &self,
        save_ping_data: Option<&std::path::Path>,
    ) -> Result<(), CrawlerError> {
        let crawler_guard = self.crawler_lock.lock().await;
        self.data.write().await.make_reloading();
        self.crawl(Some(crawler_guard)).await?;
        if let Some(file_path) = save_ping_data
            && let Err(error) = self.save_ping_data_to_file(file_path).await
        {
            error!("Failed to save ping data to file: {error}");
        }
        Ok(())
    }

    #[cfg(feature = "native")]
    pub async fn crawler_loop(&self, save_ping_data: Option<&std::path::Path>) {
        loop {
            debug!("Starting crawl");
            if let Err(error) = self.crawl_once().await {
                error!("Error occurred during crawl loop: {error}");
            } else if let Some(file_path) = save_ping_data
                && let Err(error) = self.save_ping_data_to_file(file_path).await
            {
                error!("Failed to save ping data to file: {error}");
            }
            debug!("Next crawl will start in {:?}", self.config.ping_interval);
            tokio::time::sleep(self.config.ping_interval).await;
        }
    }
}

pub fn should_read_response_body(service: &Service, instance: &Instance, status_code: u16) -> bool {
    service.search_string.is_some()
        && service.allowed_http_codes.is_allowed(status_code)
        && !instance.tags.iter().any(|tag| tag == "antibot")
}

fn classify_response(
    service: &Service,
    instance: &Instance,
    response: InstanceResponse,
) -> CrawledInstanceStatus {
    if instance.tags.iter().any(|tag| tag == "antibot") {
        debug!(
            "Skipping response checks for antibot instance: {}",
            instance.url
        );
        CrawledInstanceStatus::Ok(response.duration)
    } else if !service.allowed_http_codes.is_allowed(response.status_code) {
        CrawledInstanceStatus::InvalidStatusCode(response.status_code, response.duration)
    } else if service.search_string.as_ref().is_some_and(|search_string| {
        !response
            .body
            .as_deref()
            .unwrap_or_default()
            .contains(search_string)
    }) {
        CrawledInstanceStatus::StringNotFound
    } else {
        CrawledInstanceStatus::Ok(response.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastside_shared::serde_types::AllowedHttpCodes;

    fn service(name: &str, hosts: &[&str]) -> Service {
        Service {
            name: name.into(),
            test_url: "/".into(),
            fallback: None,
            follow_redirects: false,
            allowed_http_codes: AllowedHttpCodes {
                codes: vec![200],
                inclusive_ranges: Vec::new(),
                exclusive_ranges: Vec::new(),
            },
            search_string: None,
            regexes: Vec::new(),
            aliases: Vec::new(),
            source_link: None,
            deprecated_message: None,
            instances: hosts
                .iter()
                .map(|host| Instance {
                    url: Url::parse(&format!("https://{host}/")).unwrap(),
                    tags: Vec::new(),
                })
                .collect(),
        }
    }

    fn response(status_code: u16, body: Option<&str>) -> InstanceResponse {
        InstanceResponse {
            status_code,
            duration: Duration::from_millis(42),
            body: body.map(str::to_owned),
        }
    }

    #[test]
    fn response_checks_keep_existing_order() {
        let mut service = service("demo", &[]);
        service.search_string = Some("expected".into());
        let mut instance = Instance {
            url: Url::parse("https://demo.example/").unwrap(),
            tags: Vec::new(),
        };
        let ok = CrawledInstanceStatus::Ok(Duration::from_millis(42));

        assert!(should_read_response_body(&service, &instance, 200));
        assert!(!should_read_response_body(&service, &instance, 503));
        assert_eq!(
            classify_response(&service, &instance, response(200, Some("expected"))),
            ok
        );
        assert_eq!(
            classify_response(&service, &instance, response(200, Some("other"))),
            CrawledInstanceStatus::StringNotFound
        );
        assert_eq!(
            classify_response(&service, &instance, response(503, None)),
            CrawledInstanceStatus::InvalidStatusCode(503, Duration::from_millis(42))
        );

        instance.tags.push("antibot".into());
        assert!(!should_read_response_body(&service, &instance, 200));
        assert_eq!(
            classify_response(&service, &instance, response(503, None)),
            ok
        );
    }

    #[test]
    fn instance_batch_crosses_service_boundaries_in_stable_order() {
        let services = ServicesData::from([
            (
                "zeta".into(),
                service("zeta", &["z1.example", "z2.example"]),
            ),
            (
                "alpha".into(),
                service("alpha", &["a1.example", "a2.example"]),
            ),
        ]);

        let (batch, count) = select_instance_batch(&services, 1, 2);

        assert_eq!(count, 2);
        assert_eq!(
            batch["alpha"].instances[0].url.host_str(),
            Some("a2.example")
        );
        assert_eq!(
            batch["zeta"].instances[0].url.host_str(),
            Some("z1.example")
        );
        assert_eq!(select_instance_batch(&services, 4, 2).1, 0);
    }
}
