//! The provider's configuration: where the market is, what is offered, and how sandboxes are
//! run and reached. Read from the TOML in `AGENTCLOUD_PROVIDER_TOML` when set (how a TEE's
//! compose file passes it, so it is part of what gets measured), otherwise from a file.
//!
//! A few settings can also come from their own environment variables, which override the TOML.
//! In a TEE these values are not measured, so an operator can change them on a running, even
//! frozen, instance. Only settings that cannot weaken anyone's confidentiality are adjustable:
//! where the market is, what is charged and to whom, and how the machine is split. Each is
//! parsed as its own type and never spliced into the TOML, so no crafted value can reach the
//! security settings.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, ensure, Context};
use protocol::api::OfferSpec;
use serde::Deserialize;

pub const INLINE_ENV: &str = "AGENTCLOUD_PROVIDER_TOML";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The market's base URL, without a path.
    pub market: String,
    /// The agent key and every lease's wallet. Inside a TEE this is an encrypted volume.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    pub offer: OfferSpec,
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    /// A prebuilt sandbox image, pinned by digest. Without one the provider builds the image
    /// from the files compiled into it.
    pub image: Option<String>,
    /// Container runtime: `runc` (plain Docker) or `runsc` (gVisor) where installed.
    pub runtime: String,
    /// Host ports handed to sandboxes for SSH, inclusive. One per slot.
    pub first_port: u16,
    pub last_port: u16,
    pub pids_limit: u32,
    /// Memory each sandbox keeps under pressure. Above it, sandboxes share what is free up to
    /// `offer.memory_mb`, and the one using most is the first to go when memory runs out.
    pub memory_reserved_mb: u32,
    /// Size of each sandbox's in-memory `/tmp`, counted against its memory.
    pub tmp_mb: u32,
    /// Give each home a fixed-size filesystem of `offer.disk_gb` (an ext4 image Docker
    /// loop-mounts), so one sandbox can never fill the disk for the others.
    pub disk_quota: bool,
    /// Keep sandboxes away from the host and private networks with firewall rules. The
    /// provider refuses to start without them unless this is off.
    pub firewall: bool,
    /// How renters reach a sandbox on host port `{port}`. Empty means `offer.host` on that
    /// port. Behind a TEE gateway it is a per-port hostname such as
    /// `<app-id>-{port}.<gateway-domain>`.
    pub connect_host: String,
    /// The port renters dial; 0 means the sandbox's own host port.
    pub connect_port: u16,
    /// Whether that endpoint wraps the SSH stream in TLS.
    pub connect_tls: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            image: None,
            runtime: "runc".into(),
            first_port: 2200,
            last_port: 2299,
            pids_limit: 512,
            memory_reserved_mb: 128,
            tmp_mb: 128,
            disk_quota: true,
            firewall: true,
            connect_host: String::new(),
            connect_port: 0,
            connect_tls: false,
        }
    }
}

fn default_data_dir() -> PathBuf {
    "data/provider".into()
}

fn default_poll_seconds() -> u64 {
    10
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = match std::env::var(INLINE_ENV) {
            Ok(inline) => inline,
            Err(_) => std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
        };
        let mut config: Self = toml::from_str(&text).context("parsing the provider config")?;
        config.apply_adjustable()?;
        config.offer.check()?;
        ensure!(config.sandbox.first_port <= config.sandbox.last_port, "sandbox port range is empty");
        let ports = (config.sandbox.last_port - config.sandbox.first_port) as u32 + 1;
        ensure!(ports >= config.offer.slots, "the sandbox port range has fewer ports than the offer has slots");
        Ok(config)
    }

    /// Overrides from `AGENTCLOUD_<NAME>` variables; see the module docs for why these.
    fn apply_adjustable(&mut self) -> anyhow::Result<()> {
        let offer = &mut self.offer;
        set(&mut self.market, "MARKET_URL")?;
        set(&mut offer.name, "OFFER_NAME")?;
        set(&mut offer.payout_address, "PAYOUT_ADDRESS")?;
        set(&mut offer.price_utia_per_hour, "PRICE_UTIA_PER_HOUR")?;
        set(&mut offer.slots, "SLOTS")?;
        set(&mut offer.cpus, "CPUS")?;
        set(&mut offer.memory_mb, "MEMORY_MB")?;
        set(&mut offer.disk_gb, "DISK_GB")?;
        set(&mut offer.grace_seconds, "GRACE_SECONDS")
    }
}

/// Replaces `field` with `AGENTCLOUD_<name>` when that is set and not empty.
fn set<T: FromStr>(field: &mut T, name: &str) -> anyhow::Result<()>
where
    T::Err: Display,
{
    let var = format!("AGENTCLOUD_{name}");
    match std::env::var(&var) {
        Ok(value) if !value.trim().is_empty() => {
            *field = value.trim().parse().map_err(|error| anyhow!("{var}: {error}"))?;
            Ok(())
        }
        _ => Ok(()),
    }
}
