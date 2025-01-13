use axum::{
    response::{Html, IntoResponse, Response},
    http::StatusCode,
    Json,
};
use askama::Template;
use serde::Serialize;
use thiserror::Error;

// Error template struct for rendering HTML errors
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate<'a> {
    pub detail: &'a str,
    pub status_code: StatusCode,
}

// Helper macro for implementing IntoResponse for HTML errors
macro_rules! impl_template_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl IntoResponse for $err where $err: std::error::Error + 'static {
            fn into_response(self) -> Response {
                let status_code = self.status_code();
                let detail = format!("{}", self);
                let error_page = crate::errors::ErrorTemplate {
                    detail: &detail,
                    status_code,
                };
                let body = match error_page.render() {
                    Ok(rendered) => Html(rendered).into_response(),
                    Err(_) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to render error page".to_string()
                    ).into_response(),
                };

                (status_code, body).into_response()
            }
        }

        impl $err {
            pub fn status_code(&self) -> StatusCode {
                match self {
                    $(
                        $variant => $code,
                    )+
                }
            }
        }
    };
}
pub(crate) use impl_template_error;

// API error struct for JSON responses
#[derive(Serialize)]
pub struct ApiError {
    pub detail: String,
}

// Helper macro for implementing IntoResponse for JSON errors
macro_rules! impl_api_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl IntoResponse for $err where $err: std::error::Error + 'static {
            fn into_response(self) -> Response {
                let status_code = self.status_code();
                let detail = format!("{}", self);
                let error_response = Json(crate::errors::ApiError { detail });
                (status_code, error_response).into_response()
            }
        }

        impl $err {
            pub fn status_code(&self) -> StatusCode {
                match self {
                    $(
                        $variant => $code,
                    )+
                }
            }
        }
    };
}
pub(crate) use impl_api_error;

// RedirectError definition
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

impl_template_error!(RedirectError,
    status => {
        RedirectError::Search(s) => match s {
            SearchError::ServiceNotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR
        },
        RedirectError::UrlParse(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::UserConfig(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
);

// RedirectApiError definition
#[derive(Error, Debug)]
#[error(transparent)]
pub struct RedirectApiError(#[from] pub RedirectError);

impl_api_error!(RedirectApiError,
    status => {
        RedirectApiError(internal) => internal.status_code(),
    }
);
