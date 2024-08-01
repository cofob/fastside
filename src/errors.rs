use actix_web::http::StatusCode;
use askama::Template;
use thiserror::Error;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate<'a> {
    pub detail: &'a str,
    pub status_code: StatusCode,
}

macro_rules! impl_api_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl actix_web::ResponseError for $err where $err: std::error::Error + 'static {
            fn error_response(&self) -> actix_web::HttpResponse {
                use askama::Template;

                let detail = format!("{}", self);
                let additional_data = None;
                let error_page = crate::errors::ErrorTemplate { detail: &detail, status_code: self.status_code() };

                actix_web::HttpResponse::build(self.status_code()).html(error_page.render().expect("failed to render error page"))
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

    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}$(,)? data => {$($add_variant:pat => $add_code:expr),+ $(,)?}) => {
        impl actix_web::ResponseError for $err where $err: std::error::Error + 'static {
            fn error_response(&self) -> actix_web::HttpResponse {
                use askama::Template;

                let detail = format!("{}", self);
                let error_page = crate::errors::ErrorTemplate { detail: &detail, status_code: self.status_code() };

                actix_web::HttpResponse::build(self.status_code()).content_type("text/html; charset=utf-8").body(error_page.render().expect("failed to render error page"))
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
    #[error("serialization error: `{0}`")]
    Serialization(#[from] serde_json::Error),
    #[error("urlencode error: `{0}`")]
    Base64Decode(#[from] base64::DecodeError),
    #[error("url parse error: `{0}`")]
    UrlParse(#[from] url::ParseError),
}

impl_api_error!(RedirectError,
    status => {
        RedirectError::Search(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::Base64Decode(_) => StatusCode::INTERNAL_SERVER_ERROR,
        RedirectError::UrlParse(_) => StatusCode::INTERNAL_SERVER_ERROR,
    },
    data => {
        _ => None,
    }
);
