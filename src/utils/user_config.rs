use actix_web::HttpRequest;
use base64::prelude::*;

use crate::serde_types::UserConfig;

pub fn load_settings_cookie(req: &HttpRequest, default: &UserConfig) -> UserConfig {
    let cookie = match req.cookie("config") {
        Some(cookie) => cookie,
        None => {
            debug!("Cookie not found");
            return default.clone();
        }
    };
    let data = match BASE64_STANDARD.decode(cookie.value().as_bytes()) {
        Ok(data) => data,
        Err(_) => {
            debug!("invalid cookie data");
            return default.clone();
        }
    };
    match serde_json::from_slice(&data) {
        Ok(user_config) => user_config,
        Err(_) => {
            debug!("invalid cookie query string");
            default.clone()
        }
    }
}
