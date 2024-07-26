use std::time::Duration;

use regex::Captures;
use tokio::sync::RwLockReadGuard;

use crate::{
    crawler::{CrawledInstance, CrawledInstanceStatus, CrawledService, CrawledServices},
    serde_types::{Regexes, SelectMethod, Service, ServicesData, UserConfig},
};
use rand::seq::SliceRandom;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SearchError {
    #[error("crawler data not fetched yet, try again later")]
    CrawlerNotFetchedYet,
    #[error("service not found")]
    ServiceNotFound,
    #[error("no instances found")]
    NoInstancesFound,
    #[error("replace args error: `{0}`")]
    ReplaceArgsError(#[from] ReplaceArgsError),
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

#[derive(Error, Debug)]
pub enum ReplaceArgsError {
    #[error("invalid capture group number")]
    InvalidCaptureGroup,
    #[error("parse int error")]
    ParseIntError(#[from] std::num::ParseIntError),
}

fn replace_args_in_url(url: &str, captures: Captures) -> Result<String, ReplaceArgsError> {
    let mut out = String::new();
    let mut is_encoded = false;
    let mut is_arg = false;
    let mut num = String::new();
    for c in url.chars() {
        match (c, is_arg, is_encoded) {
            ('?', false, false) => {
                debug!("Found '?' in URL");
                is_encoded = true;
            }
            ('$', false, _) => {
                debug!("Found '$' in URL");
                is_arg = true;
            }
            ('0'..='9', true, _) => {
                debug!("Found digit {c} in URL");
                num.push(c);
            }
            (c, true, _) if num.is_empty() => {
                debug!("Found non-digit {c} in URL while parsing arg");
                is_arg = false;
                is_encoded = false;
                out.push('$');
                out.push(c);
            }
            (c, false, true) => {
                debug!("Found non-dollar {c} in URL while expecting arg");
                is_encoded = false;
                out.push('?');
                out.push(c);
            }
            (_, true, _) => {
                debug!("Found non-digit {c} in URL while parsing num, adding capture");
                let arg = captures
                    .get(num.parse()?)
                    .ok_or(ReplaceArgsError::InvalidCaptureGroup)?
                    .as_str();
                let arg = if is_encoded {
                    urlencoding::encode(arg).to_string()
                } else {
                    arg.to_string()
                };
                out.push_str(&arg);
                is_arg = false;
                is_encoded = false;
                num.clear();
            }
            _ => {
                debug!("Found non-dollar {c} in URL while not expecting arg");
                out.push(c);
            }
        }
    }
    if is_arg {
        debug!("Found EOF while parsing arg, adding capture");
        let arg = captures
            .get(num.parse()?)
            .ok_or(ReplaceArgsError::InvalidCaptureGroup)?
            .as_str();
        let arg = if is_encoded {
            urlencoding::encode(arg).to_string()
        } else {
            arg.to_string()
        };
        out.push_str(&arg);
    }
    Ok(out)
}

pub async fn find_redirect_service_by_url<'a>(
    guard: &'a RwLockReadGuard<'a, Option<CrawledServices>>,
    services: &'a ServicesData,
    regexes: &'a Regexes,
    query: &str,
) -> Result<(&'a CrawledService, &'a Service, String), SearchError> {
    let data = match guard.as_ref() {
        Some(data) => data,
        None => return Err(SearchError::CrawlerNotFetchedYet),
    };
    // Search for the service by regexes.
    for (service_name, service) in services.iter() {
        if let Some(service_regexes) = regexes.get(service_name) {
            for service_regex in service_regexes {
                let regex = &service_regex.regex;
                let captures = regex.captures(query);
                if let Some(captures) = captures {
                    let url = service_regex.url.clone();
                    let url = replace_args_in_url(&url, captures)?;
                    return Ok((&data.services[service_name], service, url));
                }
            }
        }
    }

    Err(SearchError::ServiceNotFound)
}

pub fn get_redirect_instances<'a>(
    crawled_service: &'a CrawledService,
    required_tags: &[String],
    forbidden_tags: &[String],
) -> Option<Vec<&'a CrawledInstance>> {
    let alive_instances = crawled_service.get_alive_instances();
    let instances = alive_instances
        .iter()
        .filter(|i| required_tags.iter().all(|tag| i.tags.contains(tag)))
        .filter(|i| forbidden_tags.iter().all(|tag| !i.tags.contains(tag)))
        .cloned()
        .collect::<Vec<_>>();
    if instances.is_empty() {
        return None;
    }
    Some(instances)
}

const MAX_DURATION: Duration = Duration::from_secs(std::u64::MAX);

pub fn get_redirect_instance(
    crawled_service: &CrawledService,
    service: &Service,
    user_config: &UserConfig,
) -> Result<(CrawledInstance, bool), SearchError> {
    let instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
    );
    match &instances {
        None => Ok((
            CrawledInstance {
                url: service.fallback.clone(),
                status: CrawledInstanceStatus::Ok(MAX_DURATION),
                tags: vec![],
            },
            true,
        )),
        Some(instances) => Ok((
            match &user_config.select_method {
                SelectMethod::Random => instances
                    .choose(&mut rand::thread_rng())
                    .unwrap()
                    .to_owned()
                    .to_owned(),
                SelectMethod::LowPing => instances
                    .iter()
                    .min_by_key(|i| match i.status {
                        CrawledInstanceStatus::Ok(ping) => ping,
                        _ => MAX_DURATION,
                    })
                    .unwrap()
                    .to_owned()
                    .to_owned(),
            },
            false,
        )),
    }
}
