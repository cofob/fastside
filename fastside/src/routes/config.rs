use askama::Template;
use axum::{
    Router,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, header::SET_COOKIE},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use cookie::Cookie;
use fastside_shared::config::UserConfig;

use crate::{errors::RedirectError, types::AppState, utils::user_config::load_settings_cookie};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/configure", get(configure_page))
        .route("/configure/save", get(configure_save))
}

#[derive(Template)]
#[template(path = "configure.html")]
pub struct ConfigureTemplate<'a> {
    current_config: &'a str,
}

async fn configure_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, RedirectError> {
    let loaded_data_guard = state.loaded_data.read().await;
    let user_config = load_settings_cookie(&headers, &loaded_data_guard.default_user_config);

    let template = ConfigureTemplate {
        current_config: &user_config.to_config_string()?,
    };

    Ok(axum::response::Html(
        template
            .render()
            .expect("failed to render configuration page"),
    ))
}

async fn configure_save(RawQuery(query): RawQuery) -> Result<Response, RedirectError> {
    let user_config = UserConfig::from_config_string(query.as_deref().unwrap_or_default())?;
    let lifetime = time::Duration::days(9999);
    let cookie = Cookie::build(("config", user_config.to_config_string()?))
        .path("/")
        .expires(time::OffsetDateTime::now_utc() + lifetime)
        .max_age(lifetime)
        .build();
    let mut response = Redirect::temporary("/configure?success").into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie.to_string()).expect("generated cookie is valid"),
    );
    Ok(response)
}
