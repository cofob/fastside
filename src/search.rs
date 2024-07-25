use std::{collections::HashMap, time::Duration};

use regex::Regex;
use tokio::sync::RwLockReadGuard;

use crate::{
    crawler::{CrawledInstance, CrawledInstanceStatus, CrawledService, CrawledServices},
    serde_types::{SelectMethod, Service, ServicesData, UserConfig},
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
    let query_string = query.to_string();
    for crawled_service in data.services.values() {
        for service in services.values() {
            if service.aliases.contains(&query_string) {
                return Ok((crawled_service, service));
            };
        }
    }

    Err(SearchError::ServiceNotFound)
}

pub async fn find_redirect_service_by_url<'a>(
    guard: &'a RwLockReadGuard<'a, Option<CrawledServices>>,
    services: &'a ServicesData,
    regexes: &'a HashMap<String, Regex>,
    query: &str,
) -> Result<(&'a CrawledService, &'a Service, String), SearchError> {
    let data = match guard.as_ref() {
        Some(data) => data,
        None => return Err(SearchError::CrawlerNotFetchedYet),
    };
    // Search for the service by regexes.
    for (service_name, service) in services.iter() {
        if let Some(regex) = regexes.get(service_name) {
            let captures = regex.captures(query);
            if let Some(captures) = captures {
                let redir_path = match captures.get(service.regex_group) {
                    Some(path) => path.as_str().to_string(),
                    None => continue,
                };
                return Ok((&data.services[service_name], service, redir_path));
            }
        }
    }

    Err(SearchError::ServiceNotFound)
}

pub fn get_redirect_instances<'a>(
    crawled_service: &'a CrawledService,
    required_tags: &[String],
    forbidden_tags: &[String],
) -> Result<Vec<&'a CrawledInstance>, SearchError> {
    let alive_instances = crawled_service.get_alive_instances();
    let instances = alive_instances
        .iter()
        .filter(|i| required_tags.iter().all(|tag| i.tags.contains(tag)))
        .filter(|i| forbidden_tags.iter().all(|tag| !i.tags.contains(tag)))
        .cloned()
        .collect::<Vec<_>>();
    if instances.is_empty() {
        return Err(SearchError::InstanceNotFound);
    }
    Ok(instances)
}

const MAX_DURATION: Duration = Duration::from_secs(std::u64::MAX);

pub fn get_redirect_instance(
    crawled_service: &CrawledService,
    user_config: &UserConfig,
) -> Result<CrawledInstance, SearchError> {
    let instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
    )?;
    let instance = match &user_config.select_method {
        SelectMethod::Random => instances.choose(&mut rand::thread_rng()).unwrap(),
        SelectMethod::LowPing => instances
            .iter()
            .min_by_key(|i| match i.status {
                CrawledInstanceStatus::Ok(ping) => ping,
                _ => MAX_DURATION,
            })
            .unwrap(),
    };
    Ok(instance.to_owned().to_owned().to_owned()) // wtf is happening here
}
