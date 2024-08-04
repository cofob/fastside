use actix_web::{post, web, Responder, Scope};
use fastside_shared::config::UserConfig;
use serde::{Deserialize, Serialize};

use crate::{
    config::AppConfig,
    crawler::Crawler,
    errors::{RedirectApiError, RedirectError},
    types::{LoadedData, Regexes},
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("/api/v1")
        .service(redirect)
        .service(make_user_config_string)
        .service(parse_user_config_string)
}

#[derive(Deserialize)]
struct RedirectRequest {
    url: String,
    #[serde(default)]
    config: UserConfig,
}

#[derive(Serialize)]
struct RedirectResponse {
    url: String,
    is_fallback: bool,
}

/// Get the redirect URL for a given URL
#[post("/redirect")]
async fn redirect(
    crawler: web::Data<Crawler>,
    loaded_data: web::Data<LoadedData>,
    regexes: web::Data<Regexes>,
    redirect_request: web::Json<RedirectRequest>,
) -> actix_web::Result<impl Responder> {
    let (url, is_fallback) = super::redirect::find_redirect(
        crawler.as_ref(),
        loaded_data.as_ref(),
        regexes.as_ref(),
        &redirect_request.config,
        &redirect_request.url,
    )
    .await
    .map_err(RedirectApiError)?;

    Ok(web::Json(RedirectResponse { url, is_fallback }))
}

/// Convert user config to a base64 encoded string
#[post("/make_user_config_string")]
async fn make_user_config_string(
    user_config: web::Json<UserConfig>,
) -> actix_web::Result<impl Responder> {
    Ok(web::Json(
        user_config
            .to_config_string()
            .map_err(RedirectError::from)
            .map_err(RedirectApiError)?,
    ))
}

/// Convert base64 encoded string to user config
#[post("/parse_user_config_string")]
async fn parse_user_config_string(
    user_config_string: web::Json<String>,
) -> actix_web::Result<impl Responder> {
    Ok(web::Json(
        UserConfig::from_config_string(&user_config_string)
            .map_err(RedirectError::from)
            .map_err(RedirectApiError)?,
    ))
}
