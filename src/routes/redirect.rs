use std::collections::HashMap;

use actix_web::{
    get,
    http::StatusCode,
    web::{self, Redirect},
    HttpRequest, Responder, Scope,
};
use askama::Template;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{
    config::AppConfig,
    crawler::{CrawledService, Crawler},
    errors::impl_api_error,
    search::{
        find_redirect_service_by_name, get_redirect_instances, get_redirect_random_instance,
        SearchError,
    },
    serde_types::ServicesData,
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
    #[error("search error: `{0}`")]
    SearchError(#[from] SearchError),
}

impl_api_error!(RedirectError,
    status => {
        RedirectError::SearchError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    },
    data => {
        _ => None,
    }
);

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate<'a> {
    pub crawled_services: &'a HashMap<String, CrawledService>,
    pub services: &'a ServicesData,
    pub time: &'a DateTime<Utc>,
}

mod filters {
    use crate::crawler::CrawledInstance;

    pub fn sort_list(l: &[CrawledInstance]) -> ::askama::Result<Vec<CrawledInstance>> {
        let mut new = l.to_owned();
        new.sort_by_key(|i| i.status.as_u8());
        new.reverse();
        Ok(new)
    }
}

#[get("/")]
async fn index(
    crawler: web::Data<Crawler>,
    services: web::Data<ServicesData>,
) -> actix_web::Result<impl Responder> {
    let data = crawler.read().await;
    let Some(crawled_services) = data.as_ref() else {
        return Err(RedirectError::from(SearchError::CrawlerNotFetchedYet))?;
    };
    let template = IndexTemplate {
        services: &services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

#[derive(Template)]
#[template(path = "cached_redirect.html", escape = "none")]
pub struct CachedRedirectTemplate<'a> {
    pub urls: Vec<&'a reqwest::Url>,
}

#[get("/@cached/{service_name}/{path:.*}")]
async fn cached_redirect(
    path: web::Path<(String, String)>,
    config: web::Data<AppConfig>,
    crawler: web::Data<Crawler>,
    services: web::Data<ServicesData>,
) -> actix_web::Result<impl Responder> {
    let (service_name, _) = path.into_inner();

    let guard = crawler.read().await;
    let (crawled_service, _) =
        find_redirect_service_by_name(&guard, services.as_ref(), &service_name)
            .await
            .map_err(RedirectError::from)?;
    let instances = get_redirect_instances(crawled_service, &[]).map_err(RedirectError::from)?;

    let template = CachedRedirectTemplate {
        urls: instances.iter().map(|i| &i.url).collect(),
    };

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

#[get("/{path:.*}")]
async fn base_redirect(
    req: HttpRequest,
    path: web::Path<String>,
    crawler: web::Data<Crawler>,
    services: web::Data<ServicesData>,
) -> actix_web::Result<impl Responder> {
    let path = path.into_inner();

    let is_url_query = path.starts_with("http://") || path.starts_with("https://");
    if is_url_query {
        todo!("Implement redirecting to URLs");
    }

    let (search_term, redir_path): (&str, &str) = if is_url_query {
        (&path, "")
    } else {
        let s = path.split('/').next().unwrap();
        (s, &path[s.len()..])
    };

    let guard = crawler.read().await;
    let (crawled_service, _) =
        find_redirect_service_by_name(&guard, services.as_ref(), search_term)
            .await
            .map_err(RedirectError::from)?;
    let redirect_instance =
        get_redirect_random_instance(crawled_service, &[]).map_err(RedirectError::from)?;

    let mut url = redirect_instance
        .url
        .clone()
        .join(redir_path)
        .unwrap()
        .to_string();
    let query = req.query_string();
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }

    debug!("Redirecting to {url}");

    Ok(Redirect::to(url.to_string()).temporary())
}
