use std::collections::HashMap;

use askama::Template;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Response, StatusCode, header::CONTENT_TYPE},
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};

use crate::{
    crawler::CrawledService, errors::RedirectError, filters, search::SearchError, types::AppState,
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
}

async fn index(State(state): State<AppState>) -> Result<impl IntoResponse, RedirectError> {
    let data = state.crawler.read().await;
    let Some(crawled_services) = data.get_services() else {
        return Err(SearchError::CrawlerNotFetchedYet.into());
    };
    let loaded_data_guard = state.loaded_data.read().await;
    let template = IndexTemplate {
        services: &loaded_data_guard.services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
        is_reloading: data.is_reloading(),
        is_initialized_from_defaults: data.is_initialized_from_defaults(),
    };

    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        template.render().expect("failed to render index page"),
    ))
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
