use tokio::sync::RwLockReadGuard;

use crate::{
    crawler::{CrawledInstance, CrawledService, CrawledServices},
    serde_types::{Service, ServicesData},
};
use rand::seq::SliceRandom;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SearchError {
    #[error("crawler data not fetched yet, try again later")]
    CrawlerNotFetchedYet,
    #[error("service not found")]
    ServiceNotFound,
    #[error("instance not found")]
    InstanceNotFound,
}

pub async fn find_redirect_service_by_name<'a>(
    guard: &'a RwLockReadGuard<'a, Option<CrawledServices>>,
    services: &'a ServicesData,
    query: &str,
) -> Result<(&'a CrawledService, &'a Service), SearchError> {
    let data = match guard.as_ref() {
        Some(data) => data,
        None => return Err(SearchError::CrawlerNotFetchedYet),
    };
    // Search for the service by name.
    if data.services.contains_key(query) {
        return Ok((&data.services[query], &services[query]));
    };
    // Search for the service by aliases.
    for (service_name, crawled_service) in data.services.iter() {
        for service in services.values() {
            if service.aliases.contains(service_name) {
                return Ok((crawled_service, service));
            };
        }
    }

    Err(SearchError::ServiceNotFound)
}

pub fn get_redirect_instances<'a>(
    crawled_service: &'a CrawledService,
    required_tags: &[String],
) -> Result<Vec<&'a CrawledInstance>, SearchError> {
    let alive_instances = crawled_service.get_alive_instances();
    let instances = alive_instances
        .iter()
        .filter(|i| required_tags.iter().all(|tag| i.tags.contains(tag)))
        .cloned()
        .collect::<Vec<_>>();
    if instances.is_empty() {
        return Err(SearchError::InstanceNotFound);
    }
    Ok(instances)
}

pub fn get_redirect_random_instance(
    crawled_service: &CrawledService,
    required_tags: &[String],
) -> Result<CrawledInstance, SearchError> {
    let instances = get_redirect_instances(crawled_service, required_tags)?;
    let instance = instances.choose(&mut rand::thread_rng()).unwrap();
    Ok(instance.to_owned().to_owned().to_owned()) // wtf is happening here
}
