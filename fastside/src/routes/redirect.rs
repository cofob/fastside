use askama::Template;
use axum::{
    Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Method, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};

use crate::{
    crawler::{CrawledService, Crawler},
    errors::RedirectError,
    search::{
        SearchError, find_redirect_service_by_name, find_redirect_service_by_url,
        get_redirect_instance, get_redirect_instances,
    },
    types::{AppState, LoadedData, Regexes},
    utils::user_config::load_settings_cookie,
};
use fastside_shared::{
    config::{SelectMethod, UserConfig},
    serde_types::Service,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/@cached/{service_name}/", get(cached_redirect_root))
        .route("/@cached/{service_name}/{*path}", get(cached_redirect))
        .route("/_/", get(history_redirect_root))
        .route("/_/{*path}", get(history_redirect))
        .route("/{*path}", get(base_redirect).post(base_redirect))
}

#[derive(Template)]
#[template(path = "cached_redirect.html", escape = "none")]
pub struct CachedRedirectTemplate<'a> {
    pub urls: Vec<&'a url::Url>,
    pub select_method: &'a SelectMethod,
}

async fn cached_redirect(
    State(state): State<AppState>,
    Path((service_name, _)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, RedirectError> {
    cached_redirect_response(state, service_name, headers).await
}

async fn cached_redirect_root(
    State(state): State<AppState>,
    Path(service_name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, RedirectError> {
    cached_redirect_response(state, service_name, headers).await
}

async fn cached_redirect_response(
    state: AppState,
    service_name: String,
    headers: HeaderMap,
) -> Result<Response, RedirectError> {
    let loaded_data_guard = state.loaded_data.read().await;
    let user_config = load_settings_cookie(&headers, &loaded_data_guard.default_user_config);

    let guard = state.crawler.read().await;
    let (crawled_service, _) =
        find_redirect_service_by_name(&guard, &loaded_data_guard.services, &service_name).await?;
    let mut instances = get_redirect_instances(
        crawled_service,
        &user_config.required_tags,
        &user_config.forbidden_tags,
        &user_config.preferred_instances,
    )
    .ok_or(SearchError::NoInstancesFound)?;
    if user_config.select_method == SelectMethod::LowPing {
        instances.sort_by_key(|instance| instance.status.as_isize());
    }
    debug!("User config: {user_config:?}");

    let template = CachedRedirectTemplate {
        urls: instances.iter().map(|i| &i.url).collect(),
        select_method: &user_config.select_method,
    };

    Ok((
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            (
                "cache-control",
                format!(
                "public, max-age={}, stale-while-revalidate=86400, stale-if-error=86400, immutable",
                    state.config.crawler.ping_interval.as_secs()
                ),
            ),
        ],
        Html(
            template
                .render()
                .expect("failed to render cached redirect page"),
        ),
    )
        .into_response())
}

#[derive(Template)]
#[template(path = "history_redirect.html")]
pub struct HistoryRedirectTemplate<'a> {
    pub path: &'a str,
}

async fn history_redirect(Path(path): Path<String>, RawQuery(query): RawQuery) -> Response {
    history_redirect_response(path, query)
}

async fn history_redirect_root(RawQuery(query): RawQuery) -> Response {
    history_redirect_response(String::new(), query)
}

fn history_redirect_response(mut path: String, query: Option<String>) -> Response {
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        path.push('?');
        path.push_str(&query);
    }

    let path = format!("/{path}");
    let template = HistoryRedirectTemplate { path: &path };

    (
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            ("refresh", format!("1; url={path}")),
        ],
        Html(
            template
                .render()
                .expect("failed to render history redirect page"),
        ),
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "fallback_redirect.html", escape = "none")]
pub struct FallbackRedirectTemplate<'a> {
    pub fallback: &'a str,
}

pub(super) async fn find_redirect(
    crawler: &Crawler,
    loaded_data: &LoadedData,
    regexes: &Regexes,
    user_config: &UserConfig,
    path: &str,
) -> Result<(String, bool), RedirectError> {
    let is_url_query = if path.starts_with("http://") || path.starts_with("https://") {
        true
    } else {
        path.split('/').next().unwrap_or_default().contains('.')
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
    State(state): State<AppState>,
    method: Method,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response, RedirectError> {
    let loaded_data_guard = state.loaded_data.read().await;
    let user_config = load_settings_cookie(&headers, &loaded_data_guard.default_user_config);

    let (mut url, is_fallback) = find_redirect(
        &state.crawler,
        &loaded_data_guard,
        &state.regexes,
        &user_config,
        &path,
    )
    .await?;

    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(&query);
    }

    debug!("Redirecting to {url}, is_fallback: {is_fallback}");

    match (is_fallback, user_config.ignore_fallback_warning, method) {
        (true, false, Method::GET) => {
            let template = FallbackRedirectTemplate { fallback: &url };
            Ok((
                StatusCode::OK,
                [
                    ("content-type", "text/html; charset=utf-8".to_owned()),
                    ("refresh", format!("15; url={url}")),
                ],
                Html(
                    template
                        .render()
                        .expect("failed to render fallback redirect page"),
                ),
            )
                .into_response())
        }
        _ => Ok(Redirect::temporary(&url).into_response()),
    }
}
