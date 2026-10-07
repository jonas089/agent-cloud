//! Sends what the escrow owes: first payments forwarded to providers and refunds.
//!
//! Payouts are settled one at a time, oldest first, and never paid twice:
//!
//! 1. Estimate the fee from the chain, sign for the escrow's current account sequence, record
//!    the signed transaction, then broadcast it.
//! 2. On the next pass, look the hash up. Found: settled.
//! 3. Not found and the sequence has not moved: nothing landed, so broadcast the same recorded
//!    bytes again. Only that transaction can ever use the sequence.
//! 4. Not found but the sequence moved on: another transaction used it (the key is shared
//!    with something else), so the payout goes back to the queue.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chain::Wallet;

use crate::store::Payout;
use crate::Market;

/// How long a broadcast payout may take to appear in a block before it is re-checked.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);
const IDLE: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_secs(20);

pub async fn run(market: Arc<Market>) {
    loop {
        let pause = match market.store.read(|db| db.next_payout()) {
            Ok(Some(payout)) => match settle(&market, &market.escrow, payout).await {
                Ok(()) => Duration::ZERO,
                Err(error) => {
                    tracing::warn!("escrow payout failed, retrying: {error:#}");
                    RETRY
                }
            },
            Ok(None) => IDLE,
            Err(error) => {
                tracing::warn!("reading payouts failed: {error:#}");
                RETRY
            }
        };
        tokio::time::sleep(pause).await;
    }
}

async fn settle(market: &Market, wallet: &Wallet, payout: Payout) -> anyhow::Result<()> {
    if let (Some(hash), Some(sequence), Some(bytes)) = (&payout.tx_hash, payout.sequence, &payout.tx_bytes) {
        if let Some(included) = market.chain.wait_for(hash, CONFIRM_TIMEOUT).await? {
            let error = (!included.succeeded).then_some("the payout transaction failed on chain");
            tracing::info!(payout = payout.id, tx = %hash, to = %payout.recipient, amount = payout.amount_utia, "payout settled");
            return market.store.write(|db| db.payout_settled(payout.id, error));
        }
        let address = wallet.address().to_string();
        let account = market.chain.account(&address).await?.context("escrow account not found")?;
        if account.sequence > sequence {
            tracing::warn!(payout = payout.id, "escrow sequence was used by another transaction, requeueing");
            return market.store.write(|db| db.payout_requeued(payout.id));
        }
        return rebroadcast(market, payout.id, bytes).await;
    }
    let fee = market.chain.estimate_send(wallet, &payout.recipient, payout.amount_utia, &payout.memo).await?;
    let signed = market.chain.sign_send(wallet, &payout.recipient, payout.amount_utia, &payout.memo, fee).await?;
    market.store.write(|db| db.payout_signed(payout.id, &signed.hash, signed.sequence, &signed.bytes))?;
    rebroadcast(market, payout.id, &signed.bytes).await
}

/// It may already be in a mempool; the next pass finds out either way.
async fn rebroadcast(market: &Market, payout: i64, bytes: &[u8]) -> anyhow::Result<()> {
    if let Err(error) = market.chain.broadcast(bytes).await {
        tracing::warn!(payout, "broadcast failed: {error:#}");
    }
    Ok(())
}
