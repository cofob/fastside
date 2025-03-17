use askama::Template;
use axum::{
    body::Bytes,
    extract::State,
    http::{header::CONTENT_TYPE, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc};

use crate::{crawler::CrawledService, errors::RedirectError, search::SearchError};
use crate::{filters, types::AppState};
use fastside_shared::serde_types::ServicesData;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index))
        .route("/favicon.ico", get(favicon))
        .route("/robots.txt", get(robots_txt))
}

/// The `IndexTemplate` structure renders the index page using the Askama template engine.
#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    pub crawled_services: &'a HashMap<String, CrawledService>,
    pub services: &'a ServicesData,
    pub time: &'a DateTime<Utc>,
    pub is_reloading: bool,
}

/// The `index` handler function renders the main page.
pub async fn index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let data = state.crawler.read().await;
    let Some(crawled_services) = data.get_services() else {
        return RedirectError::from(SearchError::CrawlerNotFetchedYet).into_response();
    };

    // Acquire a read lock on the `LoadedData`.
    let loaded_data_guard = state.loaded_data.read().await;

    // Render the template with the required data.
    let template = IndexTemplate {
        services: &loaded_data_guard.services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
        is_reloading: data.is_reloading(),
    };

    match template.render() {
        Ok(rendered) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "text/html; charset=utf-8")],
            Html(rendered),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Favicon as a static byte slice
const FAVICON: &[u8] = include_bytes!("../../static/favicon.ico");

// Handler for /favicon.ico
async fn favicon() -> impl IntoResponse {
    (
        [
            ("Content-Type", "image/x-icon"),
            ("Cache-Control", "public, max-age=3600"),
        ],
        Bytes::from_static(FAVICON),
    )
}

// Robots.txt content as a static string
const ROBOTS_TXT: &str = "User-agent: *\nDisallow: /\n";

// Handler for /robots.txt
async fn robots_txt() -> impl IntoResponse {
    (
        [
            ("Content-Type", "text/plain"),
            ("Cache-Control", "public, max-age=3600"),
        ],
        ROBOTS_TXT,
    )
}
