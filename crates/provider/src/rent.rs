//! Agents pay their own rent. Every active lease's wallet pays the provider an hour at a
//! time, shortly before the prepaid hour runs out, and settles any debt from an outage in the
//! same single transfer.
//! When a lease ends, whatever its wallet still holds goes back to the renter.
//!
//! Debt is never counted locally: the market reports how far each lease is paid. A wallet's
//! next payment is only computed once its previous one is in a block the market has read, so
//! nothing is paid twice. A transfer that does not show up is settled by the account's
//! sequence: if it moved, the transfer landed; if not, it was dropped and the debt is
//! computed afresh.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chain::Chain;
use protocol::api::Lease;
use protocol::memo::Memo;
use protocol::unix_now;

use crate::market::MarketClient;
use crate::wallets::{LeaseWallet, LeaseWallets};

/// How long a broadcast transfer may stay out of a block before its sequence is checked.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(90);

struct InFlight {
    hash: String,
    address: String,
    sequence: u64,
    sent: Instant,
}

pub struct Rent {
    chain: Chain,
    /// Per lease, the transfer waiting to land.
    in_flight: HashMap<String, InFlight>,
    /// Per lease, the block the market must have read before the next payment is computed.
    settled_at: HashMap<String, u64>,
}

impl Rent {
    pub fn new(chain: Chain) -> Self {
        Self { chain, in_flight: HashMap::new(), settled_at: HashMap::new() }
    }

    /// Pays what each active lease owes, from its own wallet.
    pub async fn pay(&mut self, market: &MarketClient, leases: &[Lease], wallets: &LeaseWallets) -> anyhow::Result<()> {
        self.track_in_flight().await?;
        let indexed = market.status().await?.indexed_height;
        let now = unix_now();
        for lease in leases {
            let waiting = self.in_flight.contains_key(&lease.id)
                || self.settled_at.get(&lease.id).is_some_and(|height| *height > indexed);
            let amount = lease.amount_due(now);
            if waiting || amount == 0 {
                continue;
            }
            let (wallet, _) = wallets.for_lease(lease)?;
            let balance = self.chain.balance(wallet.wallet.address().as_ref()).await?;
            // A first send from a fresh wallet costs extra gas; leave room for it.
            let fee = self.chain.config().fee_for_gas(self.chain.config().gas_for_send(0, true));
            if balance < amount + fee {
                tracing::debug!(lease = %lease.id, balance, amount, "agent wallet cannot cover its rent");
                continue;
            }
            let memo = Memo::Pay(lease.id.clone()).to_string();
            self.send(&wallet, &lease.payout_address, amount, &memo).await?;
            tracing::info!(lease = %lease.id, amount, "rent sent");
        }
        Ok(())
    }

    /// Returns the balance of every wallet whose lease is over to its renter, then forgets the
    /// wallet once nothing worth sending is left.
    pub async fn refund_ended(&mut self, active: &[Lease], wallets: &LeaseWallets) -> anyhow::Result<()> {
        let fee = self.chain.config().send_fee();
        for wallet in wallets.all()? {
            if active.iter().any(|lease| lease.id == wallet.lease_id) || self.in_flight.contains_key(&wallet.lease_id) {
                continue;
            }
            let balance = self.chain.balance(wallet.wallet.address().as_ref()).await?;
            if balance <= fee || wallet.renter.is_empty() {
                wallets.remove(&wallet.lease_id)?;
                tracing::info!(lease = %wallet.lease_id, "agent wallet emptied and removed");
                continue;
            }
            let memo = Memo::Refund(wallet.lease_id.clone()).to_string();
            self.send(&wallet, &wallet.renter, balance - fee, &memo).await?;
            tracing::info!(lease = %wallet.lease_id, amount = balance - fee, "leftover funds returned to the renter");
        }
        Ok(())
    }

    async fn send(&mut self, wallet: &LeaseWallet, to: &str, amount: u64, memo: &str) -> anyhow::Result<()> {
        let signed = self.chain.sign_send(&wallet.wallet, to, amount, memo).await?;
        self.chain.broadcast(&signed.bytes).await?;
        let in_flight = InFlight {
            hash: signed.hash,
            address: wallet.wallet.address().to_string(),
            sequence: signed.sequence,
            sent: Instant::now(),
        };
        self.in_flight.insert(wallet.lease_id.clone(), in_flight);
        Ok(())
    }

    /// Moves transfers that landed (or provably did not) out of flight.
    async fn track_in_flight(&mut self) -> anyhow::Result<()> {
        let leases: Vec<String> = self.in_flight.keys().cloned().collect();
        for lease in leases {
            let tx = &self.in_flight[&lease];
            if let Some(included) = self.chain.tx(&tx.hash).await? {
                if !included.succeeded {
                    tracing::warn!(lease = %lease, tx = %tx.hash, "transfer failed on chain");
                }
                self.settled_at.insert(lease.clone(), included.height);
                self.in_flight.remove(&lease);
            } else if tx.sent.elapsed() > CONFIRM_TIMEOUT {
                let account = self.chain.account(&tx.address).await?;
                if account.is_some_and(|account| account.sequence > tx.sequence) {
                    // The sequence was used, so something landed: wait for the market to pass
                    // the head before computing anything new.
                    self.settled_at.insert(lease.clone(), self.chain.head().await?.height);
                }
                self.in_flight.remove(&lease);
            }
        }
        Ok(())
    }
}
