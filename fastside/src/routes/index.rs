use std::collections::HashMap;

use actix_web::{get, web, Responder, Scope};
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
}

#[get("/")]
async fn index(
    req: web::HttpRequest,
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<RwLock<LoadedData>>,
) -> actix_web::Result<impl Responder> {
    let data = crawler.read().await;
    let Some(crawled_services) = data.get_services() else {
        return Err(RedirectError::from(SearchError::CrawlerNotFetchedYet))?;
    };
    let loaded_data_guard = loaded_data.read().await;

    let user_agent = req
        .headers()
        .get("User-Agent")
        .and_then(|ua| ua.to_str().ok())
        .unwrap_or("");

    if user_agent.contains("curl") {
        let mut plain_text_output = String::new();
        for (service_name, service) in &crawled_services.services {
            plain_text_output.push_str(&format!("Service: {}\n", service_name));
            for instance in &service.instances {
                plain_text_output.push_str(&format!(
                    "  Instance: {}\n  Status: {:?}\n  Tags: {:?}\n",
                    instance.url, instance.status, instance.tags
                ));
            }
            plain_text_output.push('\n');
        }
        return Ok(actix_web::HttpResponse::Ok()
            .content_type("text/plain; charset=utf-8")
            .body(plain_text_output));
    }

    let template = IndexTemplate {
        services: &loaded_data_guard.services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
        is_reloading: data.is_reloading(),
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
