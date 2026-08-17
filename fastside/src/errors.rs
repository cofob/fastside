use askama::Template;
use axum::{
    Json,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate<'a> {
    pub detail: &'a str,
    pub status_code: StatusCode,
}

#[derive(Serialize)]
pub struct ApiError {
    pub detail: String,
}

use crate::search::SearchError;

#[derive(Error, Debug)]
pub enum RedirectError {
    #[error("search error: `{0}`")]
    Search(#[from] SearchError),
    #[error("url parse error: `{0}`")]
    UrlParse(#[from] url::ParseError),
    #[error("user config error: `{0}`")]
    UserConfig(#[from] fastside_shared::errors::UserConfigError),
}

impl RedirectError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Search(error) => match error {
                SearchError::ServiceNotFound => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            },
            Self::UrlParse(_) | Self::UserConfig(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for RedirectError {
    fn into_response(self) -> Response {
        let status_code = self.status_code();
        let detail = self.to_string();
        let page = ErrorTemplate {
            detail: &detail,
            status_code,
        }
        .render()
        .expect("failed to render error page");
        (status_code, Html(page)).into_response()
    }
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct RedirectApiError(#[from] pub RedirectError);

impl IntoResponse for RedirectApiError {
    fn into_response(self) -> Response {
        let status_code = self.0.status_code();
        (
            status_code,
            Json(ApiError {
                detail: self.to_string(),
            }),
        )
            .into_response()
    }
}
