use std::sync::Arc;

use actix_web::{
    get,
    http::StatusCode,
    web::{self, Redirect},
    Responder, Scope,
};
use askama::Template;
use thiserror::Error;

use crate::{
    config::AppConfig,
    crawler::{Crawler, CrawlerError},
    errors::impl_api_error,
};

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("")
        .service(history_redirect)
        .service(base_redirect)
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

#[derive(Template)]
#[template(path = "history_redirect.html")]
pub struct HistoryRedirectTemplate<'a> {
    pub path: &'a str,
}

#[get("/_/{service_name}/{path:.*}")]
async fn history_redirect(path: web::Path<(String, String)>) -> actix_web::Result<impl Responder> {
    let (service_name, path) = path.into_inner();

    let path = format!("/{service_name}/{path}");
    let template = HistoryRedirectTemplate { path: &path };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .append_header(("refresh", format!("1; url={path}")))
        .body(template.render().expect("failed to render error page")))
}

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
