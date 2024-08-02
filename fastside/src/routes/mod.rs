mod api;
mod config;
mod index;
mod redirect;

use actix_web::Scope;

use crate::config::AppConfig;

pub fn main_scope(config: &AppConfig) -> Scope {
    index::scope(config)
}
