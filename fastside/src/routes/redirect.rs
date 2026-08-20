use askama::Template;
use axum::{
    Router,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header::CACHE_CONTROL},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};

use crate::{
    crawler::{CrawledService, Crawler},
    errors::RedirectError,
    reputation::{append_cookie, csrf_token, last_instance_cookie},
    search::{
        SearchError, find_redirect_service_by_name, find_redirect_service_by_url,
        get_redirect_instance, get_redirect_instances,
    },
    types::{AppState, LoadedData, Regexes},
    utils::user_config::load_settings_cookie,
};
use fastside_shared::{
    config::{ReputationConfig, SelectMethod, UserConfig},
    serde_types::Service,
};
use serde::Serialize;
use tokio::sync::RwLock;

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
    pub instances_json: &'a str,
    pub select_method: &'a SelectMethod,
}

#[derive(Serialize)]
struct CachedInstance<'a> {
    url: &'a str,
    weight: f64,
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
    .ok_or(SearchError::NoInstancesFound)?
    .into_iter()
    .cloned()
    .collect::<Vec<_>>();
    drop(guard);
    drop(loaded_data_guard);
    if user_config.select_method == SelectMethod::LowPing {
        instances.sort_by_key(|instance| instance.status.as_isize());
    }
    debug!("User config: {user_config:?}");

    let reputations =
        if user_config.select_method == SelectMethod::Weighted && state.config.reputation.enabled {
            let urls = instances
                .iter()
                .map(|instance| instance.url.as_str().to_owned())
                .collect::<Vec<_>>();
            state
                .state_store
                .get_reputations(&urls)
                .await
                .unwrap_or_else(|error| {
                    warn!("Failed to load reputation for cached redirect: {error}");
                    std::collections::HashMap::new()
                })
        } else {
            std::collections::HashMap::new()
        };
    let cached_instances = instances
        .iter()
        .map(|instance| CachedInstance {
            url: instance.url.as_str(),
            weight: reputations
                .get(instance.url.as_str())
                .copied()
                .unwrap_or_default()
                .weight(
                    state.config.reputation.minimum_weight,
                    state.config.reputation.maximum_weight,
                ),
        })
        .collect::<Vec<_>>();
    let instances_json = serde_json::to_string(&cached_instances)
        .expect("cached redirect data is serializable")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let template = CachedRedirectTemplate {
        instances_json: &instances_json,
        select_method: &user_config.select_method,
    };
    let cache_control =
        if user_config.select_method == SelectMethod::Weighted && state.config.reputation.enabled {
            format!(
                "public, max-age={}, stale-while-revalidate=86400, stale-if-error=86400",
                state.config.crawler.ping_interval.as_secs().min(60)
            )
        } else {
            format!(
                "public, max-age={}, stale-while-revalidate=86400, stale-if-error=86400, immutable",
                state.config.crawler.ping_interval.as_secs()
            )
        };

    Ok((
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            ("cache-control", cache_control),
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
    pub path_json: &'a str,
    pub reputation_enabled: bool,
    pub captcha_enabled: bool,
    pub csrf_token: &'a str,
}

async fn history_redirect(
    State(state): State<AppState>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    history_redirect_response(state, path, query, headers)
}

async fn history_redirect_root(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    history_redirect_response(state, String::new(), query, headers)
}

fn history_redirect_response(
    state: AppState,
    mut path: String,
    query: Option<String>,
    headers: HeaderMap,
) -> Response {
    if path.starts_with('/') || path.contains('\\') || path.chars().any(char::is_control) {
        return (StatusCode::BAD_REQUEST, "invalid history redirect path").into_response();
    }
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        path.push('?');
        path.push_str(&query);
    }

    let path = format!("/{path}");
    let path_json = serde_json::to_string(&path)
        .expect("history redirect path is serializable")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let (csrf_token, csrf_cookie) =
        if state.config.reputation.enabled && !state.config.reputation.captcha.enabled {
            match csrf_token(&headers) {
                Ok(token) => token,
                Err(_) => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "failed to create CSRF token",
                    )
                        .into_response();
                }
            }
        } else {
            (String::new(), None)
        };
    let template = HistoryRedirectTemplate {
        path: &path,
        path_json: &path_json,
        reputation_enabled: state.config.reputation.enabled,
        captcha_enabled: state.config.reputation.captcha.enabled,
        csrf_token: &csrf_token,
    };

    let mut response = (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8".to_owned())],
        Html(
            template
                .render()
                .expect("failed to render history redirect page"),
        ),
    )
        .into_response();
    if !csrf_token.is_empty() {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    append_cookie(&mut response, csrf_cookie);
    response
}

#[derive(Template)]
#[template(path = "fallback_redirect.html", escape = "none")]
pub struct FallbackRedirectTemplate<'a> {
    pub fallback: &'a str,
}

pub(super) struct RedirectTarget {
    pub url: String,
    pub is_fallback: bool,
    pub instance: Option<String>,
}

pub(super) async fn find_redirect(
    crawler: &Crawler,
    loaded_data: &RwLock<LoadedData>,
    regexes: &Regexes,
    user_config: &UserConfig,
    reputation_config: &ReputationConfig,
    state_store: &dyn crate::storage::StateStore,
    path: &str,
) -> Result<RedirectTarget, RedirectError> {
    let is_url_query = if path.starts_with("http://") || path.starts_with("https://") {
        true
    } else {
        path.split('/').next().unwrap_or_default().contains('.')
    };

    let loaded_data = loaded_data.read().await;
    let guard = crawler.read().await;
    let (redir_path, crawled_service, service): (String, CrawledService, Service) =
        match is_url_query {
            true => {
                let (crawled_service, service, redir_path) =
                    find_redirect_service_by_url(&guard, &loaded_data.services, regexes, path)
                        .await
                        .map_err(RedirectError::from)?;
                (redir_path, crawled_service.clone(), service.clone())
            }
            false => {
                let service_name = path.split('/').next().unwrap();
                let redir_path = path[service_name.len()..].to_string();
                let (crawled_service, service) =
                    find_redirect_service_by_name(&guard, &loaded_data.services, service_name)
                        .await
                        .map_err(RedirectError::from)?;
                (redir_path, crawled_service.clone(), service.clone())
            }
        };
    drop(guard);
    drop(loaded_data);

    let (redirect_instance, is_fallback) = get_redirect_instance(
        &crawled_service,
        &service,
        user_config,
        reputation_config,
        state_store,
    )
    .await
    .map_err(RedirectError::from)?;

    let instance = (!is_fallback).then(|| redirect_instance.url.as_str().to_owned());
    let url = redirect_instance
        .url
        .clone()
        .join(&redir_path)
        .map_err(RedirectError::from)?
        .to_string();

    Ok(RedirectTarget {
        url,
        is_fallback,
        instance,
    })
}

async fn base_redirect(
    State(state): State<AppState>,
    method: Method,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response, RedirectError> {
    let user_config = {
        let loaded_data = state.loaded_data.read().await;
        load_settings_cookie(&headers, &loaded_data.default_user_config)
    };

    let mut target = find_redirect(
        &state.crawler,
        &state.loaded_data,
        &state.regexes,
        &user_config,
        &state.config.reputation,
        state.state_store.as_ref(),
        &path,
    )
    .await?;

    if let Some(query) = query.filter(|query| !query.is_empty()) {
        target.url.push('?');
        target.url.push_str(&query);
    }

    debug!(
        "Redirecting to {}, is_fallback: {}",
        target.url, target.is_fallback
    );

    let mut response = match (
        target.is_fallback,
        user_config.ignore_fallback_warning,
        method,
    ) {
        (true, false, Method::GET) => {
            let template = FallbackRedirectTemplate {
                fallback: &target.url,
            };
            (
                StatusCode::OK,
                [
                    ("content-type", "text/html; charset=utf-8".to_owned()),
                    ("refresh", format!("15; url={}", target.url)),
                ],
                Html(
                    template
                        .render()
                        .expect("failed to render fallback redirect page"),
                ),
            )
                .into_response()
        }
        _ => Redirect::temporary(&target.url).into_response(),
    };
    if state.config.reputation.enabled {
        append_cookie(
            &mut response,
            Some(last_instance_cookie(target.instance.as_deref())),
        );
    }
    Ok(response)
}
