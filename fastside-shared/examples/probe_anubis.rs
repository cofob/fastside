//! Read-only live check: cargo run -p fastside-shared --release --example probe_anubis -- services.json report.json
use anyhow::Result;
use fastside_shared::{
    client_builder::build_client,
    config::load_config,
    serde_types::{HttpCodeRanges, StoredData},
};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().unwrap_or_else(|| "services.json".into());
    let output = args.next().unwrap_or_else(|| "anubis-report.json".into());
    let config_path = args.next().map(PathBuf::from);
    let repeats: usize = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(1);
    anyhow::ensure!(repeats > 0, "Repeat count must be positive");
    let mut config = load_config(&config_path)?;
    config.crawler.request_timeout = Duration::from_secs(15);
    let stored: StoredData = serde_json::from_slice(&std::fs::read(input)?)?;
    let mut rows = Vec::new();
    for service in stored.services {
        for instance in &service.instances {
            if !instance.tags.iter().any(|tag| tag == "anubis") {
                continue;
            }
            for attempt in 1..=repeats {
                let target = instance.url.join(&service.test_url)?;
                let start = Instant::now();
                let mut row = json!({"service": service.name, "instance": instance.url, "target": target,
                "follow_redirects": service.follow_redirects, "attempt": attempt});
                let result = tokio::time::timeout(Duration::from_secs(60), async {
                    let client =
                        build_client(&service, &config.crawler, &config.proxies, instance)?;
                    client.probe(target).await
                })
                .await;
                match result {
                    Ok(Ok(response)) => {
                        let status_ok = service
                            .allowed_http_codes
                            .is_allowed(response.status.as_u16());
                        let content_ok = service
                            .search_string
                            .as_ref()
                            .is_none_or(|text| response.body.contains(text));
                        row["challenge_solved"] = json!(response.solved);
                        row["session_reused"] = json!(response.reused);
                        row["algorithm"] = json!(response.algorithm);
                        row["final_status"] = json!(response.status.as_u16());
                        row["status_ok"] = json!(status_ok);
                        row["content_ok"] = json!(content_ok);
                        row["service_ok"] = json!(status_ok && content_ok);
                    }
                    Ok(Err(error)) => {
                        row["error"] = json!(format!("{error:#}"));
                    }
                    Err(_) => {
                        row["error"] = json!("Probe exceeded 60 seconds");
                    }
                }
                row["elapsed_ms"] = json!(start.elapsed().as_millis());
                println!("{row}");
                rows.push(row);
                std::fs::write(&output, serde_json::to_vec_pretty(&rows)?)?;
            }
        }
    }
    Ok(())
}
