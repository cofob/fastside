use axum::{
    extract::{Path, Query, State},
    http::Method,
    response::{Html, IntoResponse, Redirect},
    routing::get,
    Router,
};
use askama::Template;
use std::{collections::HashMap, sync::Arc};

use crate::{
    crawler::{Crawler, CrawledService},
    errors::RedirectError,
    search::{
        find_redirect_service_by_name, find_redirect_service_by_url, get_redirect_instance,
        get_redirect_instances, SearchError,
    },
    types::{AppState, LoadedData},
    utils::user_config::SettingsCookie,
};
use fastside_shared::{
    config::{SelectMethod, UserConfig},
    serde_types::Service,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/@cached/:service_name/*path", get(cached_redirect))
        .route("/_/*path", get(history_redirect))
        .route("/*path", get(base_redirect).post(base_redirect))
}

#[derive(Template)]
#[template(path = "cached_redirect.html", escape = "none")]
pub struct CachedRedirectTemplate<'a> {
    pub urls: Vec<&'a url::Url>,
    pub select_method: &'a SelectMethod,
}

async fn cached_redirect(
    State(state): State<Arc<AppState>>,
    Path((service_name, _)): Path<(String, String)>,
    settings_cookie: Option<SettingsCookie>,
) -> impl IntoResponse {
    let loaded_data_guard = state.loaded_data.read().await;
    
    // Use user config from cookie or fall back to default
    let user_config = match settings_cookie {
        Some(SettingsCookie(config)) => config,
        None => loaded_data_guard.default_user_config.clone(),
    };

    let guard = state.crawler.read().await;
    match find_redirect_service_by_name(&guard, &loaded_data_guard.services, &service_name).await {
        Ok((crawled_service, _)) => {
            match get_redirect_instances(
                crawled_service,
                &user_config.required_tags,
                &user_config.forbidden_tags,
                &user_config.preferred_instances,
            ) {
                Some(mut instances) => {
                    if user_config.select_method == SelectMethod::LowPing {
                        instances.sort_by(|a, b| a.status.as_isize().cmp(&b.status.as_isize()));
                    }
                    debug!("User config: {user_config:?}");

                    let template = CachedRedirectTemplate {
                        urls: instances.iter().map(|i| &i.url).collect(),
                        select_method: &user_config.select_method,
                    };

                    let cache_control = format!(
                        "public, max-age={}, stale-while-revalidate=86400, stale-if-error=86400, immutable",
                        state.config.crawler.ping_interval.as_secs()
                    );

                    match template.render() {
                        Ok(rendered) => (
                            axum::http::StatusCode::OK,
                            [
                                ("cache-control", cache_control),
                                ("content-type", "text/html; charset=utf-8".to_string()),
                            ],
                            Html(rendered),
                        )
                            .into_response(),
                        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
                None => RedirectError::from(SearchError::NoInstancesFound).into_response(),
            }
        }
        Err(err) => RedirectError::from(err).into_response(),
    }
}

#[derive(Template)]
#[template(path = "history_redirect.html")]
pub struct HistoryRedirectTemplate<'a> {
    pub path: &'a str,
}

async fn history_redirect(
    Query(query): Query<HashMap<String, String>>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    let mut full_path = format!("/{path}");
    
    // Add query parameters if present
    if !query.is_empty() {
        full_path.push('?');
        let query_string: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        full_path.push_str(&query_string.join("&"));
    }

    let template = HistoryRedirectTemplate { path: &full_path };

    match template.render() {
        Ok(rendered) => (
            axum::http::StatusCode::OK,
            [
                ("content-type", "text/html; charset=utf-8"),
                ("refresh", &format!("1; url={full_path}")),
            ],
            Html(rendered),
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Template)]
#[template(path = "fallback_redirect.html", escape = "none")]
pub struct FallbackRedirectTemplate<'a> {
    pub fallback: &'a str,
}

pub async fn find_redirect(
    crawler: &Crawler,
    loaded_data: &LoadedData,
    regexes: &crate::types::Regexes,
    user_config: &UserConfig,
    path: &str,
) -> Result<(String, bool), RedirectError> {
    let is_url_query = if path.starts_with("http://") || path.starts_with("https://") {
        true
    } else {
        path[0..path.find('/').unwrap_or(path.len())].contains('.')
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
                let service_name = path.split('/').next().unwrap_or("");
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
    State(state): State<Arc<AppState>>,
    method: Method,
    Query(query): Query<HashMap<String, String>>,
    Path(path): Path<String>,
    settings_cookie: Option<SettingsCookie>,
) -> impl IntoResponse {
    let loaded_data_guard = state.loaded_data.read().await;
    
    // Use provided user config or fall back to default
    let user_config = match settings_cookie {
        Some(SettingsCookie(config)) => config,
        None => loaded_data_guard.default_user_config.clone(),
    };

    match find_redirect(
        &state.crawler,
        &loaded_data_guard,
        &state.regexes,
        &user_config,
        &path,
    )
    .await {
        Ok((mut url, is_fallback)) => {
            // Add query parameters if present
            if !query.is_empty() {
                url.push('?');
                let query_string: Vec<String> = query
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect();
                url.push_str(&query_string.join("&"));
            }

            debug!("Redirecting to {url}, is_fallback: {is_fallback}");

            match (is_fallback, user_config.ignore_fallback_warning, method) {
                (true, false, Method::GET) => {
                    let template = FallbackRedirectTemplate { fallback: &url };
                    match template.render() {
                        Ok(rendered) => (
                            axum::http::StatusCode::OK,
                            [
                                ("content-type", "text/html; charset=utf-8"),
                                ("refresh", &format!("15; url={url}")),
                            ],
                            Html(rendered),
                        )
                            .into_response(),
                        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
                _ => Redirect::temporary(&url).into_response(),
            }
        }
        Err(e) => e.into_response(),
    }
}
