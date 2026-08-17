use axum::http::{HeaderMap, header::COOKIE};
use cookie::Cookie;
use fastside_shared::config::UserConfig;

pub fn load_settings_cookie(headers: &HeaderMap, default: &UserConfig) -> UserConfig {
    let config = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == "config")
        .map(|cookie| cookie.value().to_owned());

    let Some(config) = config else {
        debug!("Cookie not found");
        return default.clone();
    };

    UserConfig::from_config_string(&config).unwrap_or_else(|_| {
        debug!("invalid cookie");
        default.clone()
    })
}
