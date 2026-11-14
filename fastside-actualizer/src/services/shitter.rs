use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use serde::Deserialize;
use url::Url;

pub struct ShitterUpdater {
    pub instances_url: String,
    pub d420_url: String,
}

impl ShitterUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://codeberg.org/mv12star/shitter/wiki/raw/Instances".into(),
            d420_url: "https://status.d420.de/api/v1/instances".into(),
        }
    }
}

#[derive(Deserialize)]
struct D420Response {
    hosts: Vec<D420Host>,
}

#[derive(Deserialize)]
struct D420Host {
    url: String,
    healthy: bool,
    is_bad_host: bool,
}

/// Read only healthy hosts from the status.d420.de API.
fn parse_d420_urls(json: &str) -> anyhow::Result<Vec<Url>> {
    let response: D420Response = serde_json::from_str(json)?;
    Ok(response
        .hosts
        .into_iter()
        .filter(|host| host.healthy && !host.is_bad_host)
        .filter_map(|host| Url::parse(&host.url).ok())
        .collect())
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

/// Read only working instances. Other sections contain redirectors and dead hosts.
fn parse_instance_urls(markdown: &str) -> anyhow::Result<Vec<Url>> {
    let mut working = false;
    let mut urls = Vec::new();
    for line in markdown.lines().map(str::trim) {
        if line.starts_with('#') {
            working = line.trim_start_matches('#').trim() == "Working instances";
            continue;
        }
        if !working {
            continue;
        }
        let Some(entry) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else {
            continue;
        };
        // The wiki uses bare URLs, with optional notes such as "(Tor)".
        let Some(value) = entry.split_whitespace().next() else {
            continue;
        };
        let Ok(url) = Url::parse(value) else {
            continue;
        };
        if matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    anyhow::ensure!(
        !urls.is_empty(),
        "No working Shitter instances found in the wiki"
    );
    Ok(urls)
}

#[async_trait]
impl ServiceUpdater for ShitterUpdater {
    async fn update(
        &self,
        client: reqwest::Client,
        current_instances: &[Instance],
        changes_summary: ChangesSummary,
    ) -> anyhow::Result<Vec<Instance>> {
        let mut urls = parse_instance_urls(&fetch_text(&client, &self.instances_url).await?)?;
        // The status API is rate limited, so its errors are not fatal.
        match fetch_text(&client, &self.d420_url)
            .await
            .and_then(|json| parse_d420_urls(&json))
        {
            Ok(d420_urls) => urls.extend(d420_urls),
            Err(error) => log::warn!("Failed to fetch Shitter instances from d420: {error}"),
        }
        let mut instances = current_instances.to_vec();
        let mut added = Vec::new();
        for url in urls {
            if instances.iter().any(|instance| instance.url == url) {
                continue;
            }
            added.push(url.clone());
            instances.push(Instance::from(url));
        }
        changes_summary
            .set_new_instances_added("shitter", added)
            .await;
        Ok(instances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_working_instances() {
        let urls = parse_instance_urls(
            "# Working instances\r\n\
             - https://shitter.example\r\n\
             - https://shitter.example/\r\n\
             - http://example.onion (requires Tor browser)\r\n\
             - invalid\r\n\
             - ftp://files.example\r\n\
             ### Redirectors\r\n\
             - https://redirect.example\r\n\
             ### Active but rate limited\r\n\
             - https://limited.example\r\n\
             ### Formerly active / Taken down\r\n\
             - https://dead.example\r\n\
             # Run a public instance\r\n\
             - https://docs.example\r\n",
        )
        .unwrap();
        assert_eq!(
            urls,
            vec![
                Url::parse("https://shitter.example/").unwrap(),
                Url::parse("http://example.onion/").unwrap(),
            ]
        );
    }

    #[test]
    fn rejects_missing_or_empty_working_list() {
        for input in [
            "<html>Access denied</html>",
            "# Working instances\n",
            "# Redirectors\n- https://redirect.example",
        ] {
            assert!(parse_instance_urls(input).is_err());
        }
    }

    #[test]
    fn discovers_only_healthy_d420_hosts() {
        let urls = parse_d420_urls(
            r#"{"hosts":[
                {"url":"https://ok.example","healthy":true,"is_bad_host":false},
                {"url":"https://down.example","healthy":false,"is_bad_host":false},
                {"url":"https://bad.example","healthy":true,"is_bad_host":true},
                {"url":"invalid","healthy":true,"is_bad_host":false}
            ]}"#,
        )
        .unwrap();
        assert_eq!(urls, vec![Url::parse("https://ok.example/").unwrap()]);
    }
}
