use actix_web::{
    get,
    web::{self, Redirect},
    Responder, Scope,
};

use crate::config::AppConfig;

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("").service(index)
}

#[get("/")]
async fn index() -> actix_web::Result<impl Responder> {
    Ok(Redirect::to("https://github.com/cofob/fastside").permanent())
}
