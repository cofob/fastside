use actix_web::{cookie::Cookie, get, http::header::LOCATION, web, HttpRequest, Responder, Scope};
use askama::Template;

use crate::{
    config::AppConfig, errors::RedirectError, types::LoadedData,
    utils::user_config::load_settings_cookie,
};
use fastside_shared::serde_types::UserConfig;

pub fn scope(_config: &AppConfig) -> Scope {
    web::scope("/configure")
        .service(configure_page)
        .service(configure_save)
}

#[derive(Template)]
#[template(path = "configure.html")]
pub struct ConfigureTemplate<'a> {
    current_config: &'a str,
}

#[get("")]
async fn configure_page(
    req: HttpRequest,
    loaded_data: web::Data<LoadedData>,
) -> actix_web::Result<impl Responder> {
    let user_config = load_settings_cookie(&req, &loaded_data.default_settings);

    let template = ConfigureTemplate {
        current_config: &user_config
            .to_config_string()
            .map_err(RedirectError::from)?,
    };

    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(template.render().expect("failed to render error page")))
}

#[get("/save")]
async fn configure_save(req: HttpRequest) -> actix_web::Result<impl Responder> {
    let query_string = req.query_string();
    let user_config = UserConfig::from_config_string(query_string).map_err(RedirectError::from)?;
    let cookie = Cookie::build(
        "config",
        user_config
            .to_config_string()
            .map_err(RedirectError::from)?,
    )
    .path("/")
    .expires(time::OffsetDateTime::now_utc() + time::Duration::days(9999))
    .max_age(time::Duration::days(9999))
    .finish();
    Ok(actix_web::HttpResponse::TemporaryRedirect()
        .cookie(cookie)
        .insert_header((LOCATION, "/configure?success"))
        .finish())
}
