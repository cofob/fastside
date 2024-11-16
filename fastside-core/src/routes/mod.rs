// mod api;
mod config;
mod index;

// use actix_web::Scope;

use std::sync::Arc;

use axum::Router;

use crate::types::AppState;

pub fn main_router() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/", index::router())
        // .merge(redirect::router())
        .nest("/configure", config::router())
}
