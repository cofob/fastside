use axum::{
    async_trait,
    extract::FromRequestParts,
    http::request::Parts,
};
use axum_extra::extract::CookieJar;
use fastside_shared::config::UserConfig;
use tracing::debug;

pub struct SettingsCookie(pub UserConfig);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for SettingsCookie {
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Extract the CookieJar from the request parts
        let cookie_jar = match CookieJar::from_request_parts(parts, state).await {
            Ok(cookies) => cookies,
            Err(_) => {
                debug!("Failed to extract CookieJar");
                return Err(());
            }
        };

        // Retrieve the "config" cookie
        if let Some(config_cookie) = cookie_jar.get("config") {
            match UserConfig::from_config_string(config_cookie.value()) {
                Ok(user_config) => Ok(SettingsCookie(user_config)),
                Err(_) => {
                    debug!("Invalid config cookie format");
                    Err(())
                }
            }
        } else {
            debug!("Config cookie not found");
            Err(())
        }
    }
}
