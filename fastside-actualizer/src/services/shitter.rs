use crate::{ChangesSummary, types::ServiceUpdater};
use async_trait::async_trait;
use fastside_shared::serde_types::Instance;
use url::Url;

pub struct ShitterUpdater {
    pub instances_url: String,
}

impl ShitterUpdater {
    pub fn new() -> Self {
        Self {
            instances_url: "https://codeberg.org/mv12star/shitter/wiki/raw/Instances".into(),
        }
    }
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
        let response = client
            .get(&self.instances_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let urls = parse_instance_urls(&response)?;
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
    fn catalogue_replaces_nitter() {
        let data: fastside_shared::serde_types::StoredData =
            serde_json::from_str(include_str!("../../../services.json")).unwrap();
        let shitter = data.services.iter().find(|s| s.name == "shitter").unwrap();
        let nitter = data.services.iter().find(|s| s.name == "nitter").unwrap();
        assert!(shitter.deprecated_message.is_none());
        assert!(!shitter.instances.is_empty());
        assert!(
            nitter
                .deprecated_message
                .as_ref()
                .unwrap()
                .contains("Shitter")
        );
        assert!(nitter.instances.is_empty());
        assert!(nitter.regexes.is_empty());
        for url in [
            "https://twitter.com/jack/status/20",
            "https://x.com/jack/status/20",
        ] {
            let matches: Vec<_> = data
                .services
                .iter()
                .filter(|s| {
                    s.regexes
                        .iter()
                        .any(|r| regex::Regex::new(&r.regex).unwrap().is_match(url))
                })
                .map(|s| s.name.as_str())
                .collect();
            assert_eq!(matches, vec!["shitter"]);
        }
        assert!(super::super::get_service_updater("shitter").is_some());
        assert!(super::super::get_service_updater("nitter").is_none());
    }
}
