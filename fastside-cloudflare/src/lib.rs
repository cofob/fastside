#![cfg(target_arch = "wasm32")]

mod proxy;

use std::{
    collections::HashMap,
    sync::{Arc, Once},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
};
use chrono::Utc;
use fastside::{
    app,
    crawler::{
        CrawledData, CrawledInstanceStatus, CrawledService, CrawledServices, Crawler, CrawlerError,
        InstanceClient, InstanceRequest, InstanceResponse, select_instance_batch,
        should_read_response_body,
    },
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{AppConfig, CrawlerConfig, ProxyData, select_proxy},
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service as FastsideService, ServicesData, StoredData},
};
use futures::{future::Either, pin_mut};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_service::Service as _;
use url::Url;
use worker::{
    AbortController, Context, Date, Delay, DurableObject, Env, Fetch, Headers, HttpRequest,
    Request as WorkerRequest, RequestInit, RequestRedirect, Response as WorkerResponse, Result,
    ScheduleContext, ScheduledEvent, State as DurableObjectState, console_error, event,
};

const CONFIG_VARIABLE: &str = "FASTSIDE_CONFIG";
const CRAWLER_BINDING: &str = "CRAWLER";
const CRAWLER_NAME: &str = "global";
const KV_BINDING: &str = "FASTSIDE";
const SERVICES_URL_VARIABLE: &str = "FASTSIDE_SERVICES_URL";
const BATCH_SIZE_VARIABLE: &str = "FASTSIDE_CRAWL_BATCH_SIZE";
const SNAPSHOT_KEY: &str = "snapshot";
const CRAWL_STATE_KEY: &str = "crawl-state-v1";
const CRAWL_INTERVAL: Duration = Duration::from_secs(120);
const DEFAULT_BATCH_SIZE: usize = 20;
const MAX_BATCH_SIZE: usize = 40;

#[derive(Debug, Deserialize, Serialize)]
struct Snapshot {
    loaded_data: LoadedData,
    crawled_data: CrawledData,
}

#[derive(Debug, Deserialize, Serialize)]
struct CrawlState {
    loaded_data: LoadedData,
    crawled_services: HashMap<String, CrawledService>,
    next_instance: usize,
}

impl CrawlState {
    fn new(loaded_data: LoadedData) -> Self {
        let crawled_services = loaded_data
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
        Self {
            loaded_data,
            crawled_services,
            next_instance: 0,
        }
    }

    fn total_instances(&self) -> usize {
        self.loaded_data
            .services
            .values()
            .map(|service| service.instances.len())
            .sum()
    }

    fn is_complete(&self) -> bool {
        self.next_instance >= self.total_instances()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            loaded_data: self.loaded_data.clone(),
            crawled_data: CrawledData::CrawledServices(CrawledServices {
                services: self.crawled_services.clone(),
                time: Utc::now(),
            }),
        }
    }
}

#[derive(Debug, Default)]
struct CloudflareInstanceClient;

#[async_trait(?Send)]
impl InstanceClient for CloudflareInstanceClient {
    async fn request(
        &self,
        config: &CrawlerConfig,
        proxies: &ProxyData,
        service: &FastsideService,
        instance: &Instance,
        test_url: Url,
    ) -> std::result::Result<InstanceRequest, CrawlerError> {
        if let Some(proxy) = select_proxy(proxies, &instance.tags) {
            return proxy::request(proxy, config, service, instance, test_url).await;
        }

        let headers = Headers::new();
        for (name, value) in REQUEST_HEADERS {
            headers
                .set(name, value)
                .map_err(|error| CrawlerError::Request(error.to_string()))?;
        }

        let redirect = if service.follow_redirects {
            RequestRedirect::Follow
        } else {
            RequestRedirect::Manual
        };
        let mut init = RequestInit::new();
        init.with_headers(headers).with_redirect(redirect);
        let request = WorkerRequest::new_with_init(test_url.as_str(), &init)
            .map_err(|error| CrawlerError::Request(error.to_string()))?;
        let timeout =
            config.get_domain_timeout(instance.url.host_str().expect("instance URL has a host"));
        let controller = AbortController::default();
        let signal = controller.signal();
        let fetch = Fetch::Request(request);
        let response = fetch.send_with_signal(&signal);
        let delay = Delay::from(timeout);
        pin_mut!(response, delay);

        let start = Date::now().as_millis();
        let mut response = match futures::future::select(response, delay).await {
            Either::Left((Ok(response), _)) => response,
            Either::Left((Err(_), _)) => {
                return Ok(InstanceRequest::Failed(CrawledInstanceStatus::RequestError));
            }
            Either::Right(((), _)) => {
                controller.abort();
                return Ok(InstanceRequest::Failed(CrawledInstanceStatus::TimedOut));
            }
        };
        let duration = Duration::from_millis(Date::now().as_millis().saturating_sub(start));
        let status_code = response.status_code();
        let body = if should_read_response_body(service, instance, status_code) {
            let body = response.text();
            let delay = Delay::from(timeout);
            pin_mut!(body, delay);
            match futures::future::select(body, delay).await {
                Either::Left((Ok(body), _)) => Some(body),
                Either::Left((Err(error), _)) => {
                    controller.abort();
                    return Err(CrawlerError::Request(error.to_string()));
                }
                Either::Right(((), _)) => {
                    controller.abort();
                    return Err(CrawlerError::Request("response body timed out".to_owned()));
                }
            }
        } else {
            controller.abort();
            None
        };
        Ok(InstanceRequest::Response(InstanceResponse {
            status_code,
            duration,
            body,
        }))
    }
}

fn load_config(env: &Env) -> Result<AppConfig> {
    Ok(serde_json::from_str(
        &env.var(CONFIG_VARIABLE)?.to_string(),
    )?)
}

fn services_url(env: &Env) -> Result<Url> {
    let value = env
        .var(SERVICES_URL_VARIABLE)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| {
            "https://raw.githubusercontent.com/cofob/fastside/master/services.json".to_owned()
        });
    Url::parse(&value).map_err(|error| worker::Error::RustError(error.to_string()))
}

async fn load_state(env: &Env) -> Result<AppState> {
    let config = Arc::new(load_config(env)?);
    let snapshot = env
        .kv(KV_BINDING)?
        .get(SNAPSHOT_KEY)
        .json::<Snapshot>()
        .await?
        .ok_or_else(|| {
            worker::Error::RustError("crawler snapshot is not initialized".to_owned())
        })?;
    let regexes = Arc::new(compile_regexes(&snapshot.loaded_data.services));
    let loaded_data = Arc::new(RwLock::new(snapshot.loaded_data));
    let crawler = Arc::new(Crawler::with_data(
        loaded_data.clone(),
        config.crawler.clone(),
        Arc::new(CloudflareInstanceClient),
        snapshot.crawled_data,
    ));
    Ok(AppState {
        config,
        crawler,
        loaded_data,
        regexes,
    })
}

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> Result<AxumResponse> {
    console_error_panic_hook::set_once();
    static SEED_RNG: Once = Once::new();
    SEED_RNG.call_once(|| {
        let mut seed = [0; 8];
        if getrandom::getrandom(&mut seed).is_ok() {
            fastrand::seed(u64::from_ne_bytes(seed));
        }
    });
    let state = match load_state(&env).await {
        Ok(state) => state,
        Err(error) => {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response());
        }
    };
    Ok(app(state).call(req).await?)
}

fn batch_size(env: &Env) -> Result<usize> {
    let value = match env.var(BATCH_SIZE_VARIABLE) {
        Ok(value) => value
            .to_string()
            .parse()
            .map_err(|error| worker::Error::RustError(format!("invalid batch size: {error}")))?,
        Err(_) => DEFAULT_BATCH_SIZE,
    };
    if !(1..=MAX_BATCH_SIZE).contains(&value) {
        return Err(worker::Error::RustError(format!(
            "{BATCH_SIZE_VARIABLE} must be between 1 and {MAX_BATCH_SIZE}"
        )));
    }
    Ok(value)
}

async fn load_services(env: &Env, config: &AppConfig) -> Result<LoadedData> {
    let mut response = Fetch::Url(services_url(env)?).send().await?;
    let stored_data: StoredData = serde_json::from_str(&response.text().await?)?;
    let services: ServicesData = stored_data
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    Ok(LoadedData {
        services,
        proxies: config.proxies.clone(),
        default_user_config: config.default_user_config.clone(),
    })
}

async fn default_snapshot(loaded_data: LoadedData, config: &AppConfig) -> Snapshot {
    let shared_data = Arc::new(RwLock::new(loaded_data.clone()));
    let crawler = Crawler::new(
        shared_data,
        config.crawler.clone(),
        Arc::new(CloudflareInstanceClient),
    );
    crawler.initialize_with_defaults().await;
    let crawled_data = crawler.read().await.clone();
    Snapshot {
        loaded_data,
        crawled_data,
    }
}

async fn crawl_batch(state: &mut CrawlState, config: &AppConfig, limit: usize) -> Result<()> {
    let (services, count) =
        select_instance_batch(&state.loaded_data.services, state.next_instance, limit);
    if count == 0 {
        return Ok(());
    }

    let loaded_data = LoadedData {
        services,
        proxies: state.loaded_data.proxies.clone(),
        default_user_config: state.loaded_data.default_user_config.clone(),
    };
    let crawler = Crawler::new(
        Arc::new(RwLock::new(loaded_data)),
        config.crawler.clone(),
        Arc::new(CloudflareInstanceClient),
    );
    crawler
        .crawl_once()
        .await
        .map_err(|error| worker::Error::RustError(error.to_string()))?;

    let batch = crawler.read().await.clone();
    let CrawledData::CrawledServices(batch) = batch else {
        return Err(worker::Error::RustError(
            "crawler did not return a completed batch".to_owned(),
        ));
    };
    for (name, mut service) in batch.services {
        state
            .crawled_services
            .get_mut(&name)
            .ok_or_else(|| {
                worker::Error::RustError(format!("crawler returned unknown service: {name}"))
            })?
            .instances
            .append(&mut service.instances);
    }
    state.next_instance += count;
    Ok(())
}

async fn update_snapshot(object_state: &DurableObjectState, env: &Env) -> Result<()> {
    let config = load_config(env)?;
    let kv = env.kv(KV_BINDING)?;
    let storage = object_state.storage();
    let stored_state: Option<CrawlState> = storage
        .get::<String>(CRAWL_STATE_KEY)
        .await?
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    let mut state = match stored_state {
        Some(state) if !state.is_complete() => state,
        _ => CrawlState::new(load_services(env, &config).await?),
    };

    if kv.get(SNAPSHOT_KEY).bytes().await?.is_none() {
        let snapshot = default_snapshot(state.loaded_data.clone(), &config).await;
        kv.put(SNAPSHOT_KEY, serde_json::to_string(&snapshot)?)?
            .execute()
            .await?;
        storage
            .put(CRAWL_STATE_KEY, serde_json::to_string(&state)?)
            .await?;
        return Ok(());
    }

    crawl_batch(&mut state, &config, batch_size(env)?).await?;
    if state.is_complete() {
        kv.put(SNAPSHOT_KEY, serde_json::to_string(&state.snapshot())?)?
            .execute()
            .await?;
    }
    storage
        .put(CRAWL_STATE_KEY, serde_json::to_string(&state)?)
        .await?;
    worker::console_log!(
        "Crawled {} of {} instances",
        state.next_instance,
        state.total_instances()
    );
    Ok(())
}

#[worker::durable_object(alarm)]
pub struct CrawlerCoordinator {
    state: DurableObjectState,
    env: Env,
}

impl DurableObject for CrawlerCoordinator {
    fn new(state: DurableObjectState, env: Env) -> Self {
        console_error_panic_hook::set_once();
        Self { state, env }
    }

    async fn fetch(&self, _request: WorkerRequest) -> Result<WorkerResponse> {
        let storage = self.state.storage();
        if storage.get_alarm().await?.is_none() {
            storage.set_alarm(Duration::ZERO).await?;
        }
        WorkerResponse::empty()
    }

    async fn alarm(&self) -> Result<WorkerResponse> {
        self.state.storage().set_alarm(CRAWL_INTERVAL).await?;
        if let Err(error) = update_snapshot(&self.state, &self.env).await {
            console_error!("Failed to update crawler snapshot: {error}");
        }
        WorkerResponse::empty()
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    console_error_panic_hook::set_once();
    let result: Result<()> = async {
        env.durable_object(CRAWLER_BINDING)?
            .get_by_name(CRAWLER_NAME)?
            .fetch_with_str("https://crawler.fastside/")
            .await?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        console_error!("Failed to schedule crawler: {error}");
    }
}
