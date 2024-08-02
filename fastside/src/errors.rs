use actix_web::http::StatusCode;
use askama::Template;
use serde::Serialize;
use thiserror::Error;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate<'a> {
    pub detail: &'a str,
    pub status_code: StatusCode,
}

macro_rules! impl_template_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl actix_web::ResponseError for $err where $err: std::error::Error + 'static {
            fn error_response(&self) -> actix_web::HttpResponse {
                use askama::Template;

                let detail = format!("{}", self);
                let error_page = crate::errors::ErrorTemplate { detail: &detail, status_code: self.status_code() };

                actix_web::HttpResponse::build(self.status_code()).body(error_page.render().expect("failed to render error page"))
            }

            fn status_code(&self) -> actix_web::http::StatusCode {
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

#[derive(Serialize)]
pub struct ApiError {
    pub detail: String,
}

macro_rules! impl_api_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl actix_web::ResponseError for $err where $err: std::error::Error + 'static {
            fn error_response(&self) -> actix_web::HttpResponse {
                let detail = format!("{}", self);
                actix_web::HttpResponse::build(self.status_code()).json(crate::errors::ApiError { detail })
            }

            fn status_code(&self) -> actix_web::http::StatusCode {
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

use crate::search::SearchError;

#[derive(Error, Debug)]
pub enum RedirectError {
    #[error("search error: `{0}`")]
    Search(#[from] SearchError),
    #[error("url parse error: `{0}`")]
    UrlParse(#[from] url::ParseError),
    #[error("user config error: `{0}`")]
    UserConfig(#[from] fastside_shared::serde_types::UserConfigError),
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

#[derive(Error, Debug)]
#[error(transparent)]
pub struct RedirectApiError(#[from] pub RedirectError);

impl_api_error!(RedirectApiError,
    status => {
        RedirectApiError(internal) => internal.status_code(),
    }
);
