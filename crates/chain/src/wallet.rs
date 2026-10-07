//! A secp256k1 account derived from a BIP-39 mnemonic on the Cosmos path, the same account
//! Keplr shows for that mnemonic.

use std::path::Path;

use anyhow::Context;
use bip32::{DerivationPath, XPrv};
use bip39::Mnemonic;
use cosmrs::crypto::secp256k1::SigningKey;
use cosmrs::AccountId;

use crate::cosmrs_error;

const DERIVATION_PATH: &str = "m/44'/118'/0'/0/0";

pub struct Wallet {
    key: SigningKey,
    address: AccountId,
}

impl Wallet {
    pub fn from_mnemonic(phrase: &str, bech32_prefix: &str) -> anyhow::Result<Self> {
        let mnemonic = Mnemonic::parse_normalized(phrase.trim()).context("invalid mnemonic")?;
        let path: DerivationPath = DERIVATION_PATH.parse()?;
        let xprv = XPrv::derive_from_path(mnemonic.to_seed(""), &path)?;
        let key = SigningKey::from_slice(&xprv.private_key().to_bytes()).map_err(cosmrs_error)?;
        let address = key.public_key().account_id(bech32_prefix).map_err(cosmrs_error)?;
        Ok(Self { key, address })
    }

    /// Reads the mnemonic in `path`, or creates the file with a fresh one (mode 0600) when it
    /// does not exist yet. Returns the wallet and whether it was just created.
    pub fn load_or_create(path: &Path, bech32_prefix: &str) -> anyhow::Result<(Self, bool)> {
        if path.exists() {
            let phrase = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            return Ok((Self::from_mnemonic(&phrase, bech32_prefix)?, false));
        }
        let phrase = Mnemonic::generate(24)?.to_string();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(path, &format!("{phrase}\n")).with_context(|| format!("writing {}", path.display()))?;
        Ok((Self::from_mnemonic(&phrase, bech32_prefix)?, true))
    }

    pub fn address(&self) -> &AccountId {
        &self.address
    }

    pub(crate) fn key(&self) -> &SigningKey {
        &self.key
    }
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::Wallet;

    #[test]
    fn derives_the_keplr_address() {
        // The well-known all-"abandon" test vector.
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let wallet = Wallet::from_mnemonic(phrase, "cosmos").unwrap();
        assert_eq!(wallet.address().to_string(), "cosmos19rl4cm2hmr8afy4kldpxz3fka4jguq0auqdal4");
    }
}
