//! Every lease's agent wallet, kept in the provider's data directory: `<lease>.mnemonic` and
//! `<lease>.renter`, the address leftover funds go back to when the lease ends.
//!
//! The mnemonic is created here, handed to the sandbox so the agent can spend and earn with
//! it, and used by [`crate::rent`] to pay the agent's rent. Inside a TEE neither copy is
//! visible to the operator.

use std::path::PathBuf;

use anyhow::Context;
use chain::Wallet;
use protocol::api::Lease;

pub struct LeaseWallet {
    pub lease_id: String,
    pub renter: String,
    pub wallet: Wallet,
}

pub struct LeaseWallets {
    dir: PathBuf,
    bech32_prefix: String,
}

impl LeaseWallets {
    pub fn open(dir: PathBuf, bech32_prefix: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir, bech32_prefix: bech32_prefix.into() })
    }

    /// The lease's wallet and its mnemonic, created on first use.
    pub fn for_lease(&self, lease: &Lease) -> anyhow::Result<(LeaseWallet, String)> {
        let (wallet, _) = Wallet::load_or_create(&self.mnemonic_path(&lease.id), &self.bech32_prefix)?;
        std::fs::write(self.renter_path(&lease.id), &lease.renter)?;
        let mnemonic = std::fs::read_to_string(self.mnemonic_path(&lease.id))?;
        let wallet = LeaseWallet { lease_id: lease.id.clone(), renter: lease.renter.clone(), wallet };
        Ok((wallet, mnemonic))
    }

    /// Every wallet still on disk, including those of leases that have ended.
    pub fn all(&self) -> anyhow::Result<Vec<LeaseWallet>> {
        let mut wallets = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            let Some(lease_id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".mnemonic"))
            else {
                continue;
            };
            let (wallet, _) = Wallet::load_or_create(&path, &self.bech32_prefix)?;
            let renter = std::fs::read_to_string(self.renter_path(lease_id)).unwrap_or_default();
            wallets.push(LeaseWallet { lease_id: lease_id.to_string(), renter, wallet });
        }
        Ok(wallets)
    }

    /// Forgets a wallet once it has been emptied.
    pub fn remove(&self, lease_id: &str) -> anyhow::Result<()> {
        std::fs::remove_file(self.mnemonic_path(lease_id))?;
        let _ = std::fs::remove_file(self.renter_path(lease_id));
        Ok(())
    }

    fn mnemonic_path(&self, lease_id: &str) -> PathBuf {
        self.dir.join(format!("{lease_id}.mnemonic"))
    }

    fn renter_path(&self, lease_id: &str) -> PathBuf {
        self.dir.join(format!("{lease_id}.renter"))
    }
}
