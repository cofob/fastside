mod config;
mod crawler;
mod errors;
mod log_setup;
mod routes;
mod serde_types;

use crate::crawler::Crawler;
use crate::serde_types::{load_services_file, Services};

use actix_web::{middleware::Logger, web, App, HttpServer};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::load_config;
use log_setup::configure_logging;
use routes::main_scope;
use std::{
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

            let services: Arc<Services> = Arc::new(load_services_file(
                services
                    .clone()
                    .unwrap_or_else(|| PathBuf::from_str("services.json").unwrap())
                    .as_path(),
            )?);

            let crawler = Arc::new(Crawler::new(services, config.crawler.clone()));

            let cloned_crawler = crawler.clone();
            let crawler_loop_handle = tokio::spawn(crawler_loop(cloned_crawler));

            info!("Listening on {}", listen);

            let cloned_config = config.clone();
            HttpServer::new(move || {
                let logger = Logger::default();
                App::new()
                    .wrap(logger)
                    .app_data(web::Data::new(cloned_config.clone()))
                    .app_data(web::Data::new(crawler.clone()))
                    .service(main_scope(&cloned_config))
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
