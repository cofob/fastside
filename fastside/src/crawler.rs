use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use thiserror::Error;
use tokio::{sync::RwLock, time::sleep};
use url::Url;

use crate::{config::CrawlerConfig, types::LoadedData, utils::parallel::Parallelise};
use fastside_shared::{
    client_builder::build_client,
    serde_types::{HttpCodeRanges, Instance, Service},
};

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
pub struct Crawler {
    loaded_data: Arc<LoadedData>,
    config: Arc<CrawlerConfig>,
    data: RwLock<Option<CrawledServices>>,
}

impl Crawler {
    pub fn new(loaded_data: Arc<LoadedData>, config: CrawlerConfig) -> Self {
        Self {
            loaded_data,
            config: Arc::new(config),
            data: RwLock::new(None),
        }
    }

    #[inline]
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<Option<CrawledServices>> {
        self.data.read().await
    }

    async fn crawl_single_instance(
        config: Arc<CrawlerConfig>,
        loaded_data: Arc<LoadedData>,
        service: Arc<Service>,
        instance: Instance,
    ) -> Result<(CrawledInstance, String), CrawlerError> {
        let client = build_client(
            service.as_ref(),
            config.as_ref(),
            &loaded_data.proxies,
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
            Err(e) => {
                if e.is_timeout() {
                    CrawledInstanceStatus::TimedOut
                } else if e.is_builder() {
                    CrawledInstanceStatus::BuilderError
                } else if e.is_redirect() {
                    CrawledInstanceStatus::RedirectPolicyError
                } else if e.is_request() {
                    CrawledInstanceStatus::RequestError
                } else if e.is_body() {
                    CrawledInstanceStatus::BodyError
                } else if e.is_decode() {
                    CrawledInstanceStatus::DecodeError
                } else if e.is_connect() {
                    CrawledInstanceStatus::ConnectionError
                } else {
                    CrawledInstanceStatus::Unknown
                }
            }
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

    async fn crawl(&self) -> Result<(), CrawlerError> {
        let mut crawled_services: HashMap<String, CrawledService> = self
            .loaded_data
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
        let mut parallelise = Parallelise::with_capacity(self.config.max_concurrent_requests);

        for service in self.loaded_data.services.values() {
            let service = Arc::new(service.clone());
            for instance in &service.instances {
                let loaded_data = self.loaded_data.clone();
                let config = self.config.clone();
                let instance = instance.clone();
                parallelise
                    .push(tokio::spawn(Self::crawl_single_instance(
                        config,
                        loaded_data,
                        service.clone(),
                        instance,
                    )))
                    .await;
            }
        }

        let results = parallelise.wait().await;

        for result in results {
            let (crawled_instance, name) = match result {
                Ok(c) => c,
                Err(e) => {
                    error!("Error occured during crawling: {e}");
                    continue;
                }
            };
            crawled_services
                .get_mut(&name)
                .unwrap()
                .instances
                .push(crawled_instance.clone());
        }

        let mut data = self.data.write().await;
        if data.is_none() {
            info!("Finished initial crawl, we are ready to serve requests");
        }
        data.replace(CrawledServices {
            services: crawled_services,
            time: Utc::now(),
        });

        Ok(())
    }

    pub async fn crawler_loop(&self) {
        loop {
            debug!("Starting crawl");
            if let Err(e) = self.crawl().await {
                error!("Error occured during crawl loop: {e}");
            };
            debug!("Next crawl will start in {:?}", self.config.ping_interval);
            sleep(self.config.ping_interval).await;
        }
    }
}
