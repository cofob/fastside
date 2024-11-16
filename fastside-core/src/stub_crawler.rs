use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use thiserror::Error;
use tokio::{
    sync::{Mutex, MutexGuard, RwLock},
    time::sleep,
};
use url::Url;

use fastside_shared::config::CrawlerConfig;
use fastside_shared::{
    client_builder::build_client,
    parallel::Parallelise,
    serde_types::{HttpCodeRanges, Instance, Service},
};

use crate::types::LoadedData;

#[derive(Error, Debug)]
pub enum CrawlerError {
    #[error("url error: `{0}`")]
    UrlError(#[from] url::ParseError),
    #[error("request error: `{0}`")]
    RequestError(#[from] reqwest::Error),
}

#[derive(Clone, Debug)]
pub enum CrawledInstanceStatus {
    Ok(Duration),
    #[allow(dead_code)]
    InvalidStatusCode(StatusCode, Duration),
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
    /// Used for sorting values in index.html template.
    pub fn as_isize(&self) -> isize {
        match self {
            Self::Ok(d) => d.as_millis() as isize,
            _ => isize::MAX,
        }
    }
}

impl std::fmt::Display for CrawledInstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Clone, Debug)]
pub struct CrawledInstance {
    pub url: Url,
    pub status: CrawledInstanceStatus,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CrawledService {
    pub name: String,
    pub instances: Vec<CrawledInstance>,
}

impl CrawledService {
    pub fn get_alive_instances(&self) -> impl Iterator<Item = &CrawledInstance> {
        self.instances
            .iter()
            .filter(|s| matches!(&s.status, CrawledInstanceStatus::Ok(_)))
    }
}

#[derive(Clone, Debug)]
pub struct CrawledServices {
    pub services: HashMap<String, CrawledService>,
    pub time: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CrawledData {
    CrawledServices(CrawledServices),
    InitialLoading,
    ReloadingServices(CrawledServices),
}

impl CrawledData {
    pub fn get_services(&self) -> Option<&CrawledServices> {
        match self {
            Self::CrawledServices(s) => Some(s),
            Self::InitialLoading => None,
            Self::ReloadingServices(current) => Some(current),
        }
    }

    pub fn is_reloading(&self) -> bool {
        matches!(self, Self::ReloadingServices { .. })
    }

    pub fn replace(&mut self, new: CrawledData) {
        *self = new;
    }

    pub fn make_reloading(&mut self) {
        let current = match self {
            Self::CrawledServices(s) => s.clone(),
            _ => return,
        };
        *self = Self::ReloadingServices(current);
    }
}

impl AsRef<CrawledData> for CrawledData {
    fn as_ref(&self) -> &CrawledData {
        self
    }
}

#[derive(Debug)]
pub struct Crawler {
    loaded_data: Arc<RwLock<LoadedData>>,
    config: Arc<CrawlerConfig>,
    data: RwLock<CrawledData>,
    crawler_lock: Mutex<()>,
}

impl Crawler {
    pub fn new(loaded_data: Arc<RwLock<LoadedData>>, config: CrawlerConfig) -> Self {
        Self {
            loaded_data,
            config: Arc::new(config),
            data: RwLock::new(CrawledData::InitialLoading),
            crawler_lock: Mutex::new(()),
        }
    }

    #[inline]
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<CrawledData> {
        self.data.read().await
    }

    async fn crawl_single_instance(
        config: Arc<CrawlerConfig>,
        loaded_data: Arc<RwLock<LoadedData>>,
        service: Arc<Service>,
        instance: Instance,
    ) -> Result<(CrawledInstance, String), CrawlerError> {
        let client = build_client(
            service.as_ref(),
            config.as_ref(),
            &loaded_data.read().await.proxies,
            &instance,
        )?;

        let test_url = instance.url.join(&service.test_url)?;
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let response = client.get(test_url).send().await;
        let status = match response {
            Ok(response) => {
                let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                let status_code = response.status().as_u16();
                if service.allowed_http_codes.is_allowed(status_code) {
                    if let Some(search_string) = &service.search_string {
                        let body = response.text().await?;
                        if !body.contains(search_string) {
                            CrawledInstanceStatus::StringNotFound
                        } else {
                            CrawledInstanceStatus::Ok(end - start)
                        }
                    } else {
                        CrawledInstanceStatus::Ok(end - start)
                    }
                } else {
                    CrawledInstanceStatus::InvalidStatusCode(response.status(), end - start)
                }
            }
            Err(e) => match e {
                _ if e.is_timeout() => CrawledInstanceStatus::TimedOut,
                _ if e.is_builder() => CrawledInstanceStatus::BuilderError,
                _ if e.is_redirect() => CrawledInstanceStatus::RedirectPolicyError,
                _ if e.is_request() => CrawledInstanceStatus::RequestError,
                _ if e.is_body() => CrawledInstanceStatus::BodyError,
                _ if e.is_decode() => CrawledInstanceStatus::DecodeError,
                #[cfg(not(target_arch = "wasm32"))]
                _ if e.is_connect() => CrawledInstanceStatus::ConnectionError,
                _ => CrawledInstanceStatus::Unknown,
            },
        };

        let ret = (
            CrawledInstance {
                url: instance.url.clone(),
                tags: instance.tags.clone(),
                status,
            },
            service.name.clone(),
        );
        debug!("Crawled instance: {ret:?}");
        Ok(ret)
    }

    async fn crawl<'a>(
        &self,
        crawler_guard: Option<MutexGuard<'a, ()>>,
    ) -> Result<(), CrawlerError> {
        let crawler_guard = match crawler_guard {
            Some(guard) => guard,
            None => {
                let Ok(crawler_guard) = self.crawler_lock.try_lock() else {
                    warn!("Crawler lock is already acquired, skipping crawl");
                    return Ok(());
                };
                crawler_guard
            }
        };

        let mut crawled_services: HashMap<String, CrawledService> = self
            .loaded_data
            .read()
            .await
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

        for service in self.loaded_data.read().await.services.values() {
            let service = Arc::new(service.clone());
            for instance in &service.instances {
                let loaded_data = self.loaded_data.clone();
                let config = self.config.clone();
                let instance = instance.clone();
            }
        }

        let mut data = self.data.write().await;
        data.replace(CrawledData::CrawledServices(CrawledServices {
            services: crawled_services,
            time: Utc::now(),
        }));

        match data.as_ref() {
            CrawledData::ReloadingServices { .. } => {
                info!("Finished reloading services");
            }
            CrawledData::InitialLoading => {
                info!("Finished initial crawl, we are ready to serve requests");
            }
            CrawledData::CrawledServices(_) => {
                debug!("Finished crawl");
            }
        }

        drop(crawler_guard);
        Ok(())
    }

    /// Run crawler instantly in update loaded_data mode.
    pub async fn update_crawl(&self) -> Result<(), CrawlerError> {
        let crawler_guard = self.crawler_lock.lock().await;
        let mut data = self.data.write().await;
        data.make_reloading();
        drop(data);
        self.crawl(Some(crawler_guard)).await
    }

    pub async fn crawler_loop(&self) {
        loop {
            debug!("Starting crawl");
            if let Err(e) = self.crawl(None).await {
                error!("Error occured during crawl loop: {e}");
            };
            debug!("Next crawl will start in {:?}", self.config.ping_interval);
            sleep(self.config.ping_interval).await;
        }
    }
}
