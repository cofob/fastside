use std::collections::HashMap;

use askama::Template;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE},
    },
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};

use crate::{
    crawler::CrawledService,
    errors::RedirectError,
    filters,
    reputation::{append_cookie, csrf_token},
    search::SearchError,
    storage::InstanceReputation,
    types::AppState,
};
use fastside_shared::serde_types::ServicesData;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/favicon.ico", get(favicon))
        .route("/robots.txt", get(robots_txt))
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    pub crawled_services: &'a HashMap<String, CrawledService>,
    pub services: &'a ServicesData,
    pub time: &'a DateTime<Utc>,
    pub is_reloading: bool,
    pub is_initialized_from_defaults: bool,
    pub reputations: &'a HashMap<String, InstanceReputation>,
    pub reputation_enabled: bool,
    pub captcha_enabled: bool,
    pub csrf_token: &'a str,
}

async fn index(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response<Body>, RedirectError> {
    let (crawled_services, is_reloading, is_initialized_from_defaults) = {
        let data = state.crawler.read().await;
        let Some(crawled_services) = data.get_services() else {
            return Err(SearchError::CrawlerNotFetchedYet.into());
        };
        (
            crawled_services.clone(),
            data.is_reloading(),
            data.is_initialized_from_defaults(),
        )
    };
    let services = state.loaded_data.read().await.services.clone();
    let reputations = if state.config.reputation.enabled {
        let instances = crawled_services
            .services
            .values()
            .flat_map(|service| service.instances.iter())
            .map(|instance| instance.url.as_str().to_owned())
            .collect::<Vec<_>>();
        state
            .state_store
            .get_reputations(&instances)
            .await
            .unwrap_or_else(|error| {
                warn!("Failed to load reputation for the instance list: {error}");
                HashMap::new()
            })
    } else {
        HashMap::new()
    };
    let (csrf_token, csrf_cookie) =
        if state.config.reputation.enabled && !state.config.reputation.captcha.enabled {
            match csrf_token(&headers) {
                Ok(token) => token,
                Err(_) => {
                    return Ok((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "failed to create CSRF token",
                    )
                        .into_response());
                }
            }
        } else {
            (String::new(), None)
        };
    let template = IndexTemplate {
        services: &services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
        is_reloading,
        is_initialized_from_defaults,
        reputations: &reputations,
        reputation_enabled: state.config.reputation.enabled,
        captcha_enabled: state.config.reputation.captcha.enabled,
        csrf_token: &csrf_token,
    };

    let mut response = (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        template.render().expect("failed to render index page"),
    )
        .into_response();
    if !csrf_token.is_empty() {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    append_cookie(&mut response, csrf_cookie);
    Ok(response)
}

const FAVICON: &[u8] = include_bytes!("../../static/favicon.ico");

async fn favicon() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/x-icon")
        .body(Body::from(FAVICON))
        .expect("static favicon response is valid")
}

const ROBOTS_TXT: &str = "User-agent: *\nDisallow: /\n";

async fn robots_txt() -> impl IntoResponse {
    ([(CONTENT_TYPE, "text/plain")], ROBOTS_TXT)
}
