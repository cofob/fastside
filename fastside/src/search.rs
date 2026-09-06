use std::time::Duration;

use regex::Captures;
use tokio::sync::RwLockReadGuard;

use crate::{
    crawler::{CrawledData, CrawledInstance, CrawledInstanceStatus, CrawledService},
    storage::{InstanceReputation, StateStore},
    types::Regexes,
};
use fastside_shared::{
    config::{ReputationConfig, SelectMethod, UserConfig},
    serde_types::{Service, ServicesData},
};
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
    guard: &'a RwLockReadGuard<'a, CrawledData>,
    services: &'a ServicesData,
    query: &str,
) -> Result<(&'a CrawledService, &'a Service), SearchError> {
    let data = match guard.get_services() {
        Some(data) => data,
        None => return Err(SearchError::CrawlerNotFetchedYet),
    };

    // Search for the service by name.
    if data.services.contains_key(query) {
        return Ok((&data.services[query], &services[query]));
    };

    // Search for the service by aliases.
    let query_string = query.to_string();
    let found_service: Option<&Service> = services
        .values()
        .find(|service| service.aliases.contains(&query_string));
    if let Some(service) = found_service {
        return Ok((&data.services[&service.name], service));
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

fn push_capture(
    captures: &Captures,
    num: usize,
    is_encoded: bool,
    out: &mut String,
) -> Result<(), ReplaceArgsError> {
    let arg = captures
        .get(num)
        .ok_or(ReplaceArgsError::InvalidCaptureGroup)?
        .as_str();
    let arg = if is_encoded {
        urlencoding::encode(arg).to_string()
    } else {
        arg.to_string()
    };
    out.push_str(&arg);
    Ok(())
}

fn replace_args_in_url(url: &str, captures: Captures) -> Result<String, ReplaceArgsError> {
    let mut out = String::with_capacity(url.len());
    let mut is_encoded = false;
    let mut is_arg = false;
    let mut escape = false;
    let mut num = String::new();
    for c in url.chars() {
        match (c, is_arg, is_encoded, escape) {
            ('\\', false, _, false) => {
                debug!("Found '\\' in URL, escaping next character");
                escape = true;
            }
            ('?', false, false, false) => {
                debug!("Found '?' in URL");
                is_encoded = true;
            }
            ('$', false, _, false) => {
                debug!("Found '$' in URL");
                is_arg = true;
            }
            ('0'..='9', true, _, false) => {
                debug!("Found digit {c} in URL");
                num.push(c);
            }
            (c, true, _, false) if num.is_empty() => {
                debug!("Found non-digit {c} in URL while parsing arg");
                is_arg = false;
                is_encoded = false;
                out.push('$');
                out.push(c);
            }
            (c, false, true, false) => {
                debug!("Found non-dollar {c} in URL while expecting arg");
                is_encoded = false;
                out.push('?');
                out.push(c);
            }
            (c, true, _, false) => {
                debug!("Found non-digit {c} in URL while parsing num, adding capture");
                push_capture(&captures, num.parse()?, is_encoded, &mut out)?;
                is_arg = false;
                is_encoded = false;
                num.clear();
                out.push(c);
            }
            _ => {
                debug!("Found non-dollar {c} in URL while not expecting arg");
                out.push(c);
                if escape {
                    debug!("Disabling escape");
                    escape = false;
                }
            }
        }
    }
    if is_arg {
        debug!("Found EOF while parsing arg, adding capture");
        push_capture(&captures, num.parse()?, is_encoded, &mut out)?;
    }
    Ok(out)
}

pub async fn find_redirect_service_by_url<'a>(
    guard: &'a RwLockReadGuard<'a, CrawledData>,
    services: &'a ServicesData,
    regexes: &'a Regexes,
    query: &str,
) -> Result<(&'a CrawledService, &'a Service, String), SearchError> {
    let data = match guard.get_services() {
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
    preferred_instances: &[String],
) -> Option<Vec<&'a CrawledInstance>> {
    let alive_instances = crawled_service.get_alive_instances();
    let instances = alive_instances
        .filter(|i| required_tags.iter().all(|tag| i.tags.contains(tag)))
        .filter(|i| forbidden_tags.iter().all(|tag| !i.tags.contains(tag)))
        .collect::<Vec<_>>();
    if instances.is_empty() {
        return None;
    }
    let instances = if preferred_instances.is_empty() {
        instances
    } else {
        // Filter out instances that are not in the preferred list.
        let filtered_preferred_instances: Vec<_> = instances
            .iter()
            .filter(|i| {
                let host = i.url.host_str().unwrap_or("");
                preferred_instances.iter().any(|p| host == p)
            })
            .cloned()
            .collect();
        // If there are no preferred instances, return all instances.
        if filtered_preferred_instances.is_empty() {
            instances
        } else {
            filtered_preferred_instances
        }
    };
    Some(instances)
}

const MAX_DURATION: Duration = Duration::from_secs(u64::MAX);

pub async fn get_redirect_instance(
    crawled_service: &CrawledService,
    service: &Service,
    user_config: &UserConfig,
    reputation_config: &ReputationConfig,
    state_store: &dyn StateStore,
) -> Result<(CrawledInstance, bool), SearchError> {
    let instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
        &user_config.preferred_instances,
    );
    match &instances {
        None => match &service.fallback {
            Some(fallback) => Ok((
                CrawledInstance {
                    url: fallback.clone(),
                    status: CrawledInstanceStatus::Ok(MAX_DURATION),
                    tags: vec![],
                },
                true,
            )),
            None => Err(SearchError::NoInstancesFound),
        },
        Some(instances) => {
            let selected = match &user_config.select_method {
                SelectMethod::Random => (*instances[fastrand::usize(..instances.len())]).clone(),
                SelectMethod::LowPing => instances
                    .iter()
                    .min_by_key(|i| match i.status {
                        CrawledInstanceStatus::Ok(ping) => ping,
                        _ => MAX_DURATION,
                    })
                    .unwrap()
                    .to_owned()
                    .to_owned(),
                SelectMethod::Weighted if reputation_config.enabled => {
                    let urls = instances
                        .iter()
                        .map(|instance| instance.url.as_str().to_owned())
                        .collect::<Vec<_>>();
                    match state_store.get_reputations(&urls).await {
                        Ok(reputations) => select_weighted_instance(
                            instances,
                            &reputations,
                            reputation_config.minimum_weight,
                            reputation_config.maximum_weight,
                            fastrand::f64(),
                        )
                        .clone(),
                        Err(error) => {
                            warn!("Failed to load instance reputation: {error}");
                            (*instances[fastrand::usize(..instances.len())]).clone()
                        }
                    }
                }
                SelectMethod::Weighted => (*instances[fastrand::usize(..instances.len())]).clone(),
            };
            Ok((selected, false))
        }
    }
}

fn select_weighted_instance<'a>(
    instances: &[&'a CrawledInstance],
    reputations: &std::collections::HashMap<String, InstanceReputation>,
    minimum_weight: f64,
    maximum_weight: f64,
    sample: f64,
) -> &'a CrawledInstance {
    let weights = instances
        .iter()
        .map(|instance| {
            reputations
                .get(instance.url.as_str())
                .copied()
                .unwrap_or_default()
                .weight(minimum_weight, maximum_weight)
        })
        .collect::<Vec<_>>();
    let total = weights.iter().sum::<f64>();
    let mut target = sample.clamp(0.0, 1.0 - f64::EPSILON) * total;
    for (instance, weight) in instances.iter().zip(weights) {
        if target < weight {
            return instance;
        }
        target -= weight;
    }
    instances.last().expect("instances are not empty")
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use super::*;
    use async_trait::async_trait;
    use fastside_shared::{
        config::{ReputationConfig, SelectMethod, UserConfig},
        serde_types::{AllowedHttpCodes, Service},
    };
    use regex::{Captures, Regex};
    use url::Url;

    use crate::storage::{CrawlSnapshot, StorageError, VoteDirection};

    #[derive(Debug)]
    struct FailingStore;

    #[async_trait]
    impl StateStore for FailingStore {
        async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError> {
            Err(StorageError("unavailable".to_owned()))
        }

        async fn save_crawl_snapshot(&self, _snapshot: &CrawlSnapshot) -> Result<(), StorageError> {
            Err(StorageError("unavailable".to_owned()))
        }

        async fn get_reputations(
            &self,
            _instances: &[String],
        ) -> Result<HashMap<String, InstanceReputation>, StorageError> {
            Err(StorageError("unavailable".to_owned()))
        }

        async fn apply_vote(
            &self,
            _instance: &str,
            _direction: VoteDirection,
        ) -> Result<InstanceReputation, StorageError> {
            Err(StorageError("unavailable".to_owned()))
        }
    }

    fn instance(url: &str, milliseconds: u64, tags: &[&str]) -> CrawledInstance {
        CrawledInstance {
            url: Url::parse(url).unwrap(),
            status: CrawledInstanceStatus::Ok(Duration::from_millis(milliseconds)),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        }
    }

    fn service(instances: Vec<CrawledInstance>) -> (CrawledService, Service) {
        (
            CrawledService {
                name: "demo".to_owned(),
                instances,
            },
            Service {
                name: "demo".to_owned(),
                test_url: "/".to_owned(),
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
                instances: Vec::new(),
            },
        )
    }

    fn setup_captures<'t>(text: &'t str, re: &'t str) -> Captures<'t> {
        let regex = Regex::new(re).unwrap();
        regex.captures(text).unwrap()
    }

    #[test]
    fn test_no_placeholders() {
        let url = "http://example.com/path";
        let captures = setup_captures("input text", r"(input) (text)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), "http://example.com/path");
    }

    #[test]
    fn test_simple_replacement() {
        let url = "http://example.com/$1";
        let captures = setup_captures("value", r"(value)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), "http://example.com/value");
    }

    #[test]
    fn test_url_encoding() {
        let url = r"http://example.com/\?param=?$1";
        let captures = setup_captures("value space", r"(value space)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), "http://example.com/?param=value%20space");
    }

    #[test]
    fn test_multiple_replacements() {
        let url = r"http://example.com/$1/page\?$2";
        let captures = setup_captures("value1 value2", r"(value1) (value2)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), "http://example.com/value1/page?value2");
    }

    #[test]
    fn test_invalid_capture_group() {
        let url = "http://example.com/$2";
        let captures = setup_captures("value1", r"(value1)");
        let result = replace_args_in_url(url, captures);
        assert!(result.is_err());
    }

    #[test]
    fn test_escape() {
        let url = r"http://example.com/\$1";
        let captures = setup_captures("value", r"(value)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), "http://example.com/$1");
    }

    #[test]
    fn test_multiple_escape_characters() {
        let url = r"http://example.com/\\\$1";
        let captures = setup_captures("value", r"(value)");
        let result = replace_args_in_url(url, captures);
        assert_eq!(result.unwrap(), r"http://example.com/\$1");
    }

    #[test]
    fn weighted_selection_uses_reputation_and_bounds() {
        let first = instance("https://first.example/", 20, &[]);
        let second = instance("https://second.example/", 10, &[]);
        let instances = vec![&first, &second];
        let reputations = HashMap::from([
            (
                first.url.as_str().to_owned(),
                InstanceReputation {
                    upvotes: 100,
                    downvotes: 0,
                },
            ),
            (
                second.url.as_str().to_owned(),
                InstanceReputation {
                    upvotes: 0,
                    downvotes: 100,
                },
            ),
        ]);

        assert_eq!(
            select_weighted_instance(&instances, &reputations, 0.5, 2.0, 0.79).url,
            first.url
        );
        assert_eq!(
            select_weighted_instance(&instances, &reputations, 0.5, 2.0, 0.81).url,
            second.url
        );
    }

    #[test]
    fn tag_and_preferred_filters_run_before_selection() {
        let first = instance("https://first.example/", 20, &["https", "ipv4"]);
        let second = instance("https://second.example/", 10, &["https", "ipv6"]);
        let crawled = CrawledService {
            name: "demo".to_owned(),
            instances: vec![first, second],
        };
        let filtered = get_redirect_instances(
            &crawled,
            &["https".to_owned()],
            &["ipv6".to_owned()],
            &["first.example".to_owned()],
        )
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].url.as_str(), "https://first.example/");
    }

    #[tokio::test]
    async fn weighted_selection_falls_back_when_storage_fails() {
        let (crawled, service) = service(vec![
            instance("https://first.example/", 20, &[]),
            instance("https://second.example/", 10, &[]),
        ]);
        let config = UserConfig {
            required_tags: Vec::new(),
            select_method: SelectMethod::Weighted,
            ..UserConfig::default()
        };
        let reputation = ReputationConfig {
            enabled: true,
            ..ReputationConfig::default()
        };

        let (selected, fallback) =
            get_redirect_instance(&crawled, &service, &config, &reputation, &FailingStore)
                .await
                .unwrap();
        assert!(!fallback);
        assert!(
            crawled
                .instances
                .iter()
                .any(|item| item.url == selected.url)
        );
    }
}
