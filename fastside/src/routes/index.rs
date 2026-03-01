use std::collections::HashMap;

use actix_web::{Responder, Scope, get, web};
use askama::Template;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::{
    config::AppConfig,
    crawler::{CrawledService, Crawler},
    errors::RedirectError,
    filters,
    search::SearchError,
    types::LoadedData,
};
use fastside_shared::serde_types::ServicesData;

use super::{api, config, redirect};

pub fn scope(app_config: &AppConfig) -> Scope {
    web::scope("")
        .service(index)
        .service(favicon)
        .service(robots_txt)
        .service(config::scope(app_config))
        .service(api::scope(app_config))
        .service(redirect::scope(app_config))
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

#[get("/")]
async fn index(
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<RwLock<LoadedData>>,
) -> actix_web::Result<impl Responder> {
    let data = crawler.read().await;
    let Some(crawled_services) = data.get_services() else {
        return Err(RedirectError::from(SearchError::CrawlerNotFetchedYet))?;
    };
    let loaded_data_guard = loaded_data.read().await;
    let template = IndexTemplate {
        services: &loaded_data_guard.services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
        is_reloading: data.is_reloading(),
        is_initialized_from_defaults: data.is_initialized_from_defaults(),
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

const FAVICON: &[u8] = include_bytes!("../../static/favicon.ico");

#[get("/favicon.ico")]
async fn favicon() -> impl Responder {
    actix_web::HttpResponse::Ok()
        .content_type("image/x-icon")
        .body(FAVICON)
}

const ROBOTS_TXT: &str = "User-agent: *\nDisallow: /\n";

#[get("/robots.txt")]
async fn robots_txt() -> impl Responder {
    actix_web::HttpResponse::Ok()
        .content_type("text/plain")
        .body(ROBOTS_TXT)
}
