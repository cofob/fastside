use reqwest::Client;

use crate::{
    config::{CrawlerConfig, ProxyData},
    serde_types::{Instance, Service},
};

fn default_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(
            "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
        ),
    );
    headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/png,image/svg+xml,*/*;q=0.8s"));
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        reqwest::header::HeaderValue::from_static("en-US,en;q=0.5"),
    );
    headers.insert(
        "X-Is-Fastside",
        reqwest::header::HeaderValue::from_static("true"),
    );
    headers
}

pub fn build_client(
    service: &Service,
    config: &CrawlerConfig,
    proxies: &ProxyData,
    instance: &Instance,
) -> Result<Client, reqwest::Error> {
    let redirect_policy = if service.follow_redirects {
        reqwest::redirect::Policy::default()
    } else {
        reqwest::redirect::Policy::none()
    };
    let timeout = config.get_domain_timeout(
        instance
            .url
            .host_str()
            .expect("Failed to get host from instance URL"),
    );
    let mut client_builder = Client::builder()
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .default_headers(default_headers())
        .redirect(redirect_policy);

    let proxy_name: Option<String> = {
        let mut val: Option<String> = None;
        for proxy in proxies.keys() {
            if instance.tags.contains(proxy) {
                val = Some(proxy.clone());
                break;
            }
        }
        val
    };
    if let Some(proxy_name) = proxy_name {
        let proxy_config = proxies.get(&proxy_name).unwrap();
        let proxy = {
            let mut builder = reqwest::Proxy::all(&proxy_config.url)?;
            if let Some(auth) = &proxy_config.auth {
                builder = builder.basic_auth(&auth.username, &auth.password);
            }
            builder
        };
        client_builder = client_builder.proxy(proxy);
    }

    client_builder.build()
}
