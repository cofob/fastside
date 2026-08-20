use axum::{
    Json, Router,
    extract::{FromRequest, Request, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header::USER_AGENT},
    response::{IntoResponse, Response},
    routing::post,
};
use fastside_shared::config::UserConfig;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    errors::{RedirectApiError, RedirectError},
    types::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/redirect", post(redirect))
        .route(
            "/api/v1/make_user_config_string",
            post(make_user_config_string),
        )
        .route(
            "/api/v1/parse_user_config_string",
            post(parse_user_config_string),
        )
}

struct ApiJson<T>(T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = (StatusCode, String);

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection: JsonRejection| {
                let status = match rejection.status() {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE | StatusCode::UNPROCESSABLE_ENTITY => {
                        StatusCode::BAD_REQUEST
                    }
                    status => status,
                };
                (status, rejection.body_text())
            })
    }
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

/// Check whether the request was made with cURL.
fn is_curl_request(headers: &HeaderMap) -> bool {
    headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("curl"))
}

/// Get the redirect URL for a given URL
async fn redirect(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(redirect_request): ApiJson<RedirectRequest>,
) -> Result<Response, RedirectApiError> {
    let loaded_data_guard = state.loaded_data.read().await;
    let (url, is_fallback) = super::redirect::find_redirect(
        &state.crawler,
        &loaded_data_guard,
        &state.regexes,
        &redirect_request.config,
        &redirect_request.url,
    )
    .await
    .map_err(RedirectApiError)?;

    if is_curl_request(&headers) {
        Ok(url.into_response())
    } else {
        Ok(Json(RedirectResponse { url, is_fallback }).into_response())
    }
}

/// Convert user config to a base64 encoded string
async fn make_user_config_string(
    ApiJson(user_config): ApiJson<UserConfig>,
) -> Result<Json<String>, RedirectApiError> {
    Ok(Json(
        user_config
            .to_config_string()
            .map_err(RedirectError::from)
            .map_err(RedirectApiError)?,
    ))
}

/// Convert base64 encoded string to user config
async fn parse_user_config_string(
    ApiJson(user_config_string): ApiJson<String>,
) -> Result<Json<UserConfig>, RedirectApiError> {
    Ok(Json(
        UserConfig::from_config_string(&user_config_string)
            .map_err(RedirectError::from)
            .map_err(RedirectApiError)?,
    ))
}
