#![cfg(target_arch = "wasm32")]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use fastside::{
    app,
    crawler::{
        CrawledData, CrawledInstanceStatus, Crawler, CrawlerError, InstanceClient, InstanceRequest,
        InstanceResponse, should_read_response_body,
    },
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{AppConfig, CrawlerConfig, ProxyData},
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service as FastsideService, ServicesData, StoredData},
};
use futures::{future::Either, pin_mut};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_service::Service as _;
use url::Url;
use worker::{
    AbortController, Context, Date, Delay, Env, Fetch, Headers, HttpRequest, Request, RequestInit,
    RequestRedirect, Result, ScheduleContext, ScheduledEvent, console_error, event,
};

const CONFIG_VARIABLE: &str = "FASTSIDE_CONFIG";
const KV_BINDING: &str = "FASTSIDE";
const SERVICES_URL_VARIABLE: &str = "FASTSIDE_SERVICES_URL";
const SNAPSHOT_KEY: &str = "snapshot";

#[derive(Debug, Deserialize, Serialize)]
struct Snapshot {
    loaded_data: LoadedData,
    crawled_data: CrawledData,
}

#[derive(Debug, Default)]
struct CloudflareInstanceClient;

#[async_trait(?Send)]
impl InstanceClient for CloudflareInstanceClient {
    async fn request(
        &self,
        config: &CrawlerConfig,
        _proxies: &ProxyData,
        service: &FastsideService,
        instance: &Instance,
        test_url: Url,
    ) -> std::result::Result<InstanceRequest, CrawlerError> {
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
        let request = Request::new_with_init(test_url.as_str(), &init)
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
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let state = match load_state(&env).await {
        Ok(state) => state,
        Err(error) => {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response());
        }
    };
    Ok(app(state).call(req).await?)
}

async fn update_snapshot(env: &Env) -> Result<()> {
    let config = load_config(env)?;
    let mut response = Fetch::Url(services_url(env)?).send().await?;
    let stored_data: StoredData = serde_json::from_str(&response.text().await?)?;
    let services: ServicesData = stored_data
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    let loaded_data = LoadedData {
        services,
        proxies: config.proxies.clone(),
        default_user_config: config.default_user_config.clone(),
    };
    let shared_data = Arc::new(RwLock::new(loaded_data.clone()));
    let crawler = Crawler::new(
        shared_data,
        config.crawler,
        Arc::new(CloudflareInstanceClient),
    );
    crawler
        .crawl_once()
        .await
        .map_err(|error| worker::Error::RustError(error.to_string()))?;
    let snapshot = Snapshot {
        loaded_data,
        crawled_data: crawler.read().await.clone(),
    };
    env.kv(KV_BINDING)?
        .put(SNAPSHOT_KEY, serde_json::to_string(&snapshot)?)?
        .execute()
        .await?;
    Ok(())
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    console_error_panic_hook::set_once();
    if let Err(error) = update_snapshot(&env).await {
        console_error!("Failed to update crawler snapshot: {error}");
    }
}
