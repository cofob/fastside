//! Shared Fastside HTTP application.

pub mod captcha;
pub mod crawler;
mod errors;
mod filters;
pub mod reputation;
mod routes;
mod search;
pub mod storage;
pub mod types;
mod utils;

use axum::Router;
use types::AppState;

#[deny(unused_imports, unused_mut, unused_variables, unsafe_code)]
#[macro_use]
extern crate log;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn app(state: AppState) -> Router {
    routes::router().with_state(state)
}
