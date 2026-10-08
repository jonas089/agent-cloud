//! `market.toml`. Every field has a default, so an empty file runs a market on Mocha.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use protocol::api::ChainConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    /// Holds the database and the escrow key.
    pub data_dir: PathBuf,
    /// The built web app (`web/dist`), served for every path outside `/api`.
    pub web_dir: PathBuf,
    /// Added to every first payment: the escrow's charge for forwarding it to the provider or
    /// refunding it, which also pays that transfer's gas.
    pub escrow_fee_utia: u64,
    /// How often the ledger looks for new payments.
    pub poll_seconds: u64,
    /// How long a created lease waits for its first payment.
    pub pending_seconds: i64,
    /// An offer whose agent has been silent longer than this takes no new leases.
    pub offline_after_seconds: i64,
    /// Git URL of this code, shown in the setup instructions.
    pub repository: String,
    /// Offers that take no new leases and are no longer listed, e.g. an instance being replaced.
    /// Their running leases carry on until they end.
    pub retired_offers: Vec<String>,
    pub chain: ChainConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8420".parse().expect("valid address"),
            data_dir: "data/market".into(),
            web_dir: "web/dist".into(),
            escrow_fee_utia: 500,
            poll_seconds: 6,
            pending_seconds: 30 * 60,
            offline_after_seconds: 90,
            repository: String::new(),
            retired_offers: Vec::new(),
            chain: ChainConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let config: Self = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        Ok(config)
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("market.db")
    }

    pub fn escrow_key_path(&self) -> PathBuf {
        self.data_dir.join("escrow.mnemonic")
    }
}
