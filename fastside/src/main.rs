//! Fastside API server.
mod crawler;
mod errors;
mod filters;
mod routes;
mod search;
mod types;
mod utils;

use actix_web::{middleware::Logger, web, App, HttpServer};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::load_config;
use crawler::Crawler;
use fastside_shared::{
    config::{self, AppConfig},
    errors::CliError,
    log_setup,
    serde_types::{ServicesData, StoredData},
};
use log_setup::configure_logging;
use regex::Regex;
use routes::main_scope;
use std::{
    collections::HashMap,
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};
use tokio::sync::RwLock;
use types::{CompiledRegexSearch, LoadedData};

#[deny(unused_imports)]
#[deny(unused_variables)]
#[deny(unused_mut)]
#[deny(unsafe_code)]
// Dependencies
#[macro_use]
extern crate log;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

// This function is needed to take ownership over cloned reference to crawler.
async fn crawler_loop(crawler: Arc<Crawler>) {
    crawler.crawler_loop().await
}

// This function loads services file
fn load_services(data_path: &PathBuf, config: &AppConfig) -> Result<LoadedData> {
    if !data_path.is_file() {
        return Err(anyhow::anyhow!(
            "services file does not exist or is not a file"
        ));
    }
    let data_content =
        std::fs::read_to_string(data_path).context("failed to read services file")?;
    let stored_data: StoredData =
        serde_json::from_str(&data_content).context("failed to parse services file")?;
    let services_data: ServicesData = stored_data
        .services
        .into_iter()
        .map(|service| (service.name.clone(), service))
        .collect();
    Ok(LoadedData {
        services: services_data,
        proxies: config.proxies.clone(),
        default_user_config: config.default_user_config.clone(),
    })
}

// This functions check every 5 seconds if services file has changed and reloads it if it has.
async fn reload_services(
    data_path: PathBuf,
    config: Arc<AppConfig>,
    crawler: Arc<Crawler>,
    data: Arc<RwLock<LoadedData>>,
) {
    let mut file_stat = std::fs::metadata(&data_path).expect("failed to get file metadata");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let new_file_stat = std::fs::metadata(&data_path).expect("failed to get file metadata");
        if new_file_stat
            .modified()
            .expect("failed to get modified time")
            != file_stat.modified().expect("failed to get modified time")
        {
            info!("Reloading services file");
            let new_data = load_services(&data_path, &config).expect("failed to load services");
            *data.write().await = new_data;
            file_stat = new_file_stat;
            crawler
                .update_crawl()
                .await
                .expect("failed to update crawl");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    configure_logging(&cli.log_level).ok();

    match &cli.command {
        Some(Commands::Serve {
            services,
            listen,
            workers,
        }) => {
            let config = Arc::new(load_config(&cli.config).context("failed to load config")?);

            let data_path = services
                .clone()
                .or(config.services_path.clone())
                .unwrap_or_else(|| PathBuf::from_str("services.json").unwrap());

            let listen: SocketAddr = listen
                .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new([127, 0, 0, 1].into(), 8080)));
            let workers: usize = workers.unwrap_or_else(num_cpus::get);

            let data: Arc<RwLock<LoadedData>> = {
                let data = load_services(&data_path, &config)?;
                Arc::new(RwLock::new(data))
            };
            let regexes: HashMap<String, Vec<CompiledRegexSearch>> = data
                .read()
                .await
                .services
                .iter()
                .filter_map(|(name, service)| {
                    let regexes = service
                        .regexes
                        .iter()
                        .map(|regex| {
                            let compiled = Regex::new(&regex.regex)
                                .context(format!("failed to compile regex for {}", name))
                                .ok()?;
                            Some(CompiledRegexSearch {
                                regex: compiled,
                                url: regex.url.clone(),
                            })
                        })
                        .collect::<Option<Vec<CompiledRegexSearch>>>()?;
                    Some((name.clone(), regexes))
                })
                .collect();

            let crawler = Arc::new(Crawler::new(data.clone(), config.crawler.clone()));

            let cloned_crawler = crawler.clone();
            let crawler_loop_handle = tokio::spawn(crawler_loop(cloned_crawler));

            let reload_services_handle = tokio::spawn(reload_services(
                data_path.clone(),
                config.clone(),
                crawler.clone(),
                data.clone(),
            ));

            info!("Listening on {}", listen);

            let config_web_data = web::Data::from(config.clone());
            let crawler_web_data = web::Data::from(crawler.clone());
            let data_web_data = web::Data::from(data.clone());
            let regexes_web_data = web::Data::new(regexes);

            HttpServer::new(move || {
                let logger = Logger::default();
                App::new()
                    .wrap(logger)
                    .app_data(config_web_data.clone())
                    .app_data(crawler_web_data.clone())
                    .app_data(data_web_data.clone())
                    .app_data(regexes_web_data.clone())
                    .service(main_scope(&config.clone()))
            })
            .bind(listen)?
            .workers(workers)
            .run()
            .await
            .context("failed to start api server")?;

            reload_services_handle.abort();
            crawler_loop_handle.abort();
        }
        None => {
            return Err(CliError::NoSubcommand)
                .context("no subcommand was used. Pass --help to view available commands")?;
        }
    };

    Ok(())
}
