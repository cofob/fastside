use reqwest::Client;

use crate::{
    config::{CrawlerConfig, ProxyData},
    request_headers::REQUEST_HEADERS,
    serde_types::{Instance, Service},
};

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
