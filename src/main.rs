mod config;
mod crawler;
mod errors;
mod log_setup;
mod routes;
mod search;
mod serde_types;

use crate::crawler::Crawler;
use crate::serde_types::ServicesData;

use actix_web::{middleware::Logger, web, App, HttpServer};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::load_config;
use log_setup::configure_logging;
use regex::Regex;
use routes::main_scope;
use serde_types::Service;
use std::{
    collections::HashMap,
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};
use thiserror::Error;

#[deny(unused_imports)]
#[deny(unused_variables)]
#[deny(unused_mut)]
#[deny(unsafe_code)]
// Dependencies
#[macro_use]
extern crate log;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    /// Run API server.
    Serve {
        /// Services file path.
        #[arg(short, long)]
        services: Option<PathBuf>,
        /// Listen socket address.
        #[arg(short, long)]
        listen: Option<SocketAddr>,
        /// Worker count.
        #[arg(short, long)]
        workers: Option<usize>,
    },
}

#[derive(Error, Debug)]
pub enum CliError {
    #[error("no subcommand was used")]
    NoSubcommand,
}

// This function is needed to take ownership over cloned reference to crawler.
async fn crawler_loop(crawler: Arc<Crawler>) {
    crawler.crawler_loop().await
}

#[tokio::main]
async fn main() -> Result<()> {
    configure_logging().ok();

    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Serve {
            services,
            listen,
            workers,
        }) => {
            let config = load_config().context("failed to load config")?;

            let listen: SocketAddr = listen
                .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new([127, 0, 0, 1].into(), 8080)));
            let workers: usize = workers.unwrap_or_else(num_cpus::get);

            let services: Arc<ServicesData> = {
                let services_path = services
                    .clone()
                    .unwrap_or_else(|| PathBuf::from_str("services.json").unwrap());
                let services_content = std::fs::read_to_string(services_path)
                    .context("failed to read services file")?;
                let services_vec: Vec<Service> = serde_json::from_str(&services_content)
                    .context("failed to parse services file")?;
                let services_data: ServicesData = services_vec
                    .into_iter()
                    .map(|service| (service.name.clone(), service))
                    .collect();

                Arc::new(services_data)
            };
            let regexes: HashMap<String, Regex> = services
                .iter()
                .filter_map(|(name, service)| {
                    service.regex.as_ref().map(|regex| {
                        (
                            name.clone(),
                            Regex::new(regex).expect("failed to compile regex"),
                        )
                    })
                })
                .collect();

            let crawler = Arc::new(Crawler::new(services.clone(), config.crawler.clone()));

            let cloned_crawler = crawler.clone();
            let crawler_loop_handle = tokio::spawn(crawler_loop(cloned_crawler));

            info!("Listening on {}", listen);

            let config_web_data = web::Data::new(config.clone());
            let crawler_web_data = web::Data::from(crawler.clone());
            let services_web_data = web::Data::from(services.clone());
            let regexes_web_data = web::Data::new(regexes.clone());

            HttpServer::new(move || {
                let logger = Logger::default();
                App::new()
                    .wrap(logger)
                    .app_data(config_web_data.clone())
                    .app_data(crawler_web_data.clone())
                    .app_data(services_web_data.clone())
                    .app_data(regexes_web_data.clone())
                    .service(main_scope(&config.clone()))
            })
            .bind(listen)?
            .workers(workers)
            .run()
            .await
            .context("failed to start api server")?;

            crawler_loop_handle.abort();
        }
        None => {
            return Err(CliError::NoSubcommand)
                .context("no subcommand was used. Pass --help to view available commands")?;
        }
    };

    Ok(())
}
