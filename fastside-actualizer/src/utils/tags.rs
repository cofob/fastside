use anyhow::Result;
use async_std_resolver::{
    config,
    proto::rr::{RData, RecordType},
    resolver,
};
use ipnet::Ipv6Net;
use reqwest::Client;
use url::Url;

use super::log_err::LogErrResult;

const AUTO_TAGS: [&str; 9] = [
    "ipv4",
    "ipv6",
    "https",
    "http",
    "tor",
    "i2p",
    "ygg",
    "alfis",
    "cloudflare",
];
const HIDDEN_DOMAINS: [&str; 2] = [".onion", ".i2p"];

fn remove_auto_tags(tags: &mut Vec<String>) {
    tags.retain(|tag| !AUTO_TAGS.contains(&tag.as_str()));
}

async fn get_network_tags(client: Client, url: Url) -> Result<Vec<String>> {
    let is_hidden = if let Some(domain) = url.domain() {
        HIDDEN_DOMAINS.iter().any(|d| domain.ends_with(d))
    } else {
        false
    };

    let mut tags = Vec::new();
    let response = client.get(url).send().await?;
    let headers = response.headers();
    if let Some(header) = headers.get("Server") {
        let header_str = header.to_str()?;
        if !is_hidden && header_str.contains("cloudflare") {
            tags.push("cloudflare".to_string());
        }
    }
    Ok(tags)
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
                warn!("Get something other than IP: {:?}", rdata);
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
        if host.ends_with(".onion") {
            tags.push("tor".to_string());
        }
        if host.ends_with(".i2p") {
            tags.push("i2p".to_string());
        }
        if host.ends_with(".ygg") {
            tags.push("ygg".to_string());
            tags.push("alfis".to_string());
        }
        if url.domain().is_none() {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
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
        }
    }
    tags
}

/// Update instance tags.
///
/// This function updates instance tags based on URL, network and DNS information.
pub async fn update_instance_tags(client: Client, url: Url, tags: &[String]) -> Vec<String> {
    let mut tags = tags.to_owned();
    // Remove auto tags
    remove_auto_tags(&mut tags);
    // Actualize auto tags
    tags.extend(get_url_tags(&url));
    tags.extend(
        get_network_tags(client, url.clone())
            .await
            .log_err(module_path!(), "Failed to get network tags")
            .unwrap_or_default(),
    );
    tags.extend(
        get_dns_tags(url)
            .await
            .log_err(module_path!(), "Failed to get DNS tags")
            .unwrap_or_default(),
    );
    // Remove duplicates and sort
    tags.sort();
    tags.dedup();
    tags
}
