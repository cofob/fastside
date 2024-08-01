use actix_web::HttpRequest;

use crate::serde_types::UserConfig;

pub fn load_settings_cookie(req: &HttpRequest, default: &UserConfig) -> UserConfig {
    let cookie = match req.cookie("config") {
        Some(cookie) => cookie,
        None => {
            debug!("Cookie not found");
            return default.clone();
        }
    };
    UserConfig::from_config_string(cookie.value()).unwrap_or_else(|_| {
        debug!("invalid cookie");
        default.clone()
    })
}
