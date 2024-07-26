use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use thiserror::Error;
use tokio::{sync::RwLock, task::JoinSet, time::sleep};
use url::Url;

use crate::{
    config::CrawlerConfig,
    serde_types::{Instance, LoadedData, Service},
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
    pub fn get_alive_instances(&self) -> Vec<&CrawledInstance> {
        self.instances
            .iter()
            .filter(|s| matches!(&s.status, CrawledInstanceStatus::Ok(_)))
            .collect()
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
        service: &Service,
        instance: &Instance,
    ) -> Result<CrawledInstance, CrawlerError> {
        let redirect_policy = if service.follow_redirects {
            reqwest::redirect::Policy::default()
        } else {
            reqwest::redirect::Policy::none()
        };
        let mut client_builder = Client::builder()
            .connect_timeout(config.request_timeout)
            .read_timeout(config.request_timeout)
            .redirect(redirect_policy);

        let proxy_name: Option<String> = {
            let mut val: Option<String> = None;
            for proxy in loaded_data.proxies.keys() {
                if instance.tags.contains(proxy) {
                    val = Some(proxy.clone());
                    break;
                }
            }
            val
        };
        if let Some(proxy_name) = proxy_name {
            let proxy_config = loaded_data.proxies.get(&proxy_name).unwrap();
            let proxy = {
                let mut builder = reqwest::Proxy::all(&proxy_config.url)?;
                if let Some(auth) = &proxy_config.auth {
                    builder = builder.basic_auth(&auth.username, &auth.password);
                }
                builder
            };
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder.build().unwrap();

        let test_url = instance.url.join(&service.test_url)?;
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let response = client.get(test_url).send().await;
        let status = match response {
            Ok(response) => {
                let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                let status_code = response.status().as_u16();
                let mut status_valid = false;
                match status_code {
                    200 => status_valid = true,
                    300..=399 => {
                        if service.allow_3xx {
                            status_valid = true;
                        }
                    }
                    400..=499 => {
                        if service.allow_4xx {
                            status_valid = true;
                        }
                    }
                    500..=599 => {
                        if service.allow_5xx {
                            status_valid = true;
                        }
                    }
                    _ => {}
                }
                if status_valid {
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
        Ok(CrawledInstance {
            url: instance.url.clone(),
            tags: instance.tags.clone(),
            status,
        })
    }

    async fn crawl_single_service(
        config: Arc<CrawlerConfig>,
        service: Service,
        loaded_data: Arc<LoadedData>,
    ) -> Result<CrawledService, CrawlerError> {
        debug!("Crawling service {}", service.name);
        let mut crawled_instances: Vec<CrawledInstance> =
            Vec::with_capacity(service.instances.len());

        for instance in &service.instances {
            let crawled_instance = match Crawler::crawl_single_instance(
                config.clone(),
                loaded_data.clone(),
                &service,
                instance,
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    error!(
                        "Failed to crawl instance {instance} of service {service_name} due to error {e}",
                        service_name = service.name,
                        instance = instance.url,
                    );
                    continue;
                }
            };
            crawled_instances.push(crawled_instance);
        }

        Ok(CrawledService {
            name: service.name.clone(),
            instances: crawled_instances,
        })
    }

    async fn crawl(&self) -> Result<(), CrawlerError> {
        let mut crawled_services: HashMap<String, CrawledService> =
            HashMap::with_capacity(self.loaded_data.services.len());
        let mut crawl_tasks = JoinSet::<Result<CrawledService, CrawlerError>>::new();
        for service in self.loaded_data.services.values() {
            crawl_tasks.spawn(Crawler::crawl_single_service(
                self.config.clone(),
                service.clone(),
                self.loaded_data.clone(),
            ));
        }

        while let Some(crawled_service_result) = crawl_tasks.join_next().await {
            let Ok(result) = crawled_service_result else {
                debug!("failed to join handle");
                continue;
            };
            let crawled_service = match result {
                Ok(c) => {
                    debug!("Crawled service {}", c.name);
                    c
                }
                Err(e) => {
                    error!("Failed to crawl service due to error {e}");
                    continue;
                }
            };
            crawled_services.insert(crawled_service.name.clone(), crawled_service);
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
