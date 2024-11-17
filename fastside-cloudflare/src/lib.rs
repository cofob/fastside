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

async fn router(env: &Env) -> Router {
    let config = Arc::new(load_config(&env));
    let loaded_data: Arc<RwLock<LoadedData>> = Arc::new(RwLock::new(
        serde_json::from_str(
            &env.kv("fastside")
                .expect("failed to get kv")
                .get("loaded_data")
                .text()
                .await
                .expect("failed to get loaded_data from kv")
                .expect("loaded_data not found"),
        )
        .expect("failed to parse loaded_data"),
    ));
    let crawled_data: CrawledData = serde_json::from_str(
        &env.kv("fastside")
            .expect("failed to get kv")
            .get("crawled_data")
            .text()
            .await
            .expect("failed to get crawled_data from kv")
            .expect("crawled_data not found"),
    )
    .expect("failed to parse data");
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

    let loaded_data = {
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
        env.kv("fastside")
            .expect("failed to get kv")
            .put(
                "loaded_data",
                serde_json::to_string(&loaded_data).expect("failed to serialize loaded_data"),
            )
            .expect("failed to put loaded_data to kv (builder)")
            .execute()
            .await
            .expect("failed to put loaded_data to kv (request)");
        Arc::new(RwLock::new(loaded_data))
    };

    let crawler = Crawler::new(loaded_data, config.crawler.clone());
    crawler.crawl(None).await.expect("failed to crawl");

    let data_str = serde_json::to_string(&*crawler.read().await).expect("failed to serialize data");
    env.kv("fastside")
        .expect("failed to get kv")
        .put("crawled_data", data_str)
        .expect("failed to put crawled_data to kv (builder)")
        .execute()
        .await
        .expect("failed to put crawled_data to kv (request)");
}
