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
    captcha::{CaptchaError, CaptchaVerifier, verification_fields, verification_succeeded},
    crawler::{
        CrawledData, CrawledInstanceStatus, CrawledService, CrawledServices, Crawler, CrawlerError,
        InstanceClient, InstanceRequest, InstanceResponse, select_instance_batch,
        should_read_response_body,
    },
    reputation::{VoteProtector, validate_config as validate_reputation_config},
    storage::{CrawlSnapshot, InstanceReputation, StateStore, StorageError, VoteDirection},
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{
        AppConfig, CaptchaConfig, CaptchaEncoding, CrawlerConfig, ProxyData, StorageBackend,
        select_proxy,
    },
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service as FastsideService, ServicesData, StoredData},
};
use futures::{future::Either, pin_mut};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_service::Service as _;
use url::Url;
use wasm_bindgen::JsValue;
use worker::{
    AbortController, Context, Date, Delay, DurableObject, Env, Fetch, Headers, HttpRequest,
    Method as WorkerMethod, ObjectNamespace, Request as WorkerRequest, RequestInit,
    RequestRedirect, Response as WorkerResponse, Result, ScheduleContext, ScheduledEvent,
    State as DurableObjectState, console_error, event,
};

const CONFIG_VARIABLE: &str = "FASTSIDE_CONFIG";
const CAPTCHA_SECRET_VARIABLE: &str = "FASTSIDE_CAPTCHA_SECRET";
const CRAWLER_BINDING: &str = "CRAWLER";
const CRAWLER_NAME: &str = "global";
const KV_BINDING: &str = "FASTSIDE";
const SERVICES_URL_VARIABLE: &str = "FASTSIDE_SERVICES_URL";
const BATCH_SIZE_VARIABLE: &str = "FASTSIDE_CRAWL_BATCH_SIZE";
const SNAPSHOT_KEY: &str = "snapshot";
const CRAWL_STATE_KEY: &str = "crawl-state-v1";
const REPUTATION_STATE_KEY: &str = "reputation-state-v1";
const REPUTATION_SNAPSHOT_KEY: &str = "reputation-snapshot-v1";
const REPUTATION_PUBLISH_DUE_KEY: &str = "reputation-publish-due-v1";
const NEXT_CRAWL_AT_KEY: &str = "next-crawl-at-v1";
const CRAWL_INTERVAL: Duration = Duration::from_secs(120);
const DEFAULT_BATCH_SIZE: usize = 20;
const MAX_BATCH_SIZE: usize = 40;

#[derive(Debug, Default, Deserialize, Serialize)]
struct ReputationSnapshot {
    reputations: HashMap<String, InstanceReputation>,
}

#[derive(Debug, Deserialize, Serialize)]
struct VoteCommand {
    instance: String,
    direction: VoteDirection,
}

#[derive(Clone)]
struct CloudflareStateStore {
    kv: worker::kv::KvStore,
    coordinator: ObjectNamespace,
}

#[derive(Debug, Default)]
struct CloudflareCaptchaVerifier;

async fn verify_captcha(
    config: &CaptchaConfig,
    token: &str,
) -> std::result::Result<bool, CaptchaError> {
    let verify_url = config
        .verify_url
        .as_deref()
        .ok_or_else(|| CaptchaError("verify_url is not configured".to_owned()))?;
    let fields = verification_fields(config, token);
    let headers = Headers::new();
    for (name, value) in &config.headers {
        headers
            .set(name, value)
            .map_err(|error| CaptchaError(error.to_string()))?;
    }
    let body = match config.encoding {
        CaptchaEncoding::Json => {
            headers
                .set("content-type", "application/json")
                .map_err(|error| CaptchaError(error.to_string()))?;
            serde_json::to_string(&fields).map_err(|error| CaptchaError(error.to_string()))?
        }
        CaptchaEncoding::Form => {
            headers
                .set("content-type", "application/x-www-form-urlencoded")
                .map_err(|error| CaptchaError(error.to_string()))?;
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(&fields)
                .finish()
        }
    };
    let mut init = RequestInit::new();
    init.with_method(WorkerMethod::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));
    let request = WorkerRequest::new_with_init(verify_url, &init)
        .map_err(|error| CaptchaError(error.to_string()))?;
    let controller = AbortController::default();
    let signal = controller.signal();
    let verification = async {
        let mut response = Fetch::Request(request)
            .send_with_signal(&signal)
            .await
            .map_err(|error| CaptchaError(error.to_string()))?;
        if !(200..300).contains(&response.status_code()) {
            return Ok(false);
        }
        let body = response
            .text()
            .await
            .map_err(|error| CaptchaError(error.to_string()))?;
        verification_succeeded(config, &body)
    };
    let delay = Delay::from(config.timeout);
    pin_mut!(verification, delay);
    match futures::future::select(verification, delay).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => {
            controller.abort();
            Err(CaptchaError("verification request timed out".to_owned()))
        }
    }
}

#[async_trait]
impl CaptchaVerifier for CloudflareCaptchaVerifier {
    async fn verify(
        &self,
        config: &CaptchaConfig,
        token: &str,
        _remote_ip: Option<std::net::IpAddr>,
    ) -> std::result::Result<bool, CaptchaError> {
        worker::send::SendFuture::new(verify_captcha(config, token)).await
    }
}

impl std::fmt::Debug for CloudflareStateStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CloudflareStateStore").finish()
    }
}

impl CloudflareStateStore {
    async fn load_crawl_snapshot_inner(
        &self,
    ) -> std::result::Result<Option<CrawlSnapshot>, StorageError> {
        let snapshot = self
            .kv
            .get(SNAPSHOT_KEY)
            .json::<Snapshot>()
            .await
            .map_err(|error| StorageError(error.to_string()))?;
        Ok(snapshot.map(|snapshot| CrawlSnapshot {
            crawled_services: snapshot
                .crawled_data
                .get_services()
                .cloned()
                .expect("stored Cloudflare snapshot is initialized"),
        }))
    }

    async fn get_reputations_inner(
        &self,
        instances: &[String],
    ) -> std::result::Result<HashMap<String, InstanceReputation>, StorageError> {
        let snapshot = self
            .kv
            .get(REPUTATION_SNAPSHOT_KEY)
            .json::<ReputationSnapshot>()
            .await
            .map_err(|error| StorageError(error.to_string()))?
            .unwrap_or_default();
        Ok(instances
            .iter()
            .filter_map(|instance| {
                snapshot
                    .reputations
                    .get(instance)
                    .copied()
                    .map(|reputation| (instance.clone(), reputation))
            })
            .collect())
    }

    async fn apply_vote_inner(
        &self,
        instance: &str,
        direction: VoteDirection,
    ) -> std::result::Result<InstanceReputation, StorageError> {
        let body = serde_json::to_string(&VoteCommand {
            instance: instance.to_owned(),
            direction,
        })
        .map_err(|error| StorageError(error.to_string()))?;
        let mut init = RequestInit::new();
        init.with_method(WorkerMethod::Post)
            .with_body(Some(JsValue::from_str(&body)));
        let request = WorkerRequest::new_with_init(
            "https://crawler.fastside/internal/reputation/vote",
            &init,
        )
        .map_err(|error| StorageError(error.to_string()))?;
        let mut response = self
            .coordinator
            .get_by_name(CRAWLER_NAME)
            .map_err(|error| StorageError(error.to_string()))?
            .fetch_with_request(request)
            .await
            .map_err(|error| StorageError(error.to_string()))?;
        if response.status_code() != 200 {
            return Err(StorageError(format!(
                "coordinator returned status {}",
                response.status_code()
            )));
        }
        response
            .json()
            .await
            .map_err(|error| StorageError(error.to_string()))
    }
}

#[async_trait]
impl StateStore for CloudflareStateStore {
    async fn load_crawl_snapshot(
        &self,
    ) -> std::result::Result<Option<CrawlSnapshot>, StorageError> {
        worker::send::SendFuture::new(self.load_crawl_snapshot_inner()).await
    }

    async fn save_crawl_snapshot(
        &self,
        _snapshot: &CrawlSnapshot,
    ) -> std::result::Result<(), StorageError> {
        Err(StorageError(
            "Cloudflare crawler snapshots are written by the coordinator".to_owned(),
        ))
    }

    async fn get_reputations(
        &self,
        instances: &[String],
    ) -> std::result::Result<HashMap<String, InstanceReputation>, StorageError> {
        worker::send::SendFuture::new(self.get_reputations_inner(instances)).await
    }

    async fn apply_vote(
        &self,
        instance: &str,
        direction: VoteDirection,
    ) -> std::result::Result<InstanceReputation, StorageError> {
        worker::send::SendFuture::new(self.apply_vote_inner(instance, direction)).await
    }
}

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
    let mut config: AppConfig = serde_json::from_str(&env.var(CONFIG_VARIABLE)?.to_string())?;
    if let Ok(secret) = env.secret(CAPTCHA_SECRET_VARIABLE) {
        config.reputation.captcha.secret = Some(secret.to_string());
    }
    if !matches!(
        config.storage.backend,
        StorageBackend::Auto | StorageBackend::Cloudflare
    ) {
        return Err(worker::Error::RustError(
            "Cloudflare Workers support only Auto or Cloudflare storage".to_owned(),
        ));
    }
    if config.reputation.ip_protection.rate_limit.enabled
        || config
            .reputation
            .ip_protection
            .one_vote_per_instance
            .enabled
    {
        return Err(worker::Error::RustError(
            "IP vote controls are not supported on Cloudflare Workers".to_owned(),
        ));
    }
    validate_reputation_config(&config).map_err(worker::Error::RustError)?;
    Ok(config)
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
    let state_store: Arc<dyn StateStore> = Arc::new(CloudflareStateStore {
        kv: env.kv(KV_BINDING)?,
        coordinator: env.durable_object(CRAWLER_BINDING)?,
    });
    Ok(AppState {
        config,
        crawler,
        loaded_data,
        regexes,
        state_store,
        captcha_verifier: Arc::new(CloudflareCaptchaVerifier),
        vote_protector: Arc::new(VoteProtector::default()),
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

async fn stored_timestamp(storage: &worker::Storage, key: &str) -> Result<Option<u64>> {
    storage
        .get::<String>(key)
        .await?
        .map(|value| {
            value
                .parse()
                .map_err(|error| worker::Error::RustError(format!("invalid {key}: {error}")))
        })
        .transpose()
}

async fn set_stored_timestamp(storage: &worker::Storage, key: &str, value: u64) -> Result<()> {
    storage.put(key, value.to_string()).await
}

async fn schedule_next_coordinator_alarm(storage: &worker::Storage) -> Result<()> {
    let now = Date::now().as_millis();
    let next_crawl = stored_timestamp(storage, NEXT_CRAWL_AT_KEY).await?;
    let reputation_publish = stored_timestamp(storage, REPUTATION_PUBLISH_DUE_KEY).await?;
    let Some(next) = [next_crawl, reputation_publish].into_iter().flatten().min() else {
        return Ok(());
    };
    storage
        .set_alarm(Duration::from_millis(next.saturating_sub(now)))
        .await
}

async fn load_durable_reputations(
    storage: &worker::Storage,
) -> Result<HashMap<String, InstanceReputation>> {
    storage
        .get::<String>(REPUTATION_STATE_KEY)
        .await?
        .map(|value| serde_json::from_str(&value).map_err(worker::Error::from))
        .transpose()
        .map(Option::unwrap_or_default)
}

async fn publish_reputations(storage: &worker::Storage, env: &Env) -> Result<()> {
    let reputations = load_durable_reputations(storage).await?;
    env.kv(KV_BINDING)?
        .put(
            REPUTATION_SNAPSHOT_KEY,
            serde_json::to_string(&ReputationSnapshot { reputations })?,
        )?
        .execute()
        .await
        .map_err(worker::Error::from)
}

async fn apply_durable_vote(
    storage: &worker::Storage,
    env: &Env,
    command: VoteCommand,
) -> Result<InstanceReputation> {
    let mut reputations = load_durable_reputations(storage).await?;
    let reputation = reputations.entry(command.instance).or_default();
    match command.direction {
        VoteDirection::Up => reputation.upvotes = reputation.upvotes.saturating_add(1),
        VoteDirection::Down => {
            reputation.downvotes = reputation.downvotes.saturating_add(1);
        }
    }
    let updated = *reputation;
    storage
        .put(REPUTATION_STATE_KEY, serde_json::to_string(&reputations)?)
        .await?;
    let publish_interval = load_config(env)?
        .storage
        .cloudflare
        .reputation_publish_interval
        .max(Duration::from_secs(5));
    let due = Date::now()
        .as_millis()
        .saturating_add(publish_interval.as_millis() as u64);
    if stored_timestamp(storage, REPUTATION_PUBLISH_DUE_KEY)
        .await?
        .is_none()
    {
        set_stored_timestamp(storage, REPUTATION_PUBLISH_DUE_KEY, due).await?;
    }
    schedule_next_coordinator_alarm(storage).await?;
    Ok(updated)
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

    async fn fetch(&self, mut request: WorkerRequest) -> Result<WorkerResponse> {
        let storage = self.state.storage();
        if request.path() == "/internal/reputation/vote" {
            let command = request.json::<VoteCommand>().await?;
            let reputation = apply_durable_vote(&storage, &self.env, command).await?;
            return WorkerResponse::from_json(&reputation);
        }
        set_stored_timestamp(&storage, NEXT_CRAWL_AT_KEY, Date::now().as_millis()).await?;
        schedule_next_coordinator_alarm(&storage).await?;
        WorkerResponse::empty()
    }

    async fn alarm(&self) -> Result<WorkerResponse> {
        let storage = self.state.storage();
        let now = Date::now().as_millis();
        if stored_timestamp(&storage, REPUTATION_PUBLISH_DUE_KEY)
            .await?
            .is_some_and(|due| due <= now)
        {
            // Clear the dirty marker before the external KV write. A vote that
            // arrives during that write then schedules another publication.
            storage.delete(REPUTATION_PUBLISH_DUE_KEY).await?;
            if let Err(error) = publish_reputations(&storage, &self.env).await {
                console_error!("Failed to publish reputation snapshot: {error}");
                set_stored_timestamp(
                    &storage,
                    REPUTATION_PUBLISH_DUE_KEY,
                    now.saturating_add(Duration::from_secs(5).as_millis() as u64),
                )
                .await?;
            }
        }
        if stored_timestamp(&storage, NEXT_CRAWL_AT_KEY)
            .await?
            .is_some_and(|due| due <= now)
        {
            if let Err(error) = update_snapshot(&self.state, &self.env).await {
                console_error!("Failed to update crawler snapshot: {error}");
            }
            set_stored_timestamp(
                &storage,
                NEXT_CRAWL_AT_KEY,
                now.saturating_add(CRAWL_INTERVAL.as_millis() as u64),
            )
            .await?;
        }
        schedule_next_coordinator_alarm(&storage).await?;
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
