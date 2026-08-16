use anyhow::Result;
use async_std_resolver::{
    config,
    proto::rr::{RData, RecordType},
    resolver,
};
use ipnet::Ipv6Net;
use reqwest::{
    Client,
    header::{HeaderMap, SERVER, SET_COOKIE},
};
use url::Url;

const AUTO_TAGS: [&str; 12] = [
    "ipv4",
    "ipv6",
    "https",
    "http",
    "tor",
    "i2p",
    "ygg",
    "alfis",
    "cloudflare",
    "anubis",
    "antibot",
    "clearnet",
];
const HIDDEN_DOMAINS: [&str; 2] = [".onion", ".i2p"];

fn remove_auto_tags(tags: &mut Vec<String>) {
    tags.retain(|tag| !AUTO_TAGS.contains(&tag.as_str()));
}

fn get_response_tags(headers: &HeaderMap, is_hidden: bool) -> Result<Vec<String>> {
    let mut tags = Vec::new();

    if let Some(header) = headers.get(SERVER) {
        let header_str = header.to_str()?;
        if !is_hidden && header_str.contains("cloudflare") {
            tags.push("cloudflare".to_string());
        }
    }

    let is_anubis = headers.get_all(SET_COOKIE).iter().any(|header| {
        header
            .to_str()
            .ok()
            .and_then(|value| value.split_once('='))
            .is_some_and(|(name, _)| name.contains("-cookie-verification-"))
    });
    if is_anubis {
        tags.push("anubis".to_string());
    }

    if !tags.is_empty() {
        tags.push("antibot".to_string());
    }

    Ok(tags)
}

async fn get_network_tags(client: Client, url: Url) -> Result<Vec<String>> {
    let is_hidden = if let Some(domain) = url.domain() {
        HIDDEN_DOMAINS.iter().any(|d| domain.ends_with(d))
    } else {
        false
    };

    let response = client.get(url).send().await?;
    get_response_tags(response.headers(), is_hidden)
}

fn is_ygg(ip: &std::net::Ipv6Addr) -> bool {
    let ygg_net: Ipv6Net = "200::/7".parse().unwrap();
    ygg_net.contains(ip)
}

async fn get_dns_tags(url: Url) -> Result<Vec<String>> {
    let domain = match url.domain() {
        Some(domain) => domain,
        None => return Ok(Vec::new()),
    };
    if HIDDEN_DOMAINS.iter().any(|d| domain.ends_with(d)) {
        debug!("Skipping hidden domain DNS: {}", domain);
        return Ok(Vec::new());
    }

    let mut tags = Vec::new();

    let resolver = resolver(
        config::ResolverConfig::default(),
        config::ResolverOpts::default(),
    )
    .await;

    // This shit is ugly as shit
    // Simplest method to support CNAMEs with depth 1
    let mut lookup_domain = domain.to_string();
    let mut records: Vec<RData> = Vec::new();
    match resolver.lookup(lookup_domain.clone(), RecordType::A).await {
        Ok(l) => records.extend(l.iter().cloned()),
        Err(e) => {
            debug!("Failed to lookup A record for {}: {}", lookup_domain, e);
        }
    };
    // Find CNAME records in response
    for rdata in records.iter() {
        if let RData::CNAME(_) = rdata {
            lookup_domain = rdata.to_string();
            // Resolve A again
            records.clear();
            match resolver.lookup(lookup_domain.clone(), RecordType::A).await {
                Ok(l) => records.extend(l.iter().cloned()),
                Err(e) => {
                    debug!("Failed to lookup A record for {}: {}", lookup_domain, e);
                }
            };
            break;
        }
    }
    match resolver
        .lookup(lookup_domain.clone(), RecordType::AAAA)
        .await
    {
        Ok(l) => records.extend(l.iter().cloned()),
        Err(e) => {
            debug!("Failed to lookup AAAA record for {}: {}", lookup_domain, e);
        }
    };
    for rdata in records.iter() {
        let ip = match rdata.ip_addr() {
            Some(ip) => ip,
            None => {
                debug!("Get something other than IP: {:?}", rdata);
                continue;
            }
        };
        match ip {
            std::net::IpAddr::V4(_) => tags.push("ipv4".to_string()),
            std::net::IpAddr::V6(ip) => {
                tags.push("ipv6".to_string());
                if is_ygg(&ip) {
                    tags.push("ygg".to_string());
                }
            }
        }
    }

    Ok(tags)
}

fn get_url_tags(url: &Url) -> Vec<String> {
    let mut tags = Vec::new();
    if url.scheme() == "https" {
        tags.push("https".to_string());
    } else {
        tags.push("http".to_string());
    }
    if let Some(host) = url.host_str() {
        let mut is_special_network = false;
        if host.ends_with(".onion") {
            tags.push("tor".to_string());
            is_special_network = true;
        }
        if host.ends_with(".i2p") {
            tags.push("i2p".to_string());
            is_special_network = true;
        }
        if host.ends_with(".ygg") {
            tags.push("ygg".to_string());
            tags.push("alfis".to_string());
            is_special_network = true;
        }

        if url.domain().is_none()
            && let Ok(ip) = host.parse::<std::net::IpAddr>()
        {
            match ip {
                std::net::IpAddr::V4(_) => tags.push("ipv4".to_string()),
                std::net::IpAddr::V6(ip) => {
                    tags.push("ipv6".to_string());
                    if is_ygg(&ip) {
                        tags.push("ygg".to_string());
                        is_special_network = true;
                    }
                }
            }
        }

        // Add clearnet tag for regular domains
        if !is_special_network && url.domain().is_some() {
            tags.push("clearnet".to_string());
        }
    }
    tags
}

/// Update instance tags.
///
/// This function updates instance tags based on URL, network and DNS information.
pub async fn update_instance_tags(client: Client, url: Url, tags: &[String]) -> Vec<String> {
    let mut tags = tags.to_owned();

    // Actualize auto tags
    let url_tags = get_url_tags(&url);
    let network_tags = match get_network_tags(client.clone(), url.clone()).await {
        Ok(tags) => tags,
        Err(e) => {
            debug!("Failed to get network tags: {}", e);
            Vec::new()
        }
    };
    let dns_tags = match get_dns_tags(url.clone()).await {
        Ok(tags) => tags,
        Err(e) => {
            debug!("Failed to get DNS tags: {}", e);
            Vec::new()
        }
    };

    // Remove auto tags
    remove_auto_tags(&mut tags);
    // Combine all tags
    tags.extend(url_tags);
    tags.extend(network_tags);

    // Check if domain resolves to Yggdrasil IPs before moving dns_tags
    let has_ygg_dns = dns_tags.contains(&"ygg".to_string());
    tags.extend(dns_tags);

    // Remove clearnet tag if domain resolves to Yggdrasil IPs
    if has_ygg_dns {
        tags.retain(|tag| tag != "clearnet");
    }

    // Remove duplicates and sort
    tags.sort();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue, SERVER, SET_COOKIE};

    use super::get_response_tags;

    #[test]
    fn response_tags() {
        let cases = [
            (
                "anubis",
                SET_COOKIE,
                "custom-cookie-verification-deadbeef=challenge; Path=/",
                vec!["anubis", "antibot"],
            ),
            (
                "cloudflare",
                SERVER,
                "cloudflare",
                vec!["cloudflare", "antibot"],
            ),
            ("ordinary", SERVER, "nginx", vec![]),
        ];

        for (name, header_name, header_value, expected) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(header_name, HeaderValue::from_static(header_value));

            let actual = get_response_tags(&headers, false).unwrap();
            assert_eq!(actual, expected, "{name}");
        }
    }
}
