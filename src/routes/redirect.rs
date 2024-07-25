use std::collections::HashMap;

use actix_web::{
    cookie::Cookie,
    get,
    http::{header::LOCATION, StatusCode},
    web::{self, Redirect},
    HttpRequest, Responder, Scope,
};
use askama::Template;
use base64::prelude::*;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{
    config::AppConfig,
    crawler::{CrawledService, Crawler},
    errors::impl_api_error,
    search::{
        find_redirect_service_by_name, find_redirect_service_by_url, get_redirect_instance,
        get_redirect_instances, SearchError,
    },
    serde_types::{LoadedData, SelectMethod, ServicesData, UserConfig},
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("")
        .service(index)
        .service(configure_page)
        .service(configure_save)
        .service(history_redirect)
        .service(cached_redirect)
        .service(base_redirect)
}

#[derive(Error, Debug)]
pub enum RedirectError {
    #[error("search error: `{0}`")]
    Search(#[from] SearchError),
    #[error("serialization error: `{0}`")]
    Serialization(#[from] serde_json::Error),
    #[error("urlencode error: `{0}`")]
    Base64Decode(#[from] base64::DecodeError),
}

impl_api_error!(RedirectError,
    status => {
        RedirectError::Search(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::Base64Decode(_) => StatusCode::INTERNAL_SERVER_ERROR,
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
        new.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
        Ok(new)
    }
}

#[get("/")]
async fn index(
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<LoadedData>,
) -> actix_web::Result<impl Responder> {
    let data = crawler.read().await;
    let Some(crawled_services) = data.as_ref() else {
        return Err(RedirectError::from(SearchError::CrawlerNotFetchedYet))?;
    };
    let template = IndexTemplate {
        services: &loaded_data.services,
        crawled_services: &crawled_services.services,
        time: &crawled_services.time,
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

#[derive(Template)]
#[template(path = "configure.html")]
pub struct ConfigureTemplate<'a> {
    current_config: &'a str,
}

#[get("/configure")]
async fn configure_page(
    req: HttpRequest,
    loaded_data: web::Data<LoadedData>,
) -> actix_web::Result<impl Responder> {
    let user_config = load_settings_cookie(&req, &loaded_data.default_settings);
    let json: String = serde_json::to_string(&user_config).map_err(RedirectError::Serialization)?;
    let data = BASE64_STANDARD.encode(json.as_bytes());

    let template = ConfigureTemplate {
        current_config: &data,
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

#[get("/configure/save")]
async fn configure_save(req: HttpRequest) -> actix_web::Result<impl Responder> {
    let query_string = req.query_string();
    let b64_decoded = BASE64_STANDARD
        .decode(query_string.as_bytes())
        .map_err(RedirectError::Base64Decode)?;
    let user_config: UserConfig =
        serde_json::from_slice(&b64_decoded).map_err(RedirectError::Serialization)?;
    let json: String = serde_json::to_string(&user_config).map_err(RedirectError::Serialization)?;
    let data = BASE64_STANDARD.encode(json.as_bytes());
    let cookie = Cookie::new("config", data);
    Ok(actix_web::HttpResponse::TemporaryRedirect()
        .cookie(cookie)
        .insert_header((LOCATION, "/configure?success"))
        .finish())
}

fn load_settings_cookie(req: &HttpRequest, default: &UserConfig) -> UserConfig {
    let cookie = match req.cookie("config") {
        Some(cookie) => cookie,
        None => {
            debug!("Cookie not found");
            return default.clone();
        }
    };
    let data = match BASE64_STANDARD.decode(cookie.value().as_bytes()) {
        Ok(data) => data,
        Err(_) => {
            debug!("invalid cookie data");
            return default.clone();
        }
    };
    match serde_json::from_slice(&data) {
        Ok(user_config) => user_config,
        Err(_) => {
            debug!("invalid cookie query string");
            default.clone()
        }
    }
}

#[derive(Template)]
#[template(path = "cached_redirect.html", escape = "none")]
pub struct CachedRedirectTemplate<'a> {
    pub urls: Vec<&'a reqwest::Url>,
    pub select_method: &'a SelectMethod,
}

#[get("/@cached/{service_name}/{path:.*}")]
async fn cached_redirect(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    config: web::Data<AppConfig>,
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<LoadedData>,
) -> actix_web::Result<impl Responder> {
    let (service_name, _) = path.into_inner();

    let user_config = load_settings_cookie(&req, &loaded_data.default_settings);

    let guard = crawler.read().await;
    let (crawled_service, _) =
        find_redirect_service_by_name(&guard, &loaded_data.services, &service_name)
            .await
            .map_err(RedirectError::from)?;
    let mut instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
    )
    .map_err(RedirectError::from)?;
    if user_config.select_method == SelectMethod::LowPing {
        instances.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
    }
    debug!("User config: {user_config:?}");

    let template = CachedRedirectTemplate {
        urls: instances.iter().map(|i| &i.url).collect(),
        select_method: &user_config.select_method,
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
    loaded_data: web::Data<LoadedData>,
    regexes: web::Data<HashMap<String, regex::Regex>>,
) -> actix_web::Result<impl Responder> {
    let path = path.into_inner();

    let is_url_query = if path.starts_with("http://") || path.starts_with("https://") {
        true
    } else {
        path[0..path.find('/').unwrap_or(0)].contains('.')
    };

    let guard = crawler.read().await;
    let (redir_path, crawled_service): (String, &CrawledService) = match is_url_query {
        true => {
            let (crawled_service, _, redir_path) =
                find_redirect_service_by_url(&guard, &loaded_data.services, &regexes, &path)
                    .await
                    .map_err(RedirectError::from)?;
            (redir_path, crawled_service)
        }
        false => {
            let service_name = path.split('/').next().unwrap();
            let redir_path = path[service_name.len()..].to_string();
            let (crawled_service, _) =
                find_redirect_service_by_name(&guard, &loaded_data.services, service_name)
                    .await
                    .map_err(RedirectError::from)?;
            (redir_path, crawled_service)
        }
    };

    let user_config = load_settings_cookie(&req, &loaded_data.default_settings);

    let redirect_instance =
        get_redirect_instance(crawled_service, &user_config).map_err(RedirectError::from)?;

    let mut url = redirect_instance
        .url
        .clone()
        .join(&redir_path)
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
