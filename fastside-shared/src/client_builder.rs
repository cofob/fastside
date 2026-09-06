use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex, RwLock},
    time::{Duration, Instant},
};

use reqwest::{
    Client,
    cookie::{CookieStore, Jar},
    header::HeaderValue,
};
use url::Url;

use crate::{
    config::{CrawlerConfig, ProxyData, select_proxy},
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service},
};

const CACHE_CAPACITY: usize = 256;
const CACHE_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// Cookie storage that can discard a rejected session before a fresh challenge.
#[derive(Default)]
struct SessionCookies(RwLock<Jar>);

impl CookieStore for SessionCookies {
    fn set_cookies(&self, cookies: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        self.0.read().unwrap().set_cookies(cookies, url);
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        self.0.read().unwrap().cookies(url)
    }
}

struct Session {
    client: Client,
    cookies: Arc<SessionCookies>,
    // Serialize probes so a concurrent challenge cannot replace verification cookies.
    authenticated: tokio::sync::Mutex<bool>,
}

/// A cached HTTP connection pool and Anubis session for one origin and route.
#[derive(Clone)]
pub struct ProbeClient(Arc<Session>);

impl ProbeClient {
    /// Fetch headers for tag detection without racing a challenge exchange.
    pub async fn inspect(&self, url: Url) -> Result<(reqwest::Response, bool), reqwest::Error> {
        let authenticated = self.0.authenticated.lock().await;
        let response = self.0.client.get(url).send().await?;
        Ok((response, *authenticated))
    }

    pub async fn probe(&self, url: Url) -> anyhow::Result<crate::anubis::ProbeResponse> {
        let mut authenticated = self.0.authenticated.lock().await;
        let response = self.0.client.get(url.clone()).send().await?;
        let result = crate::anubis::resolve(&self.0.client, response).await;
        let retry = *authenticated
            && match &result {
                Ok(response) => matches!(response.status.as_u16(), 401 | 403) && !response.solved,
                // A transport failure gives no evidence that the saved session is invalid.
                Err(error) => error.downcast_ref::<reqwest::Error>().is_none(),
            };
        let mut response = if retry {
            *authenticated = false;
            *self.0.cookies.0.write().unwrap() = Jar::default();
            let response = self.0.client.get(url).send().await?;
            crate::anubis::resolve(&self.0.client, response).await?
        } else {
            result?
        };
        response.reused = *authenticated && !response.solved;
        *authenticated |= response.solved;
        Ok(response)
    }
}

// Keep credentials out of Debug output. A changed route must use a new session.
#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    origin: String,
    proxy: Option<(String, Option<(String, String)>)>,
    timeout: Duration,
    follow_redirects: bool,
}

#[derive(Default)]
struct SessionCache(VecDeque<(CacheKey, Instant, ProbeClient)>);

impl SessionCache {
    fn get(&mut self, key: &CacheKey, now: Instant) -> Option<ProbeClient> {
        self.0
            .retain(|(_, last_used, _)| now.duration_since(*last_used) < CACHE_IDLE_TTL);
        let index = self
            .0
            .iter()
            .position(|(candidate, _, _)| candidate == key)?;
        let (key, _, client) = self.0.remove(index)?;
        self.0.push_back((key, now, client.clone()));
        Some(client)
    }

    fn insert(&mut self, key: CacheKey, now: Instant, client: ProbeClient) {
        if self.0.len() >= CACHE_CAPACITY {
            // A full catalogue pass must not evict solved sessions in favor of
            // ordinary sites. Prefer the oldest idle, unauthenticated client.
            let index = self
                .0
                .iter()
                .position(|(_, _, client)| {
                    client
                        .0
                        .authenticated
                        .try_lock()
                        .is_ok_and(|authenticated| !*authenticated)
                })
                .unwrap_or(0);
            self.0.remove(index);
        }
        self.0.push_back((key, now, client));
    }
}

fn default_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in REQUEST_HEADERS {
        headers.insert(
            reqwest::header::HeaderName::from_static(name),
            reqwest::header::HeaderValue::from_static(value),
        );
    }
    headers
}

pub fn build_client(
    service: &Service,
    config: &CrawlerConfig,
    proxies: &ProxyData,
    instance: &Instance,
) -> Result<ProbeClient, reqwest::Error> {
    let timeout =
        config.get_domain_timeout(instance.url.host_str().expect("Instance URL has no host"));
    let proxy_config = select_proxy(proxies, &instance.tags);
    let key = CacheKey {
        origin: instance.url.origin().ascii_serialization(),
        proxy: proxy_config.map(|p| {
            (
                p.url.clone(),
                p.auth
                    .as_ref()
                    .map(|a| (a.username.clone(), a.password.clone())),
            )
        }),
        timeout,
        follow_redirects: service.follow_redirects,
    };
    static CACHE: LazyLock<Mutex<SessionCache>> =
        LazyLock::new(|| Mutex::new(SessionCache::default()));
    let mut cache = CACHE.lock().unwrap();
    let now = Instant::now();
    if let Some(client) = cache.get(&key, now) {
        return Ok(client);
    }

    let redirect_policy = if service.follow_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    let cookies = Arc::new(SessionCookies::default());
    let mut client_builder = Client::builder()
        .cookie_provider(cookies.clone())
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .default_headers(default_headers())
        .redirect(redirect_policy);
    if let Some(proxy_config) = proxy_config {
        let mut proxy = reqwest::Proxy::all(&proxy_config.url)?;
        if let Some(auth) = &proxy_config.auth {
            proxy = proxy.basic_auth(&auth.username, &auth.password);
        }
        client_builder = client_builder.proxy(proxy);
    }
    let client = ProbeClient(Arc::new(Session {
        client: client_builder.build()?,
        cookies,
        authenticated: tokio::sync::Mutex::new(false),
    }));
    cache.insert(key, now, client.clone());
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn settings(url: &str) -> (Service, CrawlerConfig, Instance) {
        (
            serde_json::from_value(serde_json::json!({"type":"test", "instances":[]})).unwrap(),
            CrawlerConfig {
                request_timeout: Duration::from_secs(3),
                ..Default::default()
            },
            Instance::from(Url::parse(url).unwrap()),
        )
    }

    fn challenge(id: &str) -> String {
        format!(
            r#"<script id="anubis_challenge">{{"rules":{{"algorithm":"fast","difficulty":0}},"challenge":{{"id":"{id}","randomData":"test"}}}}</script>"#
        )
    }

    // The callback checks each request. The listener has a deadline to avoid hung tests.
    fn server(
        steps: usize,
        mut handle: impl FnMut(usize, &str) -> (u16, String, String) + Send + 'static,
    ) -> (Url, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let task = std::thread::spawn(move || {
            for step in 0..steps {
                let start = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                start.elapsed() < Duration::from_secs(5),
                                "missing request {step}"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8(bytes).unwrap();
                let (code, headers, body) = handle(step, &request);
                write!(stream, "HTTP/1.1 {code} Test\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (url, task)
    }

    fn issue(id: &str) -> (u16, String, String) {
        (
            200,
            format!("Set-Cookie: verification={id}; Path=/\r\n"),
            challenge(id),
        )
    }

    fn pass(request: &str, id: &str) -> (u16, String, String) {
        assert!(request.starts_with("GET /.within.website/x/cmd/anubis/api/pass-challenge?"));
        assert!(request.contains(&format!("id={id}")));
        assert!(request.contains(&format!("verification={id}")));
        (
            302,
            format!("Location: /\r\nSet-Cookie: auth={id}; Path=/\r\n"),
            String::new(),
        )
    }

    fn page(request: &str, id: &str) -> (u16, String, String) {
        assert!(request.contains(&format!("auth={id}")));
        (200, String::new(), "service".into())
    }

    #[tokio::test]
    async fn reuses_session_and_refreshes_rejected_cookie_once() {
        let (url, server) = server(9, |step, request| match step {
            0 => issue("one"),
            1 => pass(request, "one"),
            2 | 3 => page(request, "one"),
            4 => {
                assert!(request.contains("auth=one"));
                (403, String::new(), "expired".into())
            }
            5 => {
                assert!(!request.contains("auth="));
                issue("two")
            }
            6 => pass(request, "two"),
            7 | 8 => page(request, "two"),
            _ => unreachable!(),
        });
        let (service, config, instance) = settings(url.as_str());
        let proxies = ProxyData::new();
        for (attempt, (solved, reused)) in
            [(true, false), (false, true), (true, false), (false, true)]
                .into_iter()
                .enumerate()
        {
            // Rebuild between attempts, as the actual crawler does.
            let client = build_client(&service, &config, &proxies, &instance).unwrap();
            let response = client.probe(url.clone()).await.unwrap();
            assert_eq!(
                (response.solved, response.reused),
                (solved, reused),
                "attempt {attempt}"
            );
            assert_eq!(response.body, "service");
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn solves_new_challenge_when_cookie_expires() {
        let (url, server) = server(6, |step, request| match step {
            0 => issue("one"),
            1 => pass(request, "one"),
            2 => page(request, "one"),
            3 => {
                assert!(request.contains("auth=one"));
                issue("two")
            }
            4 => pass(request, "two"),
            5 => page(request, "two"),
            _ => unreachable!(),
        });
        let (service, config, instance) = settings(url.as_str());
        let client = build_client(&service, &config, &ProxyData::new(), &instance).unwrap();
        for _ in 0..2 {
            assert!(client.probe(url.clone()).await.unwrap().solved);
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn concurrent_probes_share_one_solution() {
        let (url, server) = server(4, |step, request| match step {
            0 => issue("one"),
            1 => pass(request, "one"),
            2 | 3 => page(request, "one"),
            _ => unreachable!(),
        });
        let (service, config, instance) = settings(url.as_str());
        let client = build_client(&service, &config, &ProxyData::new(), &instance).unwrap();
        let (first, second) = tokio::join!(client.probe(url.clone()), client.probe(url));
        assert!(first.unwrap().solved);
        assert!(second.unwrap().reused);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn retry_is_bounded_and_backend_errors_keep_session() {
        let (url, server) = server(7, |step, request| match step {
            0 => issue("one"),
            1 => pass(request, "one"),
            2 => page(request, "one"),
            3 => (503, String::new(), "service unavailable".into()),
            4 => {
                assert!(request.contains("auth=one"));
                (403, String::new(), "expired".into())
            }
            5 => {
                assert!(!request.contains("auth="));
                issue("two")
            }
            6 => (403, String::new(), "proof rejected".into()),
            _ => unreachable!(),
        });
        let (service, config, instance) = settings(url.as_str());
        let client = build_client(&service, &config, &ProxyData::new(), &instance).unwrap();
        assert!(client.probe(url.clone()).await.unwrap().solved);
        let failed = client.probe(url.clone()).await.unwrap();
        assert_eq!(failed.status.as_u16(), 503);
        assert!(failed.reused);
        assert!(client.probe(url).await.is_err());
        server.join().unwrap();
    }

    #[test]
    fn ordinary_catalogue_entries_do_not_evict_solved_sessions() {
        let (service, config, instance) = settings("http://retained-session.invalid/");
        let saved = build_client(&service, &config, &ProxyData::new(), &instance).unwrap();
        *saved.0.authenticated.try_lock().unwrap() = true;
        let (_, _, instance) = settings("http://ordinary-session.invalid/");
        let ordinary = build_client(&service, &config, &ProxyData::new(), &instance).unwrap();
        let now = Instant::now();
        let mut cache = SessionCache::default();
        let key = CacheKey {
            origin: "saved".into(),
            proxy: None,
            timeout: Duration::ZERO,
            follow_redirects: false,
        };
        cache.insert(key.clone(), now, saved.clone());
        for index in 0..CACHE_CAPACITY * 2 {
            let mut ordinary_key = key.clone();
            ordinary_key.origin = index.to_string();
            cache.insert(ordinary_key, now, ordinary.clone());
        }
        assert!(Arc::ptr_eq(&cache.get(&key, now).unwrap().0, &saved.0));
        assert_eq!(cache.0.len(), CACHE_CAPACITY);
    }

    #[test]
    fn cache_separates_origins_routes_and_settings_and_expires_entries() {
        let (mut service, mut config, mut instance) = settings("http://cache-test.invalid:8123/a");
        let mut proxies = ProxyData::new();
        let first = build_client(&service, &config, &proxies, &instance).unwrap();
        instance.url.set_path("/b");
        assert!(Arc::ptr_eq(
            &first.0,
            &build_client(&service, &config, &proxies, &instance)
                .unwrap()
                .0
        ));
        instance.url.set_port(Some(8124)).unwrap();
        assert!(!Arc::ptr_eq(
            &first.0,
            &build_client(&service, &config, &proxies, &instance)
                .unwrap()
                .0
        ));
        instance.url.set_port(Some(8123)).unwrap();
        service.follow_redirects = true;
        assert!(!Arc::ptr_eq(
            &first.0,
            &build_client(&service, &config, &proxies, &instance)
                .unwrap()
                .0
        ));
        service.follow_redirects = false;
        config.request_timeout += Duration::from_secs(1);
        assert!(!Arc::ptr_eq(
            &first.0,
            &build_client(&service, &config, &proxies, &instance)
                .unwrap()
                .0
        ));
        instance.tags.push("proxy".into());
        proxies.insert(
            "proxy".into(),
            crate::config::Proxy {
                url: "http://127.0.0.1:9999".into(),
                auth: None,
            },
        );
        let proxied = build_client(&service, &config, &proxies, &instance).unwrap();
        assert!(!Arc::ptr_eq(&first.0, &proxied.0));
        proxies.get_mut("proxy").unwrap().auth = Some(crate::config::ProxyAuth {
            username: "test".into(),
            password: "test".into(),
        });
        assert!(!Arc::ptr_eq(
            &proxied.0,
            &build_client(&service, &config, &proxies, &instance)
                .unwrap()
                .0
        ));

        let mut cache = SessionCache::default();
        let now = Instant::now();
        let key = CacheKey {
            origin: "first".into(),
            proxy: None,
            timeout: Duration::ZERO,
            follow_redirects: false,
        };
        for index in 0..=CACHE_CAPACITY {
            let mut entry = key.clone();
            entry.origin = index.to_string();
            cache.insert(entry, now, first.clone());
        }
        assert_eq!(cache.0.len(), CACHE_CAPACITY);
        assert_eq!(cache.0.front().unwrap().0.origin, "1");
        let oldest = cache.0.front().unwrap().0.clone();
        assert!(cache.get(&oldest, now + CACHE_IDLE_TTL).is_none());
        assert!(cache.0.is_empty());
    }
}
