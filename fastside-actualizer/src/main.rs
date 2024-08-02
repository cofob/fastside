//! Fastside services.json actualizer.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fastside_shared::{
    errors::CliError,
    log_setup::configure_logging,
    serde_types::{ServicesData, StoredData},
};

mod services;
mod types;
mod serde_types;
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
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    configure_logging(&cli.log_level).ok();

    match &cli.command {
        Some(Commands::Actualize { services, output }) => {
            let output = output.as_ref().unwrap_or(services);
            debug!("Output file: {:?}", output);

            debug!("Reading services file: {:?}", services);
            let data_content =
                std::fs::read_to_string(services).context("failed to read services file")?;
            let stored_data: StoredData =
                serde_json::from_str(&data_content).context("failed to parse services file")?;
            let mut services_data: ServicesData = stored_data
                .services
                .into_iter()
                .map(|service| (service.name.clone(), service))
                .collect();

            for (name, service) in services_data {
                let updater = match services::get_service_updater(&name) {
                    Some(updater) => updater,
                    None => {
                        debug!("No updater found for service: {}", name);
                        continue;
                    }
                };
                info!("Updating service: {}", name);
                let updated_service = updater
                    .update(&service.instances)
                    .await
                    .context("failed to update service")?;
                debug!("Updated service: {:?}", updated_service);
            }

            todo!("implement actualize command");
        }
        None => Err(CliError::NoSubcommand)
            .context("no subcommand was used. Pass --help to view available commands")?,
    }
}
