use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use fastside::{
    app,
    crawler::{Crawler, CrawlerError, InstanceClient, InstanceRequest},
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{AppConfig, CrawlerConfig, ProxyData, UserConfig},
    serde_types::{AllowedHttpCodes, Instance, Service},
};
use tokio::sync::RwLock;
use tower_service::Service as _;
use url::Url;

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
    }
}

async fn call(state: AppState, request: Request<Body>) -> Response {
    app(state).call(request).await.unwrap()
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
    assert_eq!(response.headers()["refresh"], "1; url=/demo/path?a=1&a=2");

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
