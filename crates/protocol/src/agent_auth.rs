//! Provider agents sign their requests to the market with a secp256k1 key of their own.
//!
//! The key is the provider's identity: its offer id is derived from the public key, so only
//! the agent holding the key can update the offer or report on its leases. The payout address
//! is separate and never has to live on the provider's server.
//!
//! A request carries three headers, and the signature covers method, path, timestamp and a
//! hash of the body. The market accepts timestamps within [`MAX_SKEW_SECONDS`] of its clock.

use anyhow::{anyhow, ensure, Context};
use k256::ecdsa::signature::{Signer, Verifier};
use k256::ecdsa::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

pub const KEY_HEADER: &str = "x-agent-key";
pub const TIMESTAMP_HEADER: &str = "x-agent-timestamp";
pub const SIGNATURE_HEADER: &str = "x-agent-signature";
pub const MAX_SKEW_SECONDS: i64 = 300;

pub struct AgentKey(SigningKey);

impl AgentKey {
    pub fn generate() -> Self {
        Self(SigningKey::random(&mut rand::rngs::OsRng))
    }

    pub fn from_hex(secret: &str) -> anyhow::Result<Self> {
        let bytes = hex::decode(secret.trim()).context("agent key is not hex")?;
        Ok(Self(SigningKey::from_slice(&bytes).context("invalid agent key")?))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0.to_bytes())
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.0.verifying_key().to_sec1_bytes())
    }

    pub fn offer_id(&self) -> String {
        offer_id(&self.0.verifying_key().to_sec1_bytes())
    }

    /// The three headers that authenticate a request.
    pub fn sign_request(&self, method: &str, path: &str, body: &[u8], now: i64) -> [(&'static str, String); 3] {
        let signature: Signature = self.0.sign(&signed_message(method, path, now, body));
        [
            (KEY_HEADER, self.public_hex()),
            (TIMESTAMP_HEADER, now.to_string()),
            (SIGNATURE_HEADER, hex::encode(signature.to_bytes())),
        ]
    }
}

/// Header values as received, checked by [`SignedRequest::verify`].
pub struct SignedRequest<'a> {
    pub key: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
}

impl SignedRequest<'_> {
    /// Checks the signature and returns the offer id it proves ownership of.
    pub fn verify(&self, method: &str, path: &str, body: &[u8], now: i64) -> anyhow::Result<String> {
        let timestamp: i64 = self.timestamp.parse().context("bad timestamp")?;
        ensure!((now - timestamp).abs() <= MAX_SKEW_SECONDS, "timestamp too far from the market's clock");
        let key_bytes = hex::decode(self.key).context("bad key")?;
        let key = VerifyingKey::from_sec1_bytes(&key_bytes).context("bad key")?;
        let signature =
            Signature::from_slice(&hex::decode(self.signature).context("bad signature")?).context("bad signature")?;
        key.verify(&signed_message(method, path, timestamp, body), &signature)
            .map_err(|_| anyhow!("signature does not match"))?;
        Ok(offer_id(&key_bytes))
    }
}

/// First 16 hex characters of the SHA-256 of the agent's compressed public key.
pub fn offer_id(public_key: &[u8]) -> String {
    hex::encode(Sha256::digest(public_key))[..16].to_string()
}

fn signed_message(method: &str, path: &str, timestamp: i64, body: &[u8]) -> Vec<u8> {
    format!("{method}\n{path}\n{timestamp}\n{}", hex::encode(Sha256::digest(body))).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify(headers: &[(&str, String); 3], method: &str, body: &[u8], now: i64) -> anyhow::Result<String> {
        SignedRequest { key: &headers[0].1, timestamp: &headers[1].1, signature: &headers[2].1 }.verify(
            method,
            "/api/agent/offer",
            body,
            now,
        )
    }

    #[test]
    fn signed_request_proves_the_offer() {
        let key = AgentKey::generate();
        let headers = key.sign_request("PUT", "/api/agent/offer", b"{}", 100);
        assert_eq!(verify(&headers, "PUT", b"{}", 100).unwrap(), key.offer_id());
    }

    #[test]
    fn tampering_and_stale_requests_fail() {
        let key = AgentKey::generate();
        let headers = key.sign_request("PUT", "/api/agent/offer", b"{}", 100);
        assert!(verify(&headers, "PUT", b"{\"x\":1}", 100).is_err());
        assert!(verify(&headers, "GET", b"{}", 100).is_err());
        assert!(verify(&headers, "PUT", b"{}", 100 + MAX_SKEW_SECONDS + 1).is_err());
    }

    #[test]
    fn key_round_trips_through_hex() {
        let key = AgentKey::generate();
        assert_eq!(AgentKey::from_hex(&key.to_hex()).unwrap().offer_id(), key.offer_id());
    }
}
