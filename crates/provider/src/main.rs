//! agentcloud provider: runs on the machine being rented out, normally inside a TEE, and every
//! few seconds:
//!
//! 1. sends the offer to the market as a heartbeat,
//! 2. asks which leases are active,
//! 3. wipes every sandbox whose lease is over and starts one for every active lease that has
//!    none, after checking on chain that the renter really committed to the SSH key the
//!    market handed over, then reports how to connect and the agent's wallet,
//! 4. pays each lease's rent from its agent wallet and returns what is left in the wallets of
//!    ended leases to their renters.
//!
//! When the market cannot be reached nothing is wiped; sandboxes keep running until it
//! answers again.

mod config;
mod market;
mod rent;
mod sandbox;
mod wallets;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chain::Chain;
use clap::Parser;
use protocol::agent_auth::AgentKey;
use protocol::api::{ssh_key_commitment, Lease, MarketConfig, OfferSpec, SandboxReport};
use protocol::memo::Memo;

use crate::config::Config;
use crate::market::MarketClient;
use crate::rent::Rent;
use crate::sandbox::{Provision, Sandboxes};
use crate::wallets::LeaseWallets;

#[derive(Parser)]
#[command(about = "Run sandboxes for agentcloud leases")]
struct Args {
    /// Path to provider.toml; ignored when the config is passed inline (AGENTCLOUD_PROVIDER_TOML).
    #[arg(long, env = "AGENTCLOUD_PROVIDER_CONFIG", default_value = "data/provider.toml")]
    config: PathBuf,
}

struct Provider {
    offer: OfferSpec,
    market: MarketClient,
    market_config: MarketConfig,
    chain: Chain,
    sandboxes: Sandboxes,
    wallets: LeaseWallets,
    rent: Rent,
    /// Leases whose SSH key was found committed on chain by their renter.
    verified: HashSet<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = Config::load(&Args::parse().config)?;
    std::fs::create_dir_all(&config.data_dir)?;
    let key = load_or_create_key(&config.data_dir.join("agent.key"))?;
    tracing::info!("offer id {}", key.offer_id());

    let market = MarketClient::new(&config.market, key);
    let market_config = loop {
        match market.config().await {
            Ok(found) => break found,
            Err(error) => tracing::warn!("waiting for the market: {error:#}"),
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    };
    let chain = Chain::new(market_config.chain.clone());
    let mut provider = Provider {
        wallets: LeaseWallets::open(config.data_dir.join("wallets"), &chain.config().bech32_prefix)?,
        rent: Rent::new(chain.clone()),
        sandboxes: Sandboxes::prepare(config.sandbox, &config.data_dir).await?,
        offer: config.offer,
        market,
        market_config,
        chain,
        verified: HashSet::new(),
    };
    let interval = Duration::from_secs(config.poll_seconds);
    loop {
        if let Err(error) = provider.tick().await {
            tracing::warn!("{error:#}");
        }
        tokio::time::sleep(interval).await;
    }
}

impl Provider {
    async fn tick(&mut self) -> anyhow::Result<()> {
        self.market.heartbeat(&self.offer).await?;
        let leases = self.market.active_leases().await?;
        self.reconcile_sandboxes(&leases).await?;
        self.rent.pay(&self.market, &leases, &self.wallets).await?;
        self.rent.refund_ended(&leases, &self.wallets).await
    }

    async fn reconcile_sandboxes(&mut self, leases: &[Lease]) -> anyhow::Result<()> {
        self.start_and_report(leases).await?;
        for sandbox in self.sandboxes.list().await?.iter().filter(|s| s.running) {
            if let Err(error) = self.sandboxes.enforce_disk_quota(sandbox, self.offer.disk_gb).await {
                tracing::warn!(lease = %sandbox.lease_id, "checking the disk quota failed: {error:#}");
            }
        }
        Ok(())
    }

    /// Wipes sandboxes of ended leases, starts the missing ones, and reports new ones.
    async fn start_and_report(&mut self, leases: &[Lease]) -> anyhow::Result<()> {
        let mut running = self.sandboxes.list().await?;
        for sandbox in running.iter().filter(|s| !leases.iter().any(|lease| lease.id == s.lease_id)) {
            self.sandboxes.wipe(&sandbox.lease_id).await?;
            tracing::info!(lease = %sandbox.lease_id, "lease over, sandbox wiped");
        }
        running.retain(|s| leases.iter().any(|lease| lease.id == s.lease_id));

        for lease in leases {
            if !self.key_is_committed(lease).await? {
                tracing::warn!(lease = %lease.id, "no on-chain commitment from the renter to this SSH key yet");
                continue;
            }
            let (wallet, mnemonic) = self.wallets.for_lease(lease)?;
            let lease_json = self.describe(lease, wallet.wallet.address().as_ref());
            let provision = Provision { lease, wallet_mnemonic: &mnemonic, lease_json: &lease_json };
            let port = match running.iter().find(|s| s.lease_id == lease.id) {
                Some(sandbox) if !sandbox.current => {
                    self.sandboxes.upgrade(sandbox, provision, &self.offer).await?;
                    tracing::info!(lease = %lease.id, "sandbox moved to the current image");
                    sandbox.port
                }
                Some(sandbox) if sandbox.running => sandbox.port,
                Some(sandbox) => {
                    self.sandboxes.start(sandbox, &self.offer).await?;
                    tracing::info!(lease = %lease.id, "sandbox restarted");
                    sandbox.port
                }
                None => {
                    let taken: Vec<u16> = running.iter().map(|s| s.port).collect();
                    let sandbox = self.sandboxes.create(provision, &self.offer, &taken).await?;
                    tracing::info!(lease = %lease.id, port = sandbox.port, "sandbox started");
                    let port = sandbox.port;
                    running.push(sandbox);
                    port
                }
            };
            let report = SandboxReport {
                connection: self.sandboxes.connection(&self.offer, port),
                agent_wallet: wallet.wallet.address().to_string(),
            };
            let reported = lease.connection.as_ref() == Some(&report.connection)
                && lease.agent_wallet.as_deref() == Some(&report.agent_wallet);
            if !reported {
                self.market.report_sandbox(&lease.id, &report).await?;
            }
        }
        Ok(())
    }

    /// Whether the renter signed a first payment for this lease that commits to its SSH key.
    /// The market relays the key, so without this check it could slip in one of its own.
    async fn key_is_committed(&mut self, lease: &Lease) -> anyhow::Result<bool> {
        if self.verified.contains(&lease.id) {
            return Ok(true);
        }
        let expected = Memo::Activate { lease: lease.id.clone(), key: ssh_key_commitment(&lease.ssh_key) }.to_string();
        let sent = self.chain.transfers_from(&lease.renter).await?;
        let committed = sent.iter().any(|t| t.memo == expected && t.recipient == self.market_config.escrow_address);
        if committed {
            self.verified.insert(lease.id.clone());
        }
        Ok(committed)
    }

    /// What the agent finds in `~/.agentcloud/lease.json`.
    fn describe(&self, lease: &Lease, wallet: &str) -> String {
        serde_json::json!({
            "lease": lease.id,
            "market": self.market.base(),
            "chain": self.market_config.chain,
            "wallet": wallet,
            "rent_utia_per_hour": lease.price_utia_per_hour,
            "rent_paid_to": lease.payout_address,
            "grace_seconds": lease.grace_seconds,
            "status_url": format!("{}/api/leases/{}", self.market.base(), lease.id),
            "note": "Your rent is paid from this wallet every hour. Keep it funded or the sandbox is wiped after the grace period.",
        })
        .to_string()
    }
}

fn load_or_create_key(path: &Path) -> anyhow::Result<AgentKey> {
    if path.exists() {
        return AgentKey::from_hex(&std::fs::read_to_string(path)?);
    }
    let key = AgentKey::generate();
    std::fs::write(path, key.to_hex()).with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(path)?;
    tracing::info!("created agent key {}", path.display());
    Ok(key)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_: &Path) -> std::io::Result<()> {
    Ok(())
}
