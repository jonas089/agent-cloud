//! agentcloud market: lists offers, creates leases, follows payments on chain, runs the escrow
//! and serves the web app, all from one process.

mod api;
mod config;
mod escrow;
mod ledger;
mod store;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use chain::{Chain, Head, Wallet};
use clap::Parser;

use crate::config::Config;
use crate::store::Store;

#[derive(Parser)]
#[command(about = "Run the agentcloud market")]
struct Args {
    /// Path to market.toml. A missing file means all defaults.
    #[arg(long, env = "AGENTCLOUD_MARKET_CONFIG", default_value = "data/market.toml")]
    config: PathBuf,
}

/// What every part of the market shares.
pub struct Market {
    pub config: Config,
    pub store: Store,
    pub chain: Chain,
    pub escrow_address: String,
    /// The newest block the ledger has fully read. Leases are judged against its time.
    pub synced: RwLock<Option<Head>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = Config::load(&Args::parse().config)?;
    std::fs::create_dir_all(&config.data_dir).with_context(|| format!("creating {}", config.data_dir.display()))?;

    let (escrow, created) = Wallet::load_or_create(&config.escrow_key_path(), &config.chain.bech32_prefix)?;
    let escrow_address = escrow.address().to_string();
    if created {
        tracing::info!("created a new escrow account at {}", config.escrow_key_path().display());
    }
    tracing::info!("escrow account {escrow_address}");

    let market = Arc::new(Market {
        store: Store::open(&config.database_path())?,
        chain: Chain::new(config.chain.clone()),
        escrow_address,
        synced: RwLock::new(None),
        config,
    });
    tokio::spawn(ledger::run(market.clone()));
    tokio::spawn(escrow::run(market.clone(), escrow));

    let listener = tokio::net::TcpListener::bind(market.config.listen)
        .await
        .with_context(|| format!("binding {}", market.config.listen))?;
    tracing::info!("serving on http://{}", market.config.listen);
    axum::serve(listener, api::router(market))
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}
