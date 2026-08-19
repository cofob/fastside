use std::fmt::Debug;

use async_trait::async_trait;
use fastside_shared::config::CaptchaConfig;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("CAPTCHA verification failed: {0}")]
pub struct CaptchaError(pub String);

#[async_trait]
pub trait CaptchaVerifier: Debug + Send + Sync {
    async fn verify(
        &self,
        config: &CaptchaConfig,
        token: &str,
        remote_ip: Option<std::net::IpAddr>,
    ) -> Result<bool, CaptchaError>;
}

#[derive(Debug, Default)]
pub struct NoopCaptchaVerifier;

#[async_trait]
impl CaptchaVerifier for NoopCaptchaVerifier {
    async fn verify(
        &self,
        _config: &CaptchaConfig,
        _token: &str,
        _remote_ip: Option<std::net::IpAddr>,
    ) -> Result<bool, CaptchaError> {
        Ok(true)
    }
}

pub fn verification_fields(
    config: &CaptchaConfig,
    token: &str,
) -> std::collections::HashMap<String, String> {
    let mut fields = config.static_fields.clone();
    fields.insert(config.response_field.clone(), token.to_owned());
    if let Some(secret) = &config.secret {
        fields.insert(config.secret_field.clone(), secret.clone());
    }
    fields
}

pub fn verification_succeeded(config: &CaptchaConfig, body: &str) -> Result<bool, CaptchaError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CaptchaError(error.to_string()))?;
    Ok(value
        .pointer(&config.success_json_pointer)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

#[cfg(feature = "native")]
#[derive(Debug, Default)]
pub struct NativeCaptchaVerifier {
    client: reqwest::Client,
}

#[cfg(feature = "native")]
#[async_trait]
impl CaptchaVerifier for NativeCaptchaVerifier {
    async fn verify(
        &self,
        config: &CaptchaConfig,
        token: &str,
        _remote_ip: Option<std::net::IpAddr>,
    ) -> Result<bool, CaptchaError> {
        use fastside_shared::config::CaptchaEncoding;

        let verify_url = config
            .verify_url
            .as_deref()
            .ok_or_else(|| CaptchaError("verify_url is not configured".to_owned()))?;
        let fields = verification_fields(config, token);
        let mut request = self.client.post(verify_url).timeout(config.timeout);
        for (name, value) in &config.headers {
            request = request.header(name, value);
        }
        request = match config.encoding {
            CaptchaEncoding::Json => request.header("content-type", "application/json").body(
                serde_json::to_vec(&fields).map_err(|error| CaptchaError(error.to_string()))?,
            ),
            CaptchaEncoding::Form => request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(&fields)
                        .finish(),
                ),
        };
        let response = request
            .send()
            .await
            .map_err(|error| CaptchaError(error.to_string()))?;
        if !response.status().is_success() {
            return Ok(false);
        }
        let body = response
            .text()
            .await
            .map_err(|error| CaptchaError(error.to_string()))?;
        verification_succeeded(config, &body)
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
    use fastside_shared::config::{CaptchaConfig, CaptchaEncoding};
    use tokio::{sync::Mutex, task::JoinHandle};

    use super::{CaptchaVerifier, NativeCaptchaVerifier, verification_succeeded};

    async fn verification_server(
        response: &'static str,
        delay: Duration,
    ) -> (String, Arc<Mutex<Vec<(HeaderMap, String)>>>, JoinHandle<()>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = Router::new().route(
            "/siteverify",
            post(move |headers: HeaderMap, body: Bytes| {
                let captured = captured.clone();
                async move {
                    captured
                        .lock()
                        .await
                        .push((headers, String::from_utf8(body.to_vec()).unwrap()));
                    tokio::time::sleep(delay).await;
                    response
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/siteverify"), requests, handle)
    }

    #[test]
    fn reads_configured_success_pointer() {
        let mut config = CaptchaConfig::default();
        assert!(verification_succeeded(&config, r#"{"success":true}"#).unwrap());
        config.success_json_pointer = "/result/valid".to_owned();
        assert!(verification_succeeded(&config, r#"{"result":{"valid":true}}"#).unwrap());
        assert!(!verification_succeeded(&config, r#"{"success":false}"#).unwrap());
    }

    #[tokio::test]
    async fn cap_defaults_send_json_and_read_success() {
        let (url, requests, server) =
            verification_server(r#"{"success":true}"#, Duration::ZERO).await;
        let config = CaptchaConfig {
            enabled: true,
            verify_url: Some(url),
            secret: Some("test-secret".to_owned()),
            ..CaptchaConfig::default()
        };

        assert!(
            NativeCaptchaVerifier::default()
                .verify(&config, "test-token", None)
                .await
                .unwrap()
        );
        let requests = requests.lock().await;
        let (headers, body) = &requests[0];
        assert_eq!(headers["content-type"], "application/json");
        let fields: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(fields["secret"], "test-secret");
        assert_eq!(fields["response"], "test-token");
        server.abort();
    }

    #[tokio::test]
    async fn provider_token_names_work_with_form_verification() {
        for token_field in [
            "g-recaptcha-response",
            "cf-turnstile-response",
            "h-captcha-response",
        ] {
            let (url, requests, server) =
                verification_server(r#"{"success":true}"#, Duration::ZERO).await;
            let config = CaptchaConfig {
                enabled: true,
                token_field: token_field.to_owned(),
                verify_url: Some(url),
                secret: Some("provider-secret".to_owned()),
                encoding: CaptchaEncoding::Form,
                ..CaptchaConfig::default()
            };

            assert!(
                NativeCaptchaVerifier::default()
                    .verify(&config, "provider-token", None)
                    .await
                    .unwrap()
            );
            let requests = requests.lock().await;
            let (headers, body) = &requests[0];
            assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
            assert!(body.contains("secret=provider-secret"));
            assert!(body.contains("response=provider-token"));
            server.abort();
        }
    }

    #[tokio::test]
    async fn timeout_and_malformed_json_fail_closed() {
        let (url, _, server) =
            verification_server(r#"{"success":true}"#, Duration::from_millis(50)).await;
        let timeout_config = CaptchaConfig {
            verify_url: Some(url),
            timeout: Duration::from_millis(1),
            ..CaptchaConfig::default()
        };
        assert!(
            NativeCaptchaVerifier::default()
                .verify(&timeout_config, "token", None)
                .await
                .is_err()
        );
        server.abort();

        let (url, _, server) = verification_server("not-json", Duration::ZERO).await;
        let malformed_config = CaptchaConfig {
            verify_url: Some(url),
            ..CaptchaConfig::default()
        };
        assert!(
            NativeCaptchaVerifier::default()
                .verify(&malformed_config, "token", None)
                .await
                .is_err()
        );
        server.abort();
    }
}
