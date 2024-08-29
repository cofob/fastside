//! Fastside services.json actualizer.

use std::{path::PathBuf, sync::Arc};

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
    },
}

/// Update service instances by fetching new instances from the service update.
async fn update_service(service: &mut Service, client: reqwest::Client) {
    let name = service.name.clone();
    info!("Updating service: {}", name);
    match services::get_service_updater(&name) {
        Some(updater) => {
            let updated_instances_result = updater
                .update(client, &service.instances)
                .await
                .context("failed to update service");
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
///
/// This function will check a single instance and update its ping history.
/// Returns a tuple of the instance, the client used to check the instance, and whether the instance is alive.
async fn check_single_instance(
    checker: Arc<dyn crate::types::InstanceChecker + Sync + Send>,
    client: reqwest::Client,
    service: Arc<Service>,
    instance: Instance,
) -> Result<(Instance, reqwest::Client, bool)> {
    let is_alive = {
        info!("Checking instance: {url}", url = instance.url);
        let res = checker.check(client.clone(), &service, &instance).await;
        match res {
            Ok(is_alive) => is_alive,
            Err(e) => {
                error!("Failed to check instance {url}: {e}", url = instance.url);
                false
            }
        }
    };
    info!(
        "Instance {url} is alive: {is_alive}",
        url = instance.url,
        is_alive = is_alive
    );
    Ok((instance, client, is_alive))
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
) -> Result<()> {
    let checker: Arc<(dyn crate::types::InstanceChecker + Send + Sync + 'static)> =
        Arc::from(services::get_instance_checker(name));

    let service_history = actualizer_data
        .services
        .entry(name.to_string())
        .or_default();
    let service_arc = Arc::new(service.clone());

    let mut tasks = Parallelise::with_cpus();

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
        let (instance_clone, client, is_alive) = match result {
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
        instance_mut.tags =
            update_instance_tags(client, instance_clone.url.clone(), &instance_mut.tags).await;
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
            let mut services_data = stored_data
                .services
                .into_iter()
                .map(|service| (service.name.clone(), service))
                .collect();

            let start = std::time::Instant::now();

            actualizer_data.remove_removed_services(&services_data);
            actualizer_data.remove_removed_instances(&services_data);

            let update_service_client = reqwest::Client::new();
            for (name, service) in services_data.iter_mut() {
                update_service(service, update_service_client.clone()).await;
                check_instances(
                    &mut actualizer_data,
                    &config.proxies,
                    name,
                    service,
                    &config.crawler,
                )
                .await
                .log_err(&format!("failed to check instances for service {name}"))
                .ok();
            }

            actualizer_data.remove_dead_instances(&mut services_data);

            let elapsed = start.elapsed();
            info!("Elapsed time: {:?}", elapsed);

            // Write data back to file
            let data_content = serde_json::to_string_pretty(&actualizer_data)
                .context("failed to serialize data")?;
            std::fs::write(data, data_content).context("failed to write data file")?;
            let stored_data = StoredData {
                services: services_data.into_values().collect(),
            };
            let services_content = serde_json::to_string_pretty(&stored_data)
                .context("failed to serialize services")?;
            std::fs::write(output, services_content).context("failed to write services file")?;
        }
        None => Err(CliError::NoSubcommand)
            .context("no subcommand was used. Pass --help to view available commands")?,
    }

    Ok(())
}
