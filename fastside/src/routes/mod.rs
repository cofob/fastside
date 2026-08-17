mod api;
mod config;
mod index;
mod redirect;

use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::Response,
};

use crate::types::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(index::router())
        .merge(config::router())
        .merge(api::router())
        .merge(redirect::router())
        .layer(middleware::from_fn(log_request))
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let response = next.run(request).await;
    info!("{method} {uri} {}", response.status());
    response
}
