//! Fastside API server.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fastside::{
    app,
    captcha::NativeCaptchaVerifier,
    crawler::{Crawler, ReqwestInstanceClient},
    reputation::{VoteProtector, cleanup_loop, validate_config as validate_reputation_config},
    storage::create_native_store,
    types::{AppState, LoadedData, compile_regexes},
};
use fastside_shared::{
    config::{AppConfig, load_config},
    errors::CliError,
    log_setup,
    serde_types::{ServicesData, StoredData},
};
use log_setup::configure_logging;
use std::{
    net::{SocketAddr, SocketAddrV4},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};
use tokio::sync::RwLock;
use url::Url;

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
        /// Services path.
        #[arg(short, long)]
        services: Option<String>,
        /// Listen socket address.
        #[arg(short, long)]
        listen: Option<SocketAddr>,
        /// Worker count.
        #[arg(short, long)]
        workers: Option<usize>,
        /// Skip waiting for initial ping and start serving immediately.
        #[arg(long)]
        skip_wait: bool,
    },
    /// Validate services file.
    Validate {
        /// Services path.
        #[arg(short, long)]
        services: Option<String>,
    },
}

// This function is needed to take ownership over cloned reference to crawler.
async fn crawler_loop(crawler: Arc<Crawler>) {
    crawler.crawler_loop().await
}

#[derive(Debug)]
enum ServicesSource {
    Filesystem(PathBuf),
    Remote(Url),
}

impl FromStr for ServicesSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Ok(url) = Url::parse(s) {
            Ok(ServicesSource::Remote(url))
        } else {
            Ok(ServicesSource::Filesystem(PathBuf::from(s)))
        }
    }
}

// This function loads services from file or remote source
async fn load_services_data(source: &ServicesSource) -> Result<String> {
    debug!("Loading services from {:?}", source);
    Ok(match source {
        ServicesSource::Filesystem(path) => {
            if !path.is_file() {
                return Err(anyhow::anyhow!(
                    "services file does not exist or is not a file"
                ));
            }
            std::fs::read_to_string(path).context("failed to read services file")?
        }
        ServicesSource::Remote(url) => reqwest::get(url.clone())
            .await
            .context("failed to fetch services file")?
            .text()
            .await
            .context("failed to read services file")?,
    })
}

// This function loads services file
async fn load_services(source: &ServicesSource, config: &AppConfig) -> Result<LoadedData> {
    let data_content = load_services_data(source).await?;
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
    source: &ServicesSource,
    config: Arc<AppConfig>,
    crawler: Arc<Crawler>,
    data: Arc<RwLock<LoadedData>>,
) -> Result<()> {
    let reload_interval = config.auto_updater.interval.as_secs();
    match &source {
        ServicesSource::Filesystem(path) => {
            let mut file_stat = std::fs::metadata(path).context("failed to get file metadata")?;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(reload_interval)).await;
                let new_file_stat =
                    std::fs::metadata(path).context("failed to get file metadata")?;
                debug!("File modified: {:?}", new_file_stat.modified());
                if new_file_stat
                    .modified()
                    .context("failed to get modified time")?
                    != file_stat
                        .modified()
                        .context("failed to get modified time")?
                {
                    info!("Reloading services file");
                    let new_data = load_services(source, &config)
                        .await
                        .context("failed to load services")?;
                    *data.write().await = new_data;
                    file_stat = new_file_stat;
                    crawler
                        .update_crawl()
                        .await
                        .context("failed to update crawl")?;
                }
            }
        }
        ServicesSource::Remote(url) => {
            let client = reqwest::Client::new();
            let mut etag = client
                .head(url.clone())
                .send()
                .await
                .context("failed to send HEAD request")?
                .headers()
                .get("etag")
                .map(|header| header.to_str().expect("failed to parse etag").to_string())
                .context("failed to get etag")?;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(reload_interval)).await;
                let new_etag = client
                    .head(url.clone())
                    .send()
                    .await
                    .context("failed to send HEAD request")?
                    .headers()
                    .get("etag")
                    .map(|header| header.to_str().expect("failed to parse etag").to_string())
                    .context("failed to get etag")?;
                debug!("Etag: {}", etag);
                if new_etag != etag {
                    info!("Reloading services file");
                    let new_data = load_services(source, &config)
                        .await
                        .context("failed to load services")?;
                    *data.write().await = new_data;
                    etag = new_etag;
                    crawler
                        .update_crawl()
                        .await
                        .context("failed to update crawl")?;
                }
            }
        }
    }
}

async fn reload_services_wrapper(
    source: ServicesSource,
    config: Arc<AppConfig>,
    crawler: Arc<Crawler>,
    data: Arc<RwLock<LoadedData>>,
) {
    if !config.auto_updater.enabled {
        debug!("Auto updater is disabled");
        return;
    }
    let mut restart_counter = 0;
    loop {
        if let Err(e) =
            reload_services(&source, config.clone(), crawler.clone(), data.clone()).await
        {
            error!("Failed to reload services: {}", e);
            restart_counter += 1;
        }
        let restart_in = 60 * restart_counter;
        error!("Reload services failed, retrying in {}", restart_in);
        tokio::time::sleep(std::time::Duration::from_secs(restart_in)).await;
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    configure_logging(&cli.log_level).ok();

    let worker_threads = match &cli.command {
        Some(Commands::Serve { workers, .. }) => workers.unwrap_or_else(available_workers),
        _ => available_workers(),
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .context("failed to build async runtime")?
        .block_on(run(cli))
}

fn available_workers() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Some(Commands::Serve {
            services,
            listen,
            workers: _,
            skip_wait,
        }) => {
            let config = Arc::new(load_config(&cli.config).context("failed to load config")?);
            validate_reputation_config(&config)
                .map_err(anyhow::Error::msg)
                .context("invalid reputation configuration")?;

            // Check if we should skip waiting for initial ping
            let should_skip_wait = *skip_wait
                || std::env::var("FS__SKIP_WAIT")
                    .map(|v| v.to_lowercase() == "true")
                    .unwrap_or(false);

            let services_source = ServicesSource::from_str(
                &services
                    .clone()
                    .or(config.services.clone())
                    .unwrap_or_else(|| {
                        // If services.json exists in the current directory, use it.
                        if PathBuf::from("services.json").is_file() {
                            debug!("Using services.json in the current directory");
                            return String::from("services.json");
                        }
                        // Otherwise, use the default services source.
                        debug!("Using default services source");
                        String::from(
                            "https://raw.githubusercontent.com/cofob/fastside/master/services.json",
                        )
                    }),
            )?;
            debug!("Using services source: {:?}", services_source);

            let listen: SocketAddr = listen
                .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new([127, 0, 0, 1].into(), 8080)));

            let data: Arc<RwLock<LoadedData>> = {
                let data = load_services(&services_source, &config).await?;
                Arc::new(RwLock::new(data))
            };
            let regexes = Arc::new(compile_regexes(&data.read().await.services));

            let state_store = create_native_store(&config.storage)
                .await
                .context("failed to initialize state storage")?;
            let crawler = Arc::new(Crawler::new_with_store(
                data.clone(),
                config.crawler.clone(),
                Arc::new(ReqwestInstanceClient),
                state_store.clone(),
            ));

            let initialized_from_storage = match state_store
                .load_crawl_snapshot()
                .await
                .context("failed to load crawler snapshot")?
            {
                Some(snapshot) => crawler.restore_snapshot(snapshot).await,
                None => false,
            };

            if !initialized_from_storage && should_skip_wait {
                // Initialize with defaults and start crawler loop in background
                crawler.initialize_with_defaults().await;
                info!("Starting server immediately with default data from services.json");
                info!("Initial ping will run in background");
            }

            let cloned_crawler = crawler.clone();
            let crawler_loop_handle = tokio::spawn(crawler_loop(cloned_crawler));

            let reload_services_handle = tokio::spawn(reload_services_wrapper(
                services_source,
                config.clone(),
                crawler.clone(),
                data.clone(),
            ));

            info!("Listening on {}", listen);

            let vote_protector = Arc::new(VoteProtector::default());
            let state = AppState {
                config,
                crawler,
                loaded_data: data,
                regexes,
                state_store,
                captcha_verifier: Arc::new(NativeCaptchaVerifier::default()),
                vote_protector: vote_protector.clone(),
            };
            let protection_cleanup_handle =
                tokio::spawn(cleanup_loop(vote_protector, state.config.clone()));
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .context("failed to bind api listener")?;
            axum::serve(
                listener,
                app(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("failed to start api server")?;

            reload_services_handle.abort();
            crawler_loop_handle.abort();
            protection_cleanup_handle.abort();
        }
        None => {
            return Err(CliError::NoSubcommand)
                .context("no subcommand was used. Pass --help to view available commands")?;
        }
        Some(Commands::Validate { services }) => {
            let services_source = ServicesSource::from_str(
                &services
                    .clone()
                    .unwrap_or_else(|| String::from("services.json")),
            )?;
            debug!("Using services source: {:?}", services_source);

            let data_content = load_services_data(&services_source).await?;
            let stored_data: StoredData =
                serde_json::from_str(&data_content).context("failed to parse services file")?;

            let validation_result = stored_data.validate();

            if validation_result.has_errors() {
                error!("Services file is invalid:");
                error!("{}", validation_result.format());
                return Err(CliError::InvalidServicesFile).context("services file is invalid.")?;
            } else {
                info!("Services file is valid");
                info!("{}", validation_result.format());
            }

            return Ok(());
        }
    };

    Ok(())
}
