use std::sync::Arc;

use actix_web::{
    get,
    http::StatusCode,
    web::{self, Redirect},
    HttpRequest, Responder, Scope,
};
use askama::Template;
use chrono::{DateTime, Utc};
use thiserror::Error;
use url::Url;

use crate::{
    config::AppConfig,
    crawler::{CrawledService, Crawler, CrawlerError},
    errors::impl_api_error,
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("")
        .service(index)
        .service(history_redirect)
        .service(cached_redirect)
        .service(base_redirect)
}

#[derive(Error, Debug)]
pub enum RedirectError {
    #[error("crawler error: `{0}`")]
    CrawlerError(#[from] CrawlerError),
}

impl_api_error!(RedirectError,
    status => {
        RedirectError::CrawlerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    },
    data => {
        _ => None,
    }
);

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    pub services: &'a Vec<CrawledService>,
    pub time: &'a DateTime<Utc>,
}

mod filters {
    use crate::crawler::CrawledInstance;

    pub fn sort_list(l: &[CrawledInstance]) -> ::askama::Result<Vec<CrawledInstance>> {
        let mut new = l.to_owned();
        new.sort_by(|a, b| b.status.as_u8().cmp(&a.status.as_u8()));
        Ok(new)
    }
}

#[get("/")]
async fn index(crawler: web::Data<Arc<Crawler>>) -> actix_web::Result<impl Responder> {
    let data = crawler.read().await;
    let Some(services) = data.as_ref() else {
        return Err(RedirectError::from(CrawlerError::CrawlerNotFetchedYet))?;
    };
    let template = IndexTemplate {
        services: &services.services,
        time: &services.time,
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

#[derive(Template)]
#[template(path = "cached_redirect.html", escape = "none")]
pub struct CachedRedirectTemplate {
    pub urls: Vec<Url>,
}

#[get("/@cached/{service_name}/{path:.*}")]
async fn cached_redirect(
    path: web::Path<(String, String)>,
    config: web::Data<AppConfig>,
    crawler: web::Data<Arc<Crawler>>,
) -> actix_web::Result<impl Responder> {
    let (service_name, _) = path.into_inner();

    let urls = crawler
        .get_redirect_urls_for_service(&service_name)
        .await
        .map_err(RedirectError::from)?;

    let template = CachedRedirectTemplate { urls };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .append_header((
            "cache-control",
            format!(
                "public, max-age={}, stale-while-revalidate=86400, stale-if-error=86400, immutable",
                config.crawler.ping_interval.as_secs()
            ),
        ))
        .body(template.render().expect("failed to render error page")))
}

#[derive(Template)]
#[template(path = "history_redirect.html", escape = "none")]
pub struct HistoryRedirectTemplate<'a> {
    pub path: &'a str,
}

#[get("/_/{service_name}/{path:.*}")]
async fn history_redirect(
    req: HttpRequest,
    path: web::Path<(String, String)>,
) -> actix_web::Result<impl Responder> {
    let (service_name, mut path) = path.into_inner();
    let query = req.query_string();
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }

    let path = format!("/{service_name}/{path}");
    let template = HistoryRedirectTemplate { path: &path };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .append_header(("refresh", format!("1; url={path}")))
        .body(template.render().expect("failed to render error page")))
}

#[get("/{service_name}/{path:.*}")]
async fn base_redirect(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    crawler: web::Data<Arc<Crawler>>,
) -> actix_web::Result<impl Responder> {
    let (service_name, mut path) = path.into_inner();
    let query = req.query_string();
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }

    let redirect_url = crawler
        .get_redirect_url_for_service(&service_name, &path)
        .await
        .map_err(RedirectError::from)?;

    debug!("Redirecting to {redirect_url}");

    Ok(Redirect::to(redirect_url.to_string()).temporary())
}
