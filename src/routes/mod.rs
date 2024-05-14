mod index;
mod redirect;

use actix_web::{web, Scope};

use crate::config::AppConfig;

pub fn main_scope(config: &AppConfig) -> Scope {
    web::scope("")
        .service(index::scope(config))
        .service(redirect::scope(config))
}
