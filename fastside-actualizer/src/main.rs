//! Fastside services.json actualizer.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use fastside_shared::{
    client_builder::build_client,
    config::{load_config, CrawlerConfig, ProxyData},
    errors::CliError,
    log_setup::configure_logging,
    parallel::Parallelise,
    serde_types::{Instance, Service, StoredData},
};
use serde_types::ActualizerData;
use tokio::sync::Mutex;
use url::Url;
use utils::{log_err::LogErrResult, normalize::normalize_instances, tags::update_instance_tags};

mod serde_types;
mod services;
mod types;
mod utils;

#[macro_use]
extern crate log;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// Path to the configuration file.
    #[arg(short, long, default_value = None)]
    config: Option<PathBuf>,
    /// Log level. Takes precedence over the FS__LOG env variable. Default is INFO.
    #[arg(long, default_value = None)]
    log_level: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Actualize services.json.
    Actualize {
        /// Services file path.
        #[arg(default_value = "services.json")]
        services: PathBuf,
        /// Output file path. Default is writing to services.json.
        #[arg(short, long, default_value = None)]
        output: Option<PathBuf>,
        /// Data file path.
        #[arg(short, long, default_value = "data.json")]
        data: PathBuf,
        /// Amount of maximum parallel requests.
        #[arg(long, default_value = None)]
        max_parallel: Option<usize>,
        /// List of service names to actualize.
        /// If not provided, all services will be actualized.
        #[arg(short = 'u', long = "update", default_value = None)]
        update_service_names: Option<Vec<String>>,
    },
}

/// Changes summary inner.
#[derive(Debug)]
struct ChangesSummaryInner {
    dead_instances_removed: Vec<Url>,
    new_instances_added: HashMap<String, Vec<Url>>,
    empty_services: Vec<String>,
}

/// Changes summary.
///
/// This struct is used to store changes that happened during the actualization process.
/// Clone is cheap because it only clones the Arc. Actions on the inner data are locked
/// with a mutex.
#[derive(Debug, Clone)]
pub struct ChangesSummary {
    inner: Arc<Mutex<ChangesSummaryInner>>,
}

impl ChangesSummary {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ChangesSummaryInner {
                dead_instances_removed: Vec::new(),
                new_instances_added: HashMap::new(),
                empty_services: Vec::new(),
            })),
        }
    }

    /// Get a summary of the changes.
    pub async fn summary(&self) -> String {
        let inner = self.inner.lock().await;

        let mut out = String::new();

        // Dead instances removed
        out.push_str(
            format!(
                "Dead instances removed: (total: {})\n",
                inner.dead_instances_removed.len()
            )
            .as_str(),
        );
        if inner.dead_instances_removed.is_empty() {
            out.push_str("(empty)\n");
        }
        for instance in &inner.dead_instances_removed {
            out.push_str(&format!("- {}\n", instance));
        }
        out.push('\n');

        // New instances added
        let total_new_instances: usize = inner.new_instances_added.values().map(|v| v.len()).sum();
        out.push_str(format!("New instances added: (total: {})\n", total_new_instances).as_str());
        if inner.new_instances_added.is_empty() {
            out.push_str("(empty)\n");
        }
        for (service_name, instances) in &inner.new_instances_added {
            out.push_str(&format!(
                "- service: {} (total: {})\n",
                service_name,
                instances.len()
            ));
            for instance in instances {
                out.push_str(&format!("  - {}\n", instance));
            }
        }
        out.push('\n');

        // Empty services
        out.push_str(
            format!(
                "Empty services without deprecation: (total: {})\n",
                inner.empty_services.len()
            )
            .as_str(),
        );
        if inner.empty_services.is_empty() {
            out.push_str("(empty)\n");
        }
        for service in &inner.empty_services {
            out.push_str(&format!("- {}\n", service));
        }

        out
    }

    /// Extend dead instances removed.
    pub async fn extend_dead_instances_removed(&self, instances: Vec<Url>) {
        let mut inner = self.inner.lock().await;
        inner.dead_instances_removed.extend(instances);
    }

    /// Set new instances added.
    pub async fn set_new_instances_added(&self, service_name: &str, instances: Vec<Url>) {
        if instances.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().await;
        inner
            .new_instances_added
            .insert(service_name.to_string(), instances);
    }

    /// Set empty services.
    pub async fn extend_empty_services(&self, services: Vec<String>) {
        if services.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().await;
        inner.empty_services.extend(services);
    }
}

impl Default for ChangesSummary {
    fn default() -> Self {
        Self::new()
    }
}

/// Update service instances by fetching new instances from the service update.
async fn update_service(
    service: &mut Service,
    client: reqwest::Client,
    changes_summary: ChangesSummary,
) {
    let name = service.name.clone();
    debug!("Updating service: {}", name);
    match services::get_service_updater(&name) {
        Some(updater) => {
            let updated_instances_result = updater
                .update(client, &service.instances, changes_summary.clone())
                .await;
            match updated_instances_result {
                Ok(updated_instances) => {
                    debug!("Updated instances: {:?}", updated_instances);
                    service.instances = normalize_instances(&updated_instances)
                }
                Err(e) => {
                    error!("Failed to update service {name}: {e}");
                    service.instances = normalize_instances(&service.instances);
                }
            }
        }
        None => {
            debug!("No updater found for service {}", name);
            service.instances = normalize_instances(&service.instances);
        }
    };
}

/// Check a single instance.
async fn check_single_instance(
    checker: Arc<dyn crate::types::InstanceChecker + Sync + Send>,
    client: reqwest::Client,
    service: Arc<Service>,
    instance: Instance,
) -> Result<(Instance, Vec<String>, bool)> {
    let is_alive = {
        debug!("Checking instance: {url}", url = instance.url);
        let res = checker.check(client.clone(), &service, &instance).await;
        match res {
            Ok(is_alive) => is_alive,
            Err(e) => {
                debug!("Failed to check instance {url}: {e}", url = instance.url);
                false
            }
        }
    };
    debug!(
        "Instance {url} is alive: {is_alive}",
        url = instance.url,
        is_alive = is_alive
    );

    let tags = update_instance_tags(client, instance.url.clone(), &instance.tags).await;

    Ok((instance, tags, is_alive))
}

/// Check instances for a service.
///
/// This function will check all instances of a service and update their ping history.
async fn check_instances(
    actualizer_data: &mut ActualizerData,
    proxies: &ProxyData,
    name: &str,
    service: &mut Service,
    config: &CrawlerConfig,
    max_parallel: &Option<usize>,
) -> Result<()> {
    let checker: Arc<dyn crate::types::InstanceChecker + Send + Sync + 'static> =
        Arc::from(services::get_instance_checker(name));

    let service_history = actualizer_data
        .services
        .entry(name.to_string())
        .or_default();
    let service_arc = Arc::new(service.clone());

    let mut tasks = match max_parallel {
        Some(max_parallel) => Parallelise::with_capacity(*max_parallel),
        None => Parallelise::with_cpus(),
    };

    for instance in service.instances.iter() {
        let client = build_client(&service_arc, config, proxies, instance)?;
        tasks
            .push(tokio::spawn(check_single_instance(
                checker.clone(),
                client,
                service_arc.clone(),
                instance.clone(),
            )))
            .await;
    }

    let results = tasks.wait().await;

    for result in results {
        let (instance_clone, tags, is_alive) = match result {
            Ok(r) => r,
            Err(e) => {
                error!("Error occured during checking instance: {e}");
                continue;
            }
        };

        let instance_history = match service_history.get_instance_mut(&instance_clone.url) {
            Some(instance_history) => instance_history,
            None => {
                service_history.add_instance(&instance_clone.clone());
                service_history
                    .get_instance_mut(&instance_clone.url)
                    .unwrap()
            }
        };
        instance_history.ping_history.cleanup();
        instance_history.ping_history.push_ping(is_alive);

        let instance_mut = service
            .instances
            .iter_mut()
            .find(|i| i.url == instance_clone.url)
            .unwrap();
        instance_mut.tags = tags;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    configure_logging(&cli.log_level).ok();

    match &cli.command {
        Some(Commands::Actualize {
            services,
            output,
            data,
            max_parallel,
            update_service_names,
        }) => {
            let config = load_config(&cli.config).context("failed to load config")?;

            let output = output.as_ref().unwrap_or(services);
            debug!("Output file: {:?}", output);

            debug!("Reading data file: {:?}", data);
            let mut actualizer_data: ActualizerData = {
                if !data.is_file() {
                    warn!("Data file does not exist, creating new data");
                    ActualizerData::new()
                } else {
                    let data_content =
                        std::fs::read_to_string(data).context("failed to read data file")?;
                    serde_json::from_str(&data_content).context("failed to parse data file")?
                }
            };

            debug!("Reading services file: {:?}", services);
            let stored_data: StoredData = {
                if !services.is_file() {
                    return Err(anyhow!("services file does not exist"));
                }
                let data_content =
                    std::fs::read_to_string(services).context("failed to read services file")?;
                serde_json::from_str(&data_content).context("failed to parse services file")?
            };
            let mut services_data: HashMap<String, Service> = stored_data
                .services
                .into_iter()
                .map(|service| (service.name.clone(), service))
                .collect();

            // Check if all services from update_service_names exist
            if let Some(update_service_names) = update_service_names {
                for name in update_service_names.iter() {
                    if !services_data.contains_key(name) {
                        return Err(anyhow!("service {name:?} does not exist"));
                    }
                }
            }

            let changes_summary = ChangesSummary::new();

            let start = std::time::Instant::now();

            actualizer_data.remove_removed_services(&services_data);
            actualizer_data.remove_removed_instances(&services_data);

            let update_service_client = reqwest::Client::new();

            // Filter services data
            let mut filtered_services_data = services_data
                .iter_mut()
                .filter(|(name, _)| {
                    if let Some(update_service_names) = update_service_names {
                        update_service_names.contains(name)
                    } else {
                        true
                    }
                })
                .collect::<HashMap<_, _>>();
            let length = filtered_services_data.len();

            for (i, (name, service)) in filtered_services_data.iter_mut().enumerate() {
                info!(
                    "Actualizing service {name} ({i}/{length})",
                    name = name,
                    i = i + 1,
                    length = length
                );
                update_service(
                    service,
                    update_service_client.clone(),
                    changes_summary.clone(),
                )
                .await;
                check_instances(
                    &mut actualizer_data,
                    &config.proxies,
                    name,
                    service,
                    &config.crawler,
                    max_parallel,
                )
                .await
                .log_err(
                    module_path!(),
                    &format!("failed to check instances for service {name}"),
                )
                .ok();
            }

            let dead_instances = actualizer_data.remove_dead_instances(&mut services_data);
            changes_summary
                .extend_dead_instances_removed(dead_instances)
                .await;

            // Find empty services
            let empty_services: Vec<String> = services_data
                .iter()
                .filter_map(|(name, service)| {
                    if service.instances.is_empty() && service.deprecated_message.is_none() {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect();
            changes_summary.extend_empty_services(empty_services).await;

            let elapsed = start.elapsed();
            info!("Elapsed time: {:?}", elapsed);

            let summary = changes_summary.summary().await;
            info!("Summary:\n{}", summary);

            // Sort actualizer data service instances
            for service_history in actualizer_data.services.values_mut() {
                service_history.instances.sort_by(|a, b| a.url.cmp(&b.url));
            }
            // Write actualizer data back to file
            let data_content = serde_json::to_string_pretty(&actualizer_data)
                .context("failed to serialize data")?;
            std::fs::write(data, data_content).context("failed to write data file")?;
            // Sort services data
            let stored_data = StoredData {
                services: {
                    let mut stored_services: Vec<Service> = services_data.into_values().collect();
                    stored_services.sort_by(|a, b| a.name.cmp(&b.name));
                    for service in stored_services.iter_mut() {
                        service.instances.sort_by(|a, b| a.url.cmp(&b.url));
                    }
                    stored_services
                },
            };
            // Write services back to file
            let services_content = serde_json::to_string_pretty(&stored_data)
                .context("failed to serialize services")?;
            std::fs::write(output, services_content).context("failed to write services file")?;
        }
        None => Err(CliError::NoSubcommand)
            .context("no subcommand was used. Pass --help to view available commands")?,
    }

    Ok(())
}
