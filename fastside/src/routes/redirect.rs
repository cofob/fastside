use actix_web::{
    HttpRequest, Responder, Scope, get,
    http::{Method, header::LOCATION},
    web,
};
use askama::Template;
use tokio::sync::RwLock;

use crate::{
    config::AppConfig,
    crawler::{CrawledService, Crawler},
    errors::RedirectError,
    search::{
        SearchError, find_redirect_service_by_name, find_redirect_service_by_url,
        get_redirect_instance, get_redirect_instances,
    },
    types::{LoadedData, Regexes},
    utils::user_config::load_settings_cookie,
};
use fastside_shared::{
    config::{SelectMethod, UserConfig},
    serde_types::Service,
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("")
        .service(history_redirect)
        .service(cached_redirect)
        .route("/{path:.*}", web::get().to(base_redirect))
        .route("/{path:.*}", web::post().to(base_redirect))
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
    loaded_data: web::Data<RwLock<LoadedData>>,
) -> actix_web::Result<impl Responder> {
    let (service_name, _) = path.into_inner();

    let loaded_data_guard = loaded_data.read().await;
    let user_config = load_settings_cookie(&req, &loaded_data_guard.default_user_config);

    let guard = crawler.read().await;
    let (crawled_service, _) =
        find_redirect_service_by_name(&guard, &loaded_data_guard.services, &service_name)
            .await
            .map_err(RedirectError::from)?;
    let mut instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
        &user_config.preferred_instances,
    )
    .ok_or(RedirectError::from(SearchError::NoInstancesFound))?;
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
#[template(path = "history_redirect.html")]
pub struct HistoryRedirectTemplate<'a> {
    pub path: &'a str,
}

#[get("/_/{path:.*}")]
async fn history_redirect(
    req: HttpRequest,
    path: web::Path<String>,
) -> actix_web::Result<impl Responder> {
    let mut path = path.into_inner();
    let query = req.query_string();
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }

    let path = format!("/{path}");
    let template = HistoryRedirectTemplate { path: &path };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .append_header(("refresh", format!("1; url={path}")))
        .body(template.render().expect("failed to render error page")))
}

#[derive(Template)]
#[template(path = "fallback_redirect.html", escape = "none")]
pub struct FallbackRedirectTemplate<'a> {
    pub fallback: &'a str,
}

pub async fn find_redirect(
    crawler: &Crawler,
    loaded_data: &LoadedData,
    regexes: &Regexes,
    user_config: &UserConfig,
    path: &str,
) -> Result<(String, bool), RedirectError> {
    let is_url_query = if path.starts_with("http://") || path.starts_with("https://") {
        true
    } else {
        path[0..path.find('/').unwrap_or(0)].contains('.')
    };

    let guard = crawler.read().await;
    let (redir_path, crawled_service, service): (String, &CrawledService, &Service) =
        match is_url_query {
            true => {
                let (crawled_service, service, redir_path) =
                    find_redirect_service_by_url(&guard, &loaded_data.services, regexes, path)
                        .await
                        .map_err(RedirectError::from)?;
                (redir_path, crawled_service, service)
            }
            false => {
                let service_name = path.split('/').next().unwrap();
                let redir_path = path[service_name.len()..].to_string();
                let (crawled_service, service) =
                    find_redirect_service_by_name(&guard, &loaded_data.services, service_name)
                        .await
                        .map_err(RedirectError::from)?;
                (redir_path, crawled_service, service)
            }
        };

    let (redirect_instance, is_fallback) =
        get_redirect_instance(crawled_service, service, user_config)
            .map_err(RedirectError::from)?;

    let url = redirect_instance
        .url
        .clone()
        .join(&redir_path)
        .map_err(RedirectError::from)?
        .to_string();

    Ok((url, is_fallback))
}

async fn base_redirect(
    req: HttpRequest,
    path: web::Path<String>,
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<RwLock<LoadedData>>,
    regexes: web::Data<Regexes>,
) -> actix_web::Result<impl Responder> {
    let path = path.into_inner();

    let loaded_data_guard = loaded_data.read().await;
    let user_config = load_settings_cookie(&req, &loaded_data_guard.default_user_config);

    let (mut url, is_fallback) = find_redirect(
        crawler.get_ref(),
        &loaded_data_guard,
        regexes.get_ref(),
        &user_config,
        &path,
    )
    .await?;

    let query = req.query_string();
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }

    debug!("Redirecting to {url}, is_fallback: {is_fallback}");

    match (
        is_fallback,
        user_config.ignore_fallback_warning,
        req.method(),
    ) {
        (true, false, &Method::GET) => {
            let template = FallbackRedirectTemplate { fallback: &url };
            Ok(actix_web::HttpResponse::Ok()
                .content_type("text/html; charset=utf-8")
                .insert_header(("refresh", format!("15; url={url}")))
                .body(
                    template
                        .render()
                        .expect("failed to render fallback redirect page"),
                ))
        }
        _ => Ok(actix_web::HttpResponse::TemporaryRedirect()
            .insert_header((LOCATION, url.to_string()))
            .finish()),
    }
}
