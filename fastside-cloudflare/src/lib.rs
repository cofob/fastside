use std::{collections::HashMap, sync::Arc};

use axum::Router;
use fastside_core::{
    crawler::{CrawledData, Crawler},
    routes::main_router,
    types::{AppState, LoadedData},
};
use fastside_shared::{
    config::AppConfig,
    serde_types::{ServicesData, StoredData},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_service::Service;
use worker::*;

fn load_config(env: &Env) -> AppConfig {
    let config_str = env
        .var("config")
        .expect("config variable is not set")
        .to_string();
    let config: AppConfig = serde_json::from_str(&config_str).expect("failed to parse config");
    config
}

#[derive(Serialize, Deserialize, Debug)]
struct KvStoredData {
    loaded_data: LoadedData,
    crawled_data: CrawledData,
}

async fn router(env: &Env) -> Router {
    let config = Arc::new(load_config(&env));
    let stored_data: KvStoredData = serde_json::from_str(
        &env.kv("fastside")
            .expect("failed to get kv")
            .get("stored_data")
            .text()
            .await
            .expect("failed to get stored_data from kv")
            .expect("stored_data not found"),
    )
    .expect("failed to parse loaded_data");
    let loaded_data = Arc::new(RwLock::new(stored_data.loaded_data));
    let crawled_data = stored_data.crawled_data;
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        crawler: Arc::new(Crawler::with_data(
            loaded_data.clone(),
            config.clone().crawler.clone(),
            crawled_data,
        )),
        loaded_data: loaded_data.clone(),
        regexes: HashMap::new(),
    });
    Router::new()
        .nest("/", main_router())
        .with_state(shared_state)
}

#[event(fetch)]
async fn fetch(
    req: HttpRequest,
    env: Env,
    _ctx: Context,
) -> Result<axum::http::Response<axum::body::Body>> {
    console_error_panic_hook::set_once();

    Ok(router(&env).await.call(req).await?)
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    console_error_panic_hook::set_once();

    let config = load_config(&env);
    let services_url = env
        .var("services_url")
        .expect("services_url variable is not set")
        .to_string();

    let services_str = reqwest::get(services_url)
        .await
        .expect("request to services failed")
        .text()
        .await
        .expect("failed to get services text");
    let stored_data: StoredData =
        serde_json::from_str(&services_str).expect("failed to parse services");

    let services_data: ServicesData = stored_data
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    let loaded_data = LoadedData {
        services: services_data,
        proxies: config.proxies.clone(),
        default_user_config: config.default_user_config.clone(),
    };
    let loaded_data_clone = loaded_data.clone();
    let loaded_data = Arc::new(RwLock::new(loaded_data));

    let crawler = Crawler::new(loaded_data, config.crawler.clone());
    crawler.crawl(None).await.expect("failed to crawl");

    let stored_data = KvStoredData {
        loaded_data: loaded_data_clone,
        crawled_data: crawler.read().await.clone(),
    };

    let data_str = serde_json::to_string(&stored_data).expect("failed to serialize data");
    env.kv("fastside")
        .expect("failed to get kv")
        .put("stored_data", data_str)
        .expect("failed to put stored_data to kv (builder)")
        .execute()
        .await
        .expect("failed to put stored_data to kv (request)");
}
