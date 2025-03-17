use askama::Template;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::get,
    Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::Deserialize;
use std::sync::Arc;

use crate::{types::AppState, utils::user_config::SettingsCookie};
use fastside_shared::config::UserConfig;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(configure_page))
        .route("/save", get(configure_save))
}

/// Template struct for rendering the configuration page.
#[derive(Template)]
#[template(path = "configure.html")]
pub struct ConfigureTemplate<'a> {
    current_config: &'a str,
}

/// Query parameters for configuration save route.
#[derive(Deserialize)]
struct QueryParams {
    config: Option<String>,
}

/// Handler for displaying the configuration page.
async fn configure_page(
    State(state): State<Arc<AppState>>,
    _jar: CookieJar,
    SettingsCookie(user_config): SettingsCookie,
) -> impl IntoResponse {
    let _loaded_data_guard = state.loaded_data.read().await;

    let template = ConfigureTemplate {
        current_config: &user_config
            .to_config_string()
            .unwrap_or_else(|_| "".to_string()),
    };

    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to render the configuration page",
        )
            .into_response(),
    }
}

/// Handler for saving configuration via query parameters.
async fn configure_save(Query(params): Query<QueryParams>, jar: CookieJar) -> impl IntoResponse {
    let query_string = params.config.unwrap_or_default();
    match UserConfig::from_config_string(&query_string) {
        Ok(user_config) => {
            let cookie_value = user_config
                .to_config_string()
                .unwrap_or_else(|_| "".to_string());

            let cookie = Cookie::new("config", cookie_value);

            (
                jar.add(cookie),
                Redirect::to("/configure?success").into_response(),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "Invalid configuration string provided",
        )
            .into_response(),
    }
}
