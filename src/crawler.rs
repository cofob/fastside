use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rand::seq::SliceRandom;
use reqwest::StatusCode;
use thiserror::Error;
use tokio::{sync::RwLock, task::JoinSet, time::sleep};
use url::Url;

use crate::{
    config::CrawlerConfig,
    serde_types::{Service, Services},
};

#[derive(Error, Debug)]
pub enum CrawlerError {
    #[error("crawler not fetched data yet")]
    CrawlerNotFetchedYet,
    #[error("service not found")]
    ServiceNotFound,
    #[error("url error: `{0}`")]
    UrlError(#[from] url::ParseError),
    #[error("request error: `{0}`")]
    RequestError(#[from] reqwest::Error),
}

#[derive(Clone, Debug)]
pub enum CrawledInstanceStatus {
    Ok(Duration),
    TimedOut,
    Unknown,
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
}

#[derive(Clone, Debug)]
pub struct CrawledService {
    pub name: String,
    pub fallback_url: Url,
    pub aliases: HashSet<String>,
    pub instances: Vec<CrawledInstance>,
}

impl CrawledService {
    fn get_alive_instances(&self) -> Vec<&CrawledInstance> {
        self.instances
            .iter()
            .filter(|s| matches!(&s.status, CrawledInstanceStatus::Ok(_)))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct CrawledServices(pub Vec<CrawledService>);

impl CrawledServices {
    fn get_service_by_alias(&self, alias: &str) -> Option<&CrawledService> {
        self.0
            .iter()
            .find(|&service| service.aliases.contains(alias))
    }
}

#[derive(Debug)]
pub struct Crawler {
    services: Arc<Services>,
    config: CrawlerConfig,
    client: reqwest::Client,
    data: RwLock<Option<CrawledServices>>,
}

impl Crawler {
    pub fn new(services: Arc<Services>, config: CrawlerConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("failed to build http client");
        Self {
            services,
            config,
            client,
            data: RwLock::new(None),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<Option<CrawledServices>> {
        self.data.read().await
    }

    pub async fn get_redirect_url_for_service(
        &self,
        alias: &str,
        path: &str,
    ) -> Result<Url, CrawlerError> {
        let guard = self.data.read().await;
        let data = guard.as_ref();
        let Some(services) = data else {
            return Err(CrawlerError::CrawlerNotFetchedYet)?;
        };
        let Some(service) = services.get_service_by_alias(alias) else {
            return Err(CrawlerError::ServiceNotFound)?;
        };
        let instances = service.get_alive_instances();
        match instances.choose(&mut rand::thread_rng()) {
            Some(instance) => Ok(instance.url.join(path)?),
            None => Ok(service.fallback_url.join(path)?),
        }
    }

    pub async fn get_redirect_urls_for_service(
        &self,
        alias: &str,
    ) -> Result<Vec<Url>, CrawlerError> {
        let guard = self.data.read().await;
        let data = guard.as_ref();
        let Some(services) = data else {
            return Err(CrawlerError::CrawlerNotFetchedYet)?;
        };
        let Some(service) = services.get_service_by_alias(alias) else {
            return Err(CrawlerError::ServiceNotFound)?;
        };
        Ok(service
            .get_alive_instances()
            .iter()
            .map(|i| i.url.clone())
            .collect())
    }

    async fn crawl_single_instance(
        service: &Service,
        instance: &Url,
        client: &reqwest::Client,
    ) -> Result<CrawledInstance, CrawlerError> {
        let test_url = instance.join(&service.test_url)?;
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let response = client.get(test_url).send().await;
        let status = match response {
            Ok(response) => {
                let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                match response.status() {
                    StatusCode::OK => CrawledInstanceStatus::Ok(end - start),
                    _ => CrawledInstanceStatus::Unknown,
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    CrawledInstanceStatus::TimedOut
                } else {
                    CrawledInstanceStatus::Unknown
                }
            }
        };
        Ok(CrawledInstance {
            url: instance.clone(),
            status,
        })
    }

    async fn crawl_single_service(
        client: reqwest::Client,
        service: Service,
    ) -> Result<CrawledService, CrawlerError> {
        debug!("Crawling service {}", service.name);
        let mut crawled_instances: Vec<CrawledInstance> =
            Vec::with_capacity(service.instances.len());

        for instance in &service.instances {
            let crawled_instance = match Crawler::crawl_single_instance(&service, instance, &client)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    error!(
                        "Failed to crawl instance {instance} of service {service_name} due to error {e}",
                        service_name = service.name
                    );
                    continue;
                }
            };
            crawled_instances.push(crawled_instance);
        }

        Ok(CrawledService {
            name: service.name.clone(),
            fallback_url: service.fallback.clone(),
            aliases: service.aliases.clone(),
            instances: crawled_instances,
        })
    }

    async fn crawl(&self) -> Result<(), CrawlerError> {
        let mut crawled_services: Vec<CrawledService> = Vec::with_capacity(self.services.len());
        let mut crawl_tasks = JoinSet::<Result<CrawledService, CrawlerError>>::new();
        for service in self.services.as_ref().values() {
            crawl_tasks.spawn(Crawler::crawl_single_service(
                self.client.clone(),
                service.clone(),
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
            crawled_services.push(crawled_service);
        }

        let mut data = self.data.write().await;
        if data.is_none() {
            info!("Finished initial crawl, we are ready to serve requests");
        }
        data.replace(CrawledServices(crawled_services));

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
