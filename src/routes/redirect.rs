use std::sync::Arc;

use actix_web::{
    get,
    http::StatusCode,
    web::{self, Redirect},
    Responder, Scope,
};
use thiserror::Error;

use crate::{
    config::AppConfig,
    crawler::{Crawler, CrawlerError},
    errors::impl_api_error,
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("").service(base_redirect)
}

#[derive(Error, Debug)]
pub enum RedirectError {
    #[error("crawler error: `{0}`")]
    CrawlerError(#[from] CrawlerError),
}

impl_api_error!(RedirectError,
    status => {
        RedirectError::CrawlerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    },
    data => {
        _ => None,
    }
);

#[get("/{service_name}/{path:.*}")]
async fn base_redirect(
    path: web::Path<(String, String)>,
    crawler: web::Data<Arc<Crawler>>,
) -> actix_web::Result<impl Responder> {
    let (service_name, path) = path.into_inner();

    let redirect_url = crawler
        .get_redirect_url_for_service(&service_name, &path)
        .await
        .map_err(RedirectError::from)?;

    debug!("Redirecting to {redirect_url}");

    Ok(Redirect::to(redirect_url.to_string()).temporary())
}
