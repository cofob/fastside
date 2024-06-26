use actix_web::http::StatusCode;
use askama::Template;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate<'a> {
    pub detail: &'a str,
    pub additional_data: &'a Option<serde_json::Value>,
    pub status_code: StatusCode,
}

pub trait AdditionalErrorData {
    fn additional_error_data(&self) -> Option<serde_json::Value>;
}

macro_rules! impl_api_error {
    ($err:ty, status => {$($variant:pat => $code:expr),+ $(,)?}) => {
        impl actix_web::ResponseError for $err where $err: std::error::Error + 'static {
            fn error_response(&self) -> actix_web::HttpResponse {
                use askama::Template;

                let detail = format!("{}", self);
                let additional_data = None;
                let error_page = crate::errors::ErrorTemplate { detail: &detail, additional_data: &additional_data, status_code: self.status_code() };

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
                use crate::errors::AdditionalErrorData;
                use askama::Template;

                let detail = format!("{}", self);
                let additional_data = self.additional_error_data();
                let error_page = crate::errors::ErrorTemplate { detail: &detail, additional_data: &additional_data, status_code: self.status_code() };

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

        impl crate::errors::AdditionalErrorData for $err {
            fn additional_error_data(&self) -> Option<serde_json::Value> {
                match self {
                    $(
                        $add_variant => $add_code,
                    )+
                }
            }
        }
    };
}
pub(crate) use impl_api_error;
