use std::{collections::HashMap, sync::Arc};

use axum::Router;
use fastside_core::{
    crawler::Crawler,
    routes::main_router,
    types::{AppState, LoadedData},
};
use fastside_shared::config::{AppConfig, UserConfig};
use tokio::sync::RwLock;
use tower_service::Service;
use worker::*;

fn router() -> Router {
    let config = Arc::new(AppConfig::default());
    let loaded_data = Arc::new(RwLock::new(LoadedData {
        services: HashMap::new(),
        proxies: HashMap::new(),
        default_user_config: UserConfig::default(),
    }));
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        crawler: Arc::new(Crawler::new(
            loaded_data.clone(),
            config.clone().crawler.clone(),
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
    _env: Env,
    _ctx: Context,
) -> Result<axum::http::Response<axum::body::Body>> {
    console_error_panic_hook::set_once();

    Ok(router().call(req).await?)
}
