use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use askama::Template;
#[cfg(feature = "native")]
use axum::extract::ConnectInfo;
use axum::{
    Form, Router,
    extract::{
        FromRequestParts, Query, State,
        rejection::{FormRejection, QueryRejection},
    },
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, SET_COOKIE},
        request::Parts,
    },
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use cookie::{Cookie, SameSite};
use fastside_shared::config::{IpProtectionConfig, ReputationConfig};
use ipnet::IpNet;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{errors::ErrorTemplate, storage::VoteDirection, types::AppState};

pub const CSRF_COOKIE: &str = "fastside_csrf";
pub const LAST_INSTANCE_COOKIE: &str = "fastside_last_instance";

#[derive(Debug)]
pub(crate) struct CsrfTokenError;

pub fn validate_config(config: &fastside_shared::config::AppConfig) -> Result<(), String> {
    let reputation = &config.reputation;
    if !reputation.minimum_weight.is_finite()
        || !reputation.maximum_weight.is_finite()
        || reputation.minimum_weight <= 0.0
        || reputation.maximum_weight < reputation.minimum_weight
    {
        return Err(
            "reputation weights must be finite, positive, and ordered from minimum to maximum"
                .to_owned(),
        );
    }
    if reputation.captcha.enabled {
        if reputation.captcha.widget_html.trim().is_empty() {
            return Err("reputation.captcha.widget_html is required".to_owned());
        }
        if reputation.captcha.token_field.trim().is_empty() {
            return Err("reputation.captcha.token_field is required".to_owned());
        }
        if reputation.captcha.verify_url.is_none() {
            return Err("reputation.captcha.verify_url is required".to_owned());
        }
        if reputation.captcha.timeout.is_zero() {
            return Err("reputation.captcha.timeout must be greater than zero".to_owned());
        }
    }
    if reputation.ip_protection.rate_limit.enabled
        && reputation.ip_protection.rate_limit.max_votes == 0
    {
        return Err("reputation IP rate limit max_votes must be greater than zero".to_owned());
    }
    if reputation.ip_protection.rate_limit.enabled
        && reputation.ip_protection.rate_limit.window.is_zero()
    {
        return Err("reputation IP rate limit window must be greater than zero".to_owned());
    }
    if reputation.ip_protection.one_vote_per_instance.enabled
        && reputation
            .ip_protection
            .one_vote_per_instance
            .window
            .is_zero()
    {
        return Err("reputation one-vote window must be greater than zero".to_owned());
    }
    for network in &reputation.ip_protection.client_ip.trusted_proxies {
        IpNet::from_str(network)
            .map_err(|error| format!("invalid trusted proxy network {network}: {error}"))?;
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/reputation/vote", get(vote_confirmation).post(submit_vote))
}

#[derive(Debug, Default)]
struct ProtectionState {
    votes: HashMap<IpAddr, VecDeque<Instant>>,
    instance_votes: HashMap<(IpAddr, String), Instant>,
}

#[derive(Debug, Default)]
pub struct VoteProtector {
    state: Mutex<ProtectionState>,
}

#[derive(Debug)]
pub struct VoteLimited {
    retry_after: Duration,
}

impl VoteProtector {
    pub async fn check_and_record(
        &self,
        ip: IpAddr,
        instance: &str,
        config: &IpProtectionConfig,
    ) -> Result<(), VoteLimited> {
        self.check_and_record_at(ip, instance, config, Instant::now())
            .await
    }

    async fn check_and_record_at(
        &self,
        ip: IpAddr,
        instance: &str,
        config: &IpProtectionConfig,
        now: Instant,
    ) -> Result<(), VoteLimited> {
        let mut state = self.state.lock().await;
        cleanup_state(&mut state, config, now);

        if config.rate_limit.enabled {
            let votes = state.votes.entry(ip).or_default();
            if votes.len() >= config.rate_limit.max_votes {
                let retry_after = votes
                    .front()
                    .map(|timestamp| {
                        config
                            .rate_limit
                            .window
                            .saturating_sub(now.saturating_duration_since(*timestamp))
                    })
                    .unwrap_or_default();
                return Err(VoteLimited { retry_after });
            }
        }

        let unique_key = (ip, instance.to_owned());
        if config.one_vote_per_instance.enabled
            && let Some(timestamp) = state.instance_votes.get(&unique_key)
        {
            let retry_after = config
                .one_vote_per_instance
                .window
                .saturating_sub(now.saturating_duration_since(*timestamp));
            return Err(VoteLimited { retry_after });
        }

        if config.rate_limit.enabled {
            state.votes.entry(ip).or_default().push_back(now);
        }
        if config.one_vote_per_instance.enabled {
            state.instance_votes.insert(unique_key, now);
        }
        Ok(())
    }

    pub async fn cleanup(&self, config: &IpProtectionConfig) {
        self.cleanup_at(config, Instant::now()).await;
    }

    async fn cleanup_at(&self, config: &IpProtectionConfig, now: Instant) {
        let mut state = self.state.lock().await;
        cleanup_state(&mut state, config, now);
    }
}

fn cleanup_state(state: &mut ProtectionState, config: &IpProtectionConfig, now: Instant) {
    if config.rate_limit.enabled {
        for votes in state.votes.values_mut() {
            while votes.front().is_some_and(|timestamp| {
                now.saturating_duration_since(*timestamp) >= config.rate_limit.window
            }) {
                votes.pop_front();
            }
        }
        state.votes.retain(|_, votes| !votes.is_empty());
    } else {
        state.votes.clear();
    }
    if config.one_vote_per_instance.enabled {
        state.instance_votes.retain(|_, timestamp| {
            now.saturating_duration_since(*timestamp) < config.one_vote_per_instance.window
        });
    } else {
        state.instance_votes.clear();
    }
}

pub async fn cleanup_loop(
    protector: Arc<VoteProtector>,
    config: Arc<fastside_shared::config::AppConfig>,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        protector.cleanup(&config.reputation.ip_protection).await;
    }
}

#[derive(Debug, Deserialize)]
struct VoteQuery {
    instance: String,
    direction: String,
    #[serde(default = "default_return_to")]
    return_to: String,
}

fn default_return_to() -> String {
    "/".to_owned()
}

#[derive(Template)]
#[template(path = "vote.html")]
struct VoteTemplate<'a> {
    instance: &'a str,
    direction: &'a str,
    return_to: &'a str,
    csrf_token: &'a str,
    widget_html: &'a str,
}

struct OptionalConnectInfo(Option<SocketAddr>);

impl<S> FromRequestParts<S> for OptionalConnectInfo
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(_parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        #[cfg(feature = "native")]
        let address = _parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| *address);
        #[cfg(not(feature = "native"))]
        let address = None;
        Ok(Self(address))
    }
}

async fn vote_confirmation(
    State(state): State<AppState>,
    query: Result<Query<VoteQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, VoteError> {
    ensure_reputation_enabled(&state.config.reputation)?;
    let Query(query) =
        query.map_err(|_| VoteError::new(StatusCode::BAD_REQUEST, "invalid vote query"))?;
    let return_to =
        validate_vote_input(&state, &query.instance, &query.direction, &query.return_to).await?;
    let (csrf_token, csrf_cookie) = csrf_token(&headers).map_err(|_| {
        VoteError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "failed to create CSRF token",
        )
    })?;
    let template = VoteTemplate {
        instance: &query.instance,
        direction: &query.direction,
        return_to: &return_to,
        csrf_token: &csrf_token,
        widget_html: &state.config.reputation.captcha.widget_html,
    };
    let mut response = Html(
        template
            .render()
            .expect("failed to render reputation vote page"),
    )
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(&mut response, csrf_cookie);
    Ok(response)
}

#[axum::debug_handler]
async fn submit_vote(
    State(state): State<AppState>,
    OptionalConnectInfo(connect_info): OptionalConnectInfo,
    headers: HeaderMap,
    fields: Result<Form<HashMap<String, String>>, FormRejection>,
) -> Result<Response, VoteError> {
    ensure_reputation_enabled(&state.config.reputation)?;
    let Form(fields) =
        fields.map_err(|_| VoteError::new(StatusCode::BAD_REQUEST, "invalid vote form"))?;
    let instance = required_field(&fields, "instance")?;
    let direction = required_field(&fields, "direction")?;
    let return_to = required_field(&fields, "return_to")?;
    let return_to = validate_vote_input(&state, instance, direction, return_to).await?;
    validate_csrf(&headers, required_field(&fields, "csrf_token")?)?;

    let remote_ip = resolve_client_ip(
        connect_info.map(|address| address.ip()),
        &headers,
        &state.config.reputation.ip_protection,
    );
    if state.config.reputation.captcha.enabled {
        let token_field = &state.config.reputation.captcha.token_field;
        let token = fields
            .get(token_field)
            .filter(|value| !value.is_empty())
            .map(String::as_str)
            .ok_or_else(|| VoteError::new(StatusCode::FORBIDDEN, "CAPTCHA verification failed"))?;
        let accepted = state
            .captcha_verifier
            .verify(&state.config.reputation.captcha, token, remote_ip)
            .await
            .map_err(|_| VoteError::new(StatusCode::FORBIDDEN, "CAPTCHA verification failed"))?;
        if !accepted {
            return Err(VoteError::new(
                StatusCode::FORBIDDEN,
                "CAPTCHA verification failed",
            ));
        }
    }

    let ip_config = &state.config.reputation.ip_protection;
    if ip_config.rate_limit.enabled || ip_config.one_vote_per_instance.enabled {
        let ip = remote_ip.ok_or_else(|| {
            VoteError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "client IP address is unavailable",
            )
        })?;
        state
            .vote_protector
            .check_and_record(ip, instance, ip_config)
            .await
            .map_err(|limited| {
                VoteError::limited("vote limit reached", limited.retry_after.as_secs().max(1))
            })?;
    }

    state
        .state_store
        .apply_vote(instance, parse_direction(direction)?)
        .await
        .map_err(|_| VoteError::new(StatusCode::SERVICE_UNAVAILABLE, "failed to store vote"))?;
    Ok(Redirect::to(&return_to).into_response())
}

fn required_field<'a>(
    fields: &'a HashMap<String, String>,
    name: &str,
) -> Result<&'a str, VoteError> {
    fields
        .get(name)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| VoteError::new(StatusCode::BAD_REQUEST, format!("missing {name}")))
}

fn parse_direction(direction: &str) -> Result<VoteDirection, VoteError> {
    match direction {
        "up" => Ok(VoteDirection::Up),
        "down" => Ok(VoteDirection::Down),
        _ => Err(VoteError::new(
            StatusCode::BAD_REQUEST,
            "invalid vote direction",
        )),
    }
}

async fn validate_vote_input(
    state: &AppState,
    instance: &str,
    direction: &str,
    return_to: &str,
) -> Result<String, VoteError> {
    parse_direction(direction)?;
    let return_to = encode_return_to(return_to)
        .ok_or_else(|| VoteError::new(StatusCode::BAD_REQUEST, "invalid return path"))?;
    let loaded_data = state.loaded_data.read().await;
    let exists = loaded_data.services.values().any(|service| {
        service
            .instances
            .iter()
            .any(|known| known.url.as_str() == instance)
    });
    if !exists {
        return Err(VoteError::new(StatusCode::NOT_FOUND, "instance not found"));
    }
    Ok(return_to)
}

fn encode_return_to(value: &str) -> Option<String> {
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return None;
    }

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b':'
                    | b'/'
                    | b'?'
                    | b'#'
                    | b'['
                    | b']'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'%'
            )
        {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").ok()?;
        }
    }
    Some(encoded)
}

fn ensure_reputation_enabled(config: &ReputationConfig) -> Result<(), VoteError> {
    if config.enabled {
        Ok(())
    } else {
        Err(VoteError::new(
            StatusCode::NOT_FOUND,
            "instance reputation is disabled",
        ))
    }
}

pub(crate) fn csrf_token(
    headers: &HeaderMap,
) -> Result<(String, Option<Cookie<'static>>), CsrfTokenError> {
    if let Some(token) = cookie_value(headers, CSRF_COOKIE)
        && valid_csrf_token(&token)
    {
        return Ok((token, None));
    }
    let mut bytes = [0_u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|_| CsrfTokenError)?;
    let token = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");
    let cookie = Cookie::build((CSRF_COOKIE, token.clone()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .build();
    Ok((token, Some(cookie)))
}

fn valid_csrf_token(token: &str) -> bool {
    token.len() == 48 && token.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_csrf(headers: &HeaderMap, submitted: &str) -> Result<(), VoteError> {
    if valid_csrf_token(submitted)
        && cookie_value(headers, CSRF_COOKIE).as_deref() == Some(submitted)
    {
        Ok(())
    } else {
        Err(VoteError::new(StatusCode::FORBIDDEN, "invalid CSRF token"))
    }
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
}

pub fn append_cookie(response: &mut Response, cookie: Option<Cookie<'static>>) {
    if let Some(cookie) = cookie
        && let Ok(value) = HeaderValue::from_str(&cookie.to_string())
    {
        response.headers_mut().append(SET_COOKIE, value);
    }
}

pub fn last_instance_cookie(instance: Option<&str>) -> Cookie<'static> {
    let mut builder = Cookie::build((
        LAST_INSTANCE_COOKIE,
        instance
            .map(|instance| urlencoding::encode(instance).into_owned())
            .unwrap_or_default(),
    ))
    .path("/")
    .same_site(SameSite::Lax);
    if instance.is_none() {
        builder = builder.max_age(cookie::time::Duration::ZERO);
    }
    builder.build()
}

fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    config: &IpProtectionConfig,
) -> Option<IpAddr> {
    let peer = peer?;
    let Some(header_name) = &config.client_ip.header else {
        return Some(peer);
    };
    let trusted = config
        .client_ip
        .trusted_proxies
        .iter()
        .filter_map(|network| IpNet::from_str(network).ok())
        .any(|network| network.contains(&peer));
    if !trusted {
        return Some(peer);
    }
    headers
        .get(header_name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|value| value.trim().parse().ok())
        .or(Some(peer))
}

#[derive(Debug)]
struct VoteError {
    status: StatusCode,
    detail: String,
    retry_after: Option<u64>,
}

impl VoteError {
    fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
            retry_after: None,
        }
    }

    fn limited(detail: impl Into<String>, retry_after: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            detail: detail.into(),
            retry_after: Some(retry_after),
        }
    }
}

impl IntoResponse for VoteError {
    fn into_response(self) -> Response {
        let page = ErrorTemplate {
            detail: &self.detail,
            status_code: self.status,
        }
        .render()
        .expect("failed to render vote error page");
        let mut response = (self.status, Html(page)).into_response();
        if let Some(retry_after) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&retry_after.to_string())
        {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv4Addr, time::Duration};

    use axum::http::{HeaderMap, HeaderValue};
    use fastside_shared::config::{AppConfig, IpProtectionConfig};

    use super::*;

    #[tokio::test]
    async fn one_vote_rule_uses_the_configured_long_window() {
        let protector = VoteProtector::default();
        let mut config = IpProtectionConfig::default();
        config.one_vote_per_instance.enabled = true;
        config.one_vote_per_instance.window = Duration::from_secs(45 * 60);
        let start = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        protector
            .check_and_record_at(ip, "https://one.example/", &config, start)
            .await
            .unwrap();
        assert!(
            protector
                .check_and_record_at(
                    ip,
                    "https://one.example/",
                    &config,
                    start + Duration::from_secs(31 * 60),
                )
                .await
                .is_err()
        );
        protector
            .check_and_record_at(
                ip,
                "https://one.example/",
                &config,
                start + Duration::from_secs(46 * 60),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn general_limit_and_cleanup_use_a_controlled_clock() {
        let protector = VoteProtector::default();
        let mut config = IpProtectionConfig::default();
        config.rate_limit.enabled = true;
        config.rate_limit.max_votes = 2;
        config.rate_limit.window = Duration::from_secs(60);
        let start = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        for instance in ["https://one.example/", "https://two.example/"] {
            protector
                .check_and_record_at(ip, instance, &config, start)
                .await
                .unwrap();
        }
        assert!(
            protector
                .check_and_record_at(ip, "https://three.example/", &config, start)
                .await
                .is_err()
        );
        protector
            .cleanup_at(&config, start + Duration::from_secs(61))
            .await;
        assert!(protector.state.lock().await.votes.is_empty());
    }

    #[test]
    fn client_header_is_used_only_for_a_trusted_proxy() {
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let client = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 8));
        let mut headers = HeaderMap::new();
        headers.insert("x-client-ip", HeaderValue::from_static("198.51.100.8"));
        let mut config = IpProtectionConfig::default();
        config.client_ip.header = Some("x-client-ip".to_owned());

        assert_eq!(resolve_client_ip(Some(peer), &headers, &config), Some(peer));
        config
            .client_ip
            .trusted_proxies
            .push("10.0.0.0/8".to_owned());
        assert_eq!(
            resolve_client_ip(Some(peer), &headers, &config),
            Some(client)
        );
    }

    #[test]
    fn validation_accepts_windows_longer_than_thirty_minutes() {
        let mut config = AppConfig::default();
        config
            .reputation
            .ip_protection
            .one_vote_per_instance
            .enabled = true;
        config.reputation.ip_protection.one_vote_per_instance.window =
            Duration::from_secs(24 * 60 * 60);
        validate_config(&config).unwrap();
    }

    #[test]
    fn validation_rejects_zero_length_enabled_protections() {
        let mut config = AppConfig::default();
        config.reputation.captcha.enabled = true;
        config.reputation.captcha.widget_html = "<div></div>".to_owned();
        config.reputation.captcha.verify_url = Some("https://captcha.example/verify".to_owned());
        config.reputation.captcha.timeout = Duration::ZERO;
        assert!(validate_config(&config).is_err());

        config.reputation.captcha.enabled = false;
        config.reputation.ip_protection.rate_limit.enabled = true;
        config.reputation.ip_protection.rate_limit.window = Duration::ZERO;
        assert!(validate_config(&config).is_err());

        config.reputation.ip_protection.rate_limit.enabled = false;
        config
            .reputation
            .ip_protection
            .one_vote_per_instance
            .enabled = true;
        config.reputation.ip_protection.one_vote_per_instance.window = Duration::ZERO;
        assert!(validate_config(&config).is_err());
    }
}
