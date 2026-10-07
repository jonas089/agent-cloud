//! Request and response bodies of the market's HTTP API.
//!
//! Amounts are integers in `utia` (1 TIA = 1_000_000 utia) and times are unix seconds.

use serde::{Deserialize, Serialize};

/// The chain every component talks to. The market serves its own copy at `/api/config`, so a
/// payer only needs the market's URL. Missing fields take their Mocha defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ChainConfig {
    pub chain_id: String,
    /// Cosmos SDK REST (LCD) endpoint.
    pub rest: String,
    /// CometBFT RPC endpoint, handed to wallets that need one.
    pub rpc: String,
    pub denom: String,
    pub bech32_prefix: String,
    /// Simulated gas is multiplied by this before signing, as headroom for what simulation
    /// cannot see. Gas price and gas always come from the chain itself.
    pub gas_adjustment: f64,
    pub explorer: String,
}

impl Default for ChainConfig {
    /// Celestia's Mocha testnet through its public endpoints.
    fn default() -> Self {
        Self {
            chain_id: "mocha-5".into(),
            rest: "https://api.celestia-mocha.com".into(),
            rpc: "https://rpc.celestia-mocha.com".into(),
            denom: "utia".into(),
            bech32_prefix: "celestia".into(),
            gas_adjustment: 1.1,
            explorer: "https://mocha.celenium.io".into(),
        }
    }
}

/// `GET /api/config`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketConfig {
    pub chain: ChainConfig,
    /// The market-controlled account that receives every first payment.
    pub escrow_address: String,
    /// Added to every first payment. It pays the escrow's gas for forwarding the payment to
    /// the provider, or for refunding it when the offer was taken first.
    pub escrow_fee_utia: u64,
    /// The chain's minimum gas price right now, in `denom` per unit of gas.
    pub gas_price: f64,
    /// The average fee of recent agentcloud transfers on chain, for estimates. `None` until
    /// the market has seen one.
    pub typical_fee_utia: Option<u64>,
    /// Where the source lives, for the setup instructions in the web app.
    pub repository: String,
}

/// `GET /api/status`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub chain_height: u64,
    /// Every payment up to and including this height is reflected in the leases served.
    pub indexed_height: u64,
}

/// What a provider advertises. The agent sends it on every heartbeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfferSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub region: String,
    /// Public hostname or IP renters connect to.
    pub host: String,
    /// CPU cores one sandbox may use. Slots share the machine, so this is a ceiling, not a
    /// reservation.
    pub cpus: f64,
    /// Memory ceiling of one sandbox.
    pub memory_mb: u32,
    pub disk_gb: u32,
    #[serde(default)]
    pub gpu: Option<String>,
    /// Where anyone can check the attestation of the TEE the sandboxes run in.
    #[serde(default)]
    pub attestation_url: Option<String>,
    pub price_utia_per_hour: u64,
    /// How long a lease survives without payment before it is ended and wiped.
    pub grace_seconds: u64,
    /// How many leases the provider runs at the same time.
    pub slots: u32,
    /// Where rent is paid to.
    pub payout_address: String,
}

/// `GET /api/offers`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    #[serde(flatten)]
    pub spec: OfferSpec,
    /// The provider agent sent a heartbeat recently.
    pub online: bool,
    pub free_slots: u32,
    pub last_seen: i64,
}

/// `POST /api/leases`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewLease {
    pub offer_id: String,
    /// The renter's address. Only this account can activate or cancel the lease.
    pub renter: String,
    /// OpenSSH public key installed for the renter in the sandbox.
    pub ssh_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    /// Created, waiting for its first payment.
    Pending,
    /// Paid for and running.
    Active,
    /// Over for good; the sandbox is wiped.
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// No first payment arrived in time.
    Unpaid,
    /// The offer had no free slot when the first payment landed, which was refunded.
    Outbid,
    /// Payments stopped for longer than the grace period.
    Expired,
    /// The renter cancelled on chain.
    Cancelled,
}

/// How to reach a running sandbox over SSH.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The port speaks TLS around the SSH stream, as gateways in front of TEEs do, so the
    /// client tunnels through `openssl s_client`.
    #[serde(default)]
    pub tls: bool,
}

impl Connection {
    pub fn ssh_command(&self) -> String {
        if self.tls {
            let target = format!("{}:{}", self.host, self.port);
            format!(
                "ssh -o ProxyCommand='openssl s_client -quiet -connect {target} -servername {}' {}@{}",
                self.host, self.user, self.host
            )
        } else {
            format!("ssh -p {} {}@{}", self.port, self.user, self.host)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    pub offer_id: String,
    pub renter: String,
    pub ssh_key: String,
    pub status: LeaseStatus,
    pub end_reason: Option<EndReason>,
    pub created_at: i64,
    /// Block time of the first payment.
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    /// Everything credited to the lease so far, escrow fee excluded.
    pub paid_utia: u64,
    /// Terms copied from the offer when the lease was created, so later edits to the offer
    /// never change a running deal.
    pub price_utia_per_hour: u64,
    pub grace_seconds: u64,
    pub payout_address: String,
    /// Filled in by the provider agent once the sandbox is up.
    pub connection: Option<Connection>,
    /// The agent's own account, created inside the sandbox's host. It pays the rent after the
    /// first hour; whoever wants the agent to keep running funds it.
    pub agent_wallet: Option<String>,
}

/// The transfer that activates a new lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub to: String,
    pub amount_utia: u64,
    pub memo: String,
}

/// Response to `POST /api/leases`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeaseCreated {
    pub lease: Lease,
    pub payment: PaymentRequest,
}

/// A transfer to the escrow or a provider that the market has seen on chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Payment {
    pub tx_hash: String,
    pub height: u64,
    pub time: i64,
    pub sender: String,
    pub recipient: String,
    pub amount_utia: u64,
    pub memo: String,
    pub lease_id: Option<String>,
    /// What the market did with it, in a few words.
    pub outcome: String,
    /// What the sender paid in gas for the transaction.
    pub fee_utia: u64,
}

/// The wallet a lease's agent pays its rent from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentWallet {
    pub lease_id: String,
    pub address: String,
    pub balance_utia: u64,
}

/// `GET /api/accounts/{address}`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub address: String,
    pub balance_utia: u64,
    pub leases: Vec<Lease>,
    /// Wallets of the renter's leases that are not over.
    pub agent_wallets: Vec<AgentWallet>,
    pub payments: Vec<Payment>,
    /// Rent of every active lease per hour, gas excluded.
    pub rent_utia_per_hour: u64,
    /// Gas the agents spend per hour paying that rent.
    pub gas_utia_per_hour: u64,
}

/// `PUT /api/agent/leases/{id}/sandbox`: the provider agent reports a running sandbox.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SandboxReport {
    pub connection: Connection,
    pub agent_wallet: String,
}

/// `GET /api/chain/accounts/{address}`: what a wallet needs to sign a transaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainAccount {
    pub account_number: u64,
    pub sequence: u64,
}

/// `POST /api/chain/broadcast`. Browsers cannot reach the public nodes directly (no CORS),
/// so the market relays signed transactions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Broadcast {
    /// Base64 of a signed `TxRaw`.
    pub tx_bytes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Broadcasted {
    pub tx_hash: String,
}

/// Response to `POST /api/chain/simulate`, which takes a [`Broadcast`] body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Simulated {
    pub gas_used: u64,
}

/// Body of every non-2xx response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

impl OfferSpec {
    /// Rejects offers the market could not honour or display. Addresses are checked by the
    /// market, which knows the chain's prefix.
    pub fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.name.trim().is_empty() && self.name.len() <= 64, "name must be 1 to 64 characters");
        anyhow::ensure!(self.description.len() <= 500, "description is longer than 500 characters");
        anyhow::ensure!(!self.host.trim().is_empty(), "host is empty");
        anyhow::ensure!(self.price_utia_per_hour > 0, "price must be at least 1 utia per hour");
        anyhow::ensure!(self.grace_seconds >= 120, "grace period must be at least 120 seconds");
        anyhow::ensure!(self.slots > 0, "an offer needs at least one slot");
        anyhow::ensure!(self.cpus > 0.0 && self.memory_mb > 0, "cpus and memory must be above zero");
        Ok(())
    }
}

impl NewLease {
    pub fn check(&self) -> anyhow::Result<()> {
        let key = self.ssh_key.trim();
        let known =
            ["ssh-ed25519 ", "ssh-rsa ", "ecdsa-sha2-nistp256 ", "ecdsa-sha2-nistp384 ", "ecdsa-sha2-nistp521 "];
        anyhow::ensure!(known.iter().any(|kind| key.starts_with(kind)), "not an OpenSSH public key");
        anyhow::ensure!(key.len() <= 2048 && !key.contains(['\n', '\r']), "the SSH key must be a single line");
        anyhow::ensure!(key.split_whitespace().count() >= 2, "the SSH key has no key data");
        Ok(())
    }
}

/// The renter's commitment to their SSH key, carried in the first payment's memo. The
/// renter signs it in their wallet, so whoever runs the sandbox can check on chain that the
/// key it was handed is the renter's, and the market cannot substitute one of its own.
///
/// First 16 bytes of SHA-256 over `<type> <base64>`, the key without its comment, in hex.
pub fn ssh_key_commitment(ssh_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let key: Vec<&str> = ssh_key.split_whitespace().take(2).collect();
    hex::encode(&Sha256::digest(key.join(" ").as_bytes())[..16])
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_commitment_ignores_the_comment() {
        let a = super::ssh_key_commitment("ssh-ed25519 AAAAC3Nz you@laptop");
        assert_eq!(a, super::ssh_key_commitment("ssh-ed25519 AAAAC3Nz  other"));
        assert_ne!(a, super::ssh_key_commitment("ssh-ed25519 AAAAC3Ny you@laptop"));
        assert_eq!(a, "460a4843966ebd8c680179a576c8f689", "the web app's test pins the same vector");
    }
}
