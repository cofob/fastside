use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser)]
#[command(about = "Calculate Anubis proofs for Fastside Workers")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8090")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let token = std::env::var("FASTSIDE_CAPTCHA_SOLVER_TOKEN")
        .context("Set FASTSIDE_CAPTCHA_SOLVER_TOKEN before starting the server")?;
    let app = fastside_captcha_solver::router(token)?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    eprintln!("Captcha solver listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
