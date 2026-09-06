use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use fastside::{
    app,
    captcha::NoopCaptchaVerifier,
    crawler::{Crawler, CrawlerError, InstanceClient, InstanceRequest},
    reputation::VoteProtector,
    storage::{
        CrawlSnapshot, InstanceReputation, MemoryStateStore, StateStore, StorageError,
        VoteDirection,
    },
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{AppConfig, CrawlerConfig, ProxyData, UserConfig},
    serde_types::{AllowedHttpCodes, Instance, Service},
};
use tokio::sync::RwLock;
use tower_service::Service as _;
use url::Url;

const CSRF_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug)]
struct UnusedClient;

#[async_trait]
impl InstanceClient for UnusedClient {
    async fn request(
        &self,
        _config: &CrawlerConfig,
        _proxies: &ProxyData,
        _service: &Service,
        _instance: &Instance,
        _test_url: Url,
    ) -> Result<InstanceRequest, CrawlerError> {
        unreachable!("initialized test data does not crawl")
    }
}

#[derive(Debug)]
struct FailingStore;

#[async_trait]
impl StateStore for FailingStore {
    async fn load_crawl_snapshot(&self) -> Result<Option<CrawlSnapshot>, StorageError> {
        Err(StorageError("test storage failure".to_owned()))
    }

    async fn save_crawl_snapshot(&self, _snapshot: &CrawlSnapshot) -> Result<(), StorageError> {
        Err(StorageError("test storage failure".to_owned()))
    }

    async fn get_reputations(
        &self,
        _instances: &[String],
    ) -> Result<HashMap<String, InstanceReputation>, StorageError> {
        Err(StorageError("test storage failure".to_owned()))
    }

    async fn apply_vote(
        &self,
        _instance: &str,
        _direction: VoteDirection,
    ) -> Result<InstanceReputation, StorageError> {
        Err(StorageError("test storage failure".to_owned()))
    }
}

async fn state() -> AppState {
    let user_config = UserConfig {
        required_tags: vec!["clearnet".into(), "https".into(), "ipv4".into()],
        ..UserConfig::default()
    };
    let service = Service {
        name: "demo".into(),
        test_url: "/".into(),
        fallback: Some(Url::parse("https://fallback.example/").unwrap()),
        follow_redirects: false,
        allowed_http_codes: AllowedHttpCodes {
            codes: vec![200],
            inclusive_ranges: Vec::new(),
            exclusive_ranges: Vec::new(),
        },
        search_string: None,
        regexes: Vec::new(),
        aliases: vec!["alias".into()],
        source_link: None,
        deprecated_message: None,
        instances: vec![Instance {
            url: Url::parse("https://demo.example/").unwrap(),
            tags: user_config.required_tags.clone(),
        }],
    };
    let services = HashMap::from([(service.name.clone(), service)]);
    let loaded_data = Arc::new(RwLock::new(LoadedData {
        services,
        proxies: ProxyData::new(),
        default_user_config: user_config.clone(),
    }));
    let config = AppConfig {
        crawler: CrawlerConfig {
            ping_interval: Duration::from_secs(300),
            ..CrawlerConfig::default()
        },
        default_user_config: user_config,
        ..AppConfig::default()
    };
    let config = Arc::new(config);
    let crawler = Arc::new(Crawler::new(
        loaded_data.clone(),
        config.crawler.clone(),
        Arc::new(UnusedClient),
    ));
    crawler.initialize_with_defaults().await;
    let regexes = Arc::new(compile_regexes(&loaded_data.read().await.services));

    AppState {
        config,
        crawler,
        loaded_data,
        regexes,
        state_store: Arc::new(MemoryStateStore::default()),
        captcha_verifier: Arc::new(NoopCaptchaVerifier),
        vote_protector: Arc::new(VoteProtector::default()),
    }
}

async fn call(state: AppState, request: Request<Body>) -> Response {
    app(state).call(request).await.unwrap()
}

async fn reputation_state() -> (AppState, Arc<MemoryStateStore>) {
    let mut state = state().await;
    let mut config = (*state.config).clone();
    config.reputation.enabled = true;
    state.config = Arc::new(config);
    let store = Arc::new(MemoryStateStore::default());
    state.state_store = store.clone();
    (state, store)
}

fn vote_body(instance: &str, direction: &str, return_to: &str, csrf: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("instance", instance)
        .append_pair("direction", direction)
        .append_pair("return_to", return_to)
        .append_pair("csrf_token", csrf)
        .finish()
}

#[tokio::test]
async fn redirect_keeps_method_rules_and_raw_query() {
    for method in [Method::GET, Method::POST] {
        let request = Request::builder()
            .method(method)
            .uri("/demo/path?a=1&a=2")
            .body(Body::empty())
            .unwrap();
        let response = call(state().await, request).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://demo.example/path?a=1&a=2"
        );
    }
}

#[tokio::test]
async fn cached_and_history_routes_keep_headers() {
    let request = Request::builder()
        .uri("/@cached/demo/path")
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=300, stale-while-revalidate=86400, stale-if-error=86400, immutable"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("https://demo.example/"));

    let request = Request::builder()
        .uri("/_/demo/path?a=1&a=2")
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("/demo/path"));
    assert!(body.contains("a=1"));
    assert!(body.contains("a=2"));

    for uri in ["/@cached/demo/", "/_/"] {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        assert_eq!(call(state().await, request).await.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn configuration_cookie_round_trips() {
    let config = UserConfig {
        required_tags: vec!["tor".into()],
        ..UserConfig::default()
    };
    let encoded = config.to_config_string().unwrap();
    let request = Request::builder()
        .uri(format!("/configure/save?{encoded}"))
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/configure?success");
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(&format!("config={encoded};")));

    let request = Request::builder()
        .uri("/configure")
        .header(header::COOKIE, format!("config={encoded}"))
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains(&encoded));
}

#[tokio::test]
async fn api_keeps_json_contract() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/redirect")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"url":"alias/path","config":{"required_tags":["clearnet","https","ipv4"]}}"#,
        ))
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        String::from_utf8(body.to_vec()).unwrap(),
        r#"{"url":"https://demo.example/path","is_fallback":false}"#
    );

    let invalid_requests = [
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/redirect")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/make_user_config_string")
            .body(Body::from("{}"))
            .unwrap(),
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/parse_user_config_string")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("123"))
            .unwrap(),
    ];
    for request in invalid_requests {
        assert_eq!(
            call(state().await, request).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn fallback_and_error_contracts_are_preserved() {
    let config = UserConfig {
        required_tags: vec!["tor".into()],
        ..UserConfig::default()
    }
    .to_config_string()
    .unwrap();

    let request = Request::builder()
        .uri("/demo/path")
        .header(header::COOKIE, format!("config={config}"))
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["refresh"],
        "15; url=https://fallback.example/path"
    );

    let request = Request::builder()
        .method(Method::POST)
        .uri("/demo/path")
        .header(header::COOKIE, format!("config={config}"))
        .body(Body::empty())
        .unwrap();
    let response = call(state().await, request).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://fallback.example/path"
    );

    let request = Request::builder()
        .uri("/missing/path")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        call(state().await, request).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn instance_list_hides_or_shows_reputation() {
    let response = call(
        state().await,
        Request::builder().uri("/").body(Body::empty()).unwrap(),
    )
    .await;
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("Reputation:"));

    let (state, store) = reputation_state().await;
    store
        .apply_vote("https://demo.example/", VoteDirection::Up)
        .await
        .unwrap();
    let response = call(
        state,
        Request::builder().uri("/").body(Body::empty()).unwrap(),
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("Reputation: <code>+1</code> <code>-0</code>"));
    assert!(body.contains("aria-label=\"Upvote https://demo.example/\""));
}

#[tokio::test]
async fn direct_vote_uses_csrf_and_returns_see_other() {
    let (state, store) = reputation_state().await;
    let index = call(
        state.clone(),
        Request::builder().uri("/").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(index.headers()[header::CACHE_CONTROL], "no-store");
    let set_cookie = index.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = set_cookie.split(';').next().unwrap();
    let csrf = cookie.strip_prefix("fastside_csrf=").unwrap();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/#demo",
            csrf,
        )))
        .unwrap();
    let response = call(state, request).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/#demo");
    let reputation = store
        .get_reputations(&["https://demo.example/".to_owned()])
        .await
        .unwrap();
    assert_eq!(reputation["https://demo.example/"].upvotes, 1);
}

#[tokio::test]
async fn vote_encodes_a_non_ascii_return_path() {
    let (state, store) = reputation_state().await;
    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("fastside_csrf={CSRF_TOKEN}"))
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/café",
            CSRF_TOKEN,
        )))
        .unwrap();
    let response = call(state, request).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/caf%C3%A9");
    let reputation = store
        .get_reputations(&["https://demo.example/".to_owned()])
        .await
        .unwrap();
    assert_eq!(reputation["https://demo.example/"].upvotes, 1);
}

#[tokio::test]
async fn vote_validation_returns_documented_errors() {
    let (reputation_state, _) = reputation_state().await;
    let cases = [
        (
            vote_body("https://demo.example/", "sideways", "/", CSRF_TOKEN),
            CSRF_TOKEN,
            StatusCode::BAD_REQUEST,
        ),
        (
            vote_body("https://unknown.example/", "up", "/", CSRF_TOKEN),
            CSRF_TOKEN,
            StatusCode::NOT_FOUND,
        ),
        (
            vote_body("https://demo.example/", "up", "//other.example", CSRF_TOKEN),
            CSRF_TOKEN,
            StatusCode::BAD_REQUEST,
        ),
        (
            vote_body(
                "https://demo.example/",
                "up",
                "/\\other.example",
                CSRF_TOKEN,
            ),
            CSRF_TOKEN,
            StatusCode::BAD_REQUEST,
        ),
        (
            vote_body("https://demo.example/", "up", "/", "wrong"),
            CSRF_TOKEN,
            StatusCode::FORBIDDEN,
        ),
        (
            vote_body("https://demo.example/", "up", "/", "short"),
            "short",
            StatusCode::FORBIDDEN,
        ),
    ];
    for (body, cookie, status) in cases {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/reputation/vote")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, format!("fastside_csrf={cookie}"))
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            call(reputation_state.clone(), request).await.status(),
            status
        );
    }

    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/",
            CSRF_TOKEN,
        )))
        .unwrap();
    assert_eq!(
        call(reputation_state.clone(), request).await.status(),
        StatusCode::BAD_REQUEST
    );

    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("fastside_csrf={CSRF_TOKEN}"))
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/",
            CSRF_TOKEN,
        )))
        .unwrap();
    assert_eq!(
        call(state().await, request).await.status(),
        StatusCode::NOT_FOUND
    );

    let request = Request::builder()
        .uri("/reputation/vote")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        call(state().await, request).await.status(),
        StatusCode::NOT_FOUND
    );

    let request = Request::builder()
        .uri("/reputation/vote")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        call(reputation_state, request).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn vote_returns_service_unavailable_for_storage_failure() {
    let (mut state, _) = reputation_state().await;
    state.state_store = Arc::new(FailingStore);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("fastside_csrf={CSRF_TOKEN}"))
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/",
            CSRF_TOKEN,
        )))
        .unwrap();
    assert_eq!(
        call(state, request).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn captcha_confirmation_contains_one_trusted_widget() {
    let (mut state, _) = reputation_state().await;
    let mut config = (*state.config).clone();
    config.reputation.captcha.enabled = true;
    config.reputation.captcha.widget_html = "<div class=\"captcha-widget\"></div>".to_owned();
    state.config = Arc::new(config);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("instance", "https://demo.example/")
        .append_pair("direction", "up")
        .append_pair("return_to", "/")
        .finish();
    let request = Request::builder()
        .uri(format!("/reputation/vote?{query}"))
        .body(Body::empty())
        .unwrap();
    let response = call(state, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert_eq!(body.matches("captcha-widget").count(), 1);
}

#[tokio::test]
async fn missing_captcha_token_fails_with_forbidden() {
    let (mut state, _) = reputation_state().await;
    let mut config = (*state.config).clone();
    config.reputation.captcha.enabled = true;
    config.reputation.captcha.widget_html = "<div class=\"captcha-widget\"></div>".to_owned();
    state.config = Arc::new(config);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/reputation/vote")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("fastside_csrf={CSRF_TOKEN}"))
        .body(Body::from(vote_body(
            "https://demo.example/",
            "up",
            "/",
            CSRF_TOKEN,
        )))
        .unwrap();

    assert_eq!(call(state, request).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rate_limit_returns_retry_after() {
    let (mut state, _) = reputation_state().await;
    let mut config = (*state.config).clone();
    config.reputation.ip_protection.rate_limit.enabled = true;
    config.reputation.ip_protection.rate_limit.max_votes = 1;
    state.config = Arc::new(config);
    let address: SocketAddr = "127.0.0.1:12345".parse().unwrap();

    for expected in [StatusCode::SEE_OTHER, StatusCode::TOO_MANY_REQUESTS] {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/reputation/vote")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, format!("fastside_csrf={CSRF_TOKEN}"))
            .body(Body::from(vote_body(
                "https://demo.example/",
                "up",
                "/",
                CSRF_TOKEN,
            )))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(address));
        let response = call(state.clone(), request).await;
        assert_eq!(response.status(), expected);
        if expected == StatusCode::TOO_MANY_REQUESTS {
            assert!(response.headers().contains_key(header::RETRY_AFTER));
        }
    }
}

#[tokio::test]
async fn redirects_manage_last_instance_cookie_and_history_script() {
    let response = call(
        state().await,
        Request::builder()
            .uri("/demo/path")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(!response.headers().contains_key(header::SET_COOKIE));

    let response = call(
        state().await,
        Request::builder()
            .uri("/_/demo/path")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("fastside_last_instance"));

    let (state, _) = reputation_state().await;
    let response = call(
        state,
        Request::builder()
            .uri("/demo/path")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("fastside_last_instance=https%3A%2F%2Fdemo.example%2F")
    );

    let config = UserConfig {
        required_tags: vec!["tor".to_owned()],
        ..UserConfig::default()
    }
    .to_config_string()
    .unwrap();
    let response = call(
        reputation_state().await.0,
        Request::builder()
            .uri("/demo/path")
            .header(header::COOKIE, format!("config={config}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("fastside_last_instance=;"));
    assert!(cookie.contains("Max-Age=0"));

    let (state, _) = reputation_state().await;
    let response = call(
        state,
        Request::builder()
            .uri("/_/demo/path")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("pageshow"));
    assert!(body.contains("fastside_last_instance"));
    assert!(body.contains("last-instance-vote"));

    for path in ["/_/%5C%5Cother.example", "/_/%2F%2Fother.example"] {
        let response = call(
            reputation_state().await.0,
            Request::builder().uri(path).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn weighted_cached_redirect_has_short_mutable_cache() {
    let (state, store) = reputation_state().await;
    store
        .apply_vote("https://demo.example/", VoteDirection::Up)
        .await
        .unwrap();
    let config = UserConfig {
        select_method: fastside_shared::config::SelectMethod::Weighted,
        required_tags: vec!["clearnet".into(), "https".into(), "ipv4".into()],
        ..UserConfig::default()
    }
    .to_config_string()
    .unwrap();
    let response = call(
        state,
        Request::builder()
            .uri("/@cached/demo/path")
            .header(header::COOKIE, format!("config={config}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=60, stale-while-revalidate=86400, stale-if-error=86400"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("\"weight\":2.0"));
}
