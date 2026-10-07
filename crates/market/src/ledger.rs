//! Follows the chain and turns transfers into lease state.
//!
//! The ledger watches the escrow and every provider's payout address. A transfer only counts
//! when its memo names a lease (see [`protocol::memo`]):
//!
//! - **First payment** (to the escrow): activates a pending lease if the renter sent it, it
//!   commits to the lease's SSH key, it covers one hour plus the escrow fee, and the offer
//!   still has a free slot. The escrow then forwards it to the provider. Anything else is
//!   refunded minus the fee, which is how two renters racing for the last slot are settled:
//!   the first in block order wins.
//! - **Rent** (to the provider): extends an active lease, whoever sent it. Normally that is
//!   the lease's own agent wallet.
//! - **Cancel** (from the renter): ends the lease at once.
//!
//! Leases expire against the time of the newest block the ledger has fully read, never the
//! wall clock, so a slow or unreachable node can delay an expiry but never cause a wrong one.

use std::sync::Arc;
use std::time::Duration;

use chain::Transfer;
use protocol::api::{ssh_key_commitment, EndReason, LeaseStatus, Payment};
use protocol::memo::Memo;

use crate::store::Db;
use crate::Market;

pub async fn run(market: Arc<Market>) {
    let interval = Duration::from_secs(market.config.poll_seconds);
    loop {
        if let Err(error) = sync(&market).await {
            tracing::warn!("ledger sync failed: {error:#}");
        }
        if let Err(error) = expire(&market) {
            tracing::warn!("expiring leases failed: {error:#}");
        }
        tokio::time::sleep(interval).await;
    }
}

/// Reads every watched address up to the chain's head, then marks the head as synced.
async fn sync(market: &Market) -> anyhow::Result<()> {
    let head = market.chain.head().await?;
    for address in watched_addresses(market)? {
        // A newly watched address starts at the head: there is nothing older to find.
        let from = market.store.read(|db| db.cursor(&address))?.unwrap_or(head.height);
        if from > head.height {
            continue;
        }
        let transfers = market.chain.transfers_to(&address, from, head.height).await?;
        market.store.write(|db| {
            for transfer in &transfers {
                apply(market, db, transfer)?;
            }
            db.set_cursor(&address, head.height + 1)
        })?;
    }
    *market.synced.write().expect("synced lock poisoned") = Some(head);
    Ok(())
}

/// The escrow first, so a lease is activated before rent for it is looked at.
fn watched_addresses(market: &Market) -> anyhow::Result<Vec<String>> {
    let recent = protocol::unix_now() - 7 * 24 * 3600;
    let (offers, active) =
        market.store.read(|db| Ok((db.offers_seen_since(recent)?, db.leases_with_status(LeaseStatus::Active)?)))?;
    let mut addresses = vec![market.escrow_address.clone()];
    let payouts = offers.into_iter().map(|o| o.spec.payout_address).chain(active.into_iter().map(|l| l.payout_address));
    for address in payouts {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    Ok(addresses)
}

fn apply(market: &Market, db: &Db, transfer: &Transfer) -> anyhow::Result<()> {
    if db.has_payment(&transfer.tx_hash, transfer.msg_index)? {
        return Ok(());
    }
    let memo = transfer.memo.parse::<Memo>().ok();
    let outcome = match &memo {
        None => "not an agentcloud payment".to_string(),
        Some(Memo::Activate { lease, key }) if transfer.recipient == market.escrow_address => {
            activate(market, db, transfer, lease, key)?
        }
        Some(Memo::Activate { .. }) => "ignored: first payments go to the escrow".to_string(),
        Some(Memo::Pay(id)) if transfer.recipient == market.escrow_address => {
            refund(market, db, transfer, id, "rent is paid to the provider, not the escrow")?
        }
        Some(Memo::Pay(id)) => credit(db, transfer, id)?,
        Some(Memo::Cancel(id)) => cancel(db, transfer, id)?,
        Some(Memo::Forward(_) | Memo::Refund(_)) => "escrow payout".to_string(),
    };
    tracing::info!(tx = %transfer.tx_hash, memo = %transfer.memo, amount = transfer.amount, "{outcome}");
    let payment = Payment {
        tx_hash: transfer.tx_hash.clone(),
        height: transfer.height,
        time: transfer.time,
        sender: transfer.sender.clone(),
        recipient: transfer.recipient.clone(),
        amount_utia: transfer.amount,
        memo: transfer.memo.clone(),
        lease_id: memo.map(|memo| memo.lease_id().to_string()),
        outcome,
        fee_utia: transfer.fee,
    };
    db.insert_payment(&payment, transfer.msg_index)
}

fn activate(market: &Market, db: &Db, transfer: &Transfer, id: &str, key: &str) -> anyhow::Result<String> {
    let Some(lease) = db.lease(id)? else {
        return refund(market, db, transfer, id, "unknown lease");
    };
    if lease.status != LeaseStatus::Pending {
        return refund(market, db, transfer, id, "the lease is not waiting for a first payment");
    }
    if transfer.sender != lease.renter {
        return refund(market, db, transfer, id, "only the renter can activate a lease");
    }
    if key != ssh_key_commitment(&lease.ssh_key) {
        return refund(market, db, transfer, id, "the memo commits to a different SSH key");
    }
    if transfer.amount < lease.price_utia_per_hour + market.config.escrow_fee_utia {
        return refund(market, db, transfer, id, "less than the first payment");
    }
    let slots = db.offer(&lease.offer_id)?.map_or(0, |offer| offer.spec.slots as usize);
    if db.leases_of_offer(&lease.offer_id, LeaseStatus::Active)?.len() >= slots {
        db.end_lease(id, EndReason::Outbid, transfer.time)?;
        return refund(market, db, transfer, id, "someone else rented the last slot first");
    }
    let credited = transfer.amount - market.config.escrow_fee_utia;
    db.activate_lease(id, transfer.time, credited)?;
    db.queue_payout(&cause(transfer), &lease.payout_address, credited, &Memo::Forward(id.into()).to_string())?;
    Ok("activated the lease".into())
}

/// Sends a first payment back, minus the escrow fee that pays for sending it.
fn refund(market: &Market, db: &Db, transfer: &Transfer, id: &str, reason: &str) -> anyhow::Result<String> {
    let amount = transfer.amount.saturating_sub(market.config.escrow_fee_utia);
    if amount == 0 {
        return Ok(format!("kept as fee: {reason}"));
    }
    db.queue_payout(&cause(transfer), &transfer.sender, amount, &Memo::Refund(id.into()).to_string())?;
    Ok(format!("refunded: {reason}"))
}

fn credit(db: &Db, transfer: &Transfer, id: &str) -> anyhow::Result<String> {
    Ok(match db.lease(id)? {
        Some(lease) if lease.status == LeaseStatus::Active && transfer.recipient == lease.payout_address => {
            db.credit_lease(id, transfer.amount)?;
            "credited".into()
        }
        Some(lease) if lease.status == LeaseStatus::Active => "not credited: sent to another address".into(),
        Some(_) => "not credited: the lease is not active".into(),
        None => "not credited: unknown lease".into(),
    })
}

fn cancel(db: &Db, transfer: &Transfer, id: &str) -> anyhow::Result<String> {
    Ok(match db.lease(id)? {
        Some(lease) if transfer.sender != lease.renter => "ignored: only the renter can cancel".into(),
        Some(lease) if lease.status == LeaseStatus::Ended => "ignored: already ended".into(),
        Some(_) => {
            db.end_lease(id, EndReason::Cancelled, transfer.time)?;
            "cancelled the lease".into()
        }
        None => "ignored: unknown lease".into(),
    })
}

fn expire(market: &Market) -> anyhow::Result<()> {
    let Some(head) = *market.synced.read().expect("synced lock poisoned") else { return Ok(()) };
    market.store.write(|db| {
        for lease in db.leases_with_status(LeaseStatus::Active)? {
            if let Some(expired_at) = lease.expires_at().filter(|at| *at < head.time) {
                tracing::info!(lease = %lease.id, "expired");
                db.end_lease(&lease.id, EndReason::Expired, expired_at)?;
            }
        }
        for lease in db.leases_with_status(LeaseStatus::Pending)? {
            if lease.created_at + market.config.pending_seconds < head.time {
                db.end_lease(&lease.id, EndReason::Unpaid, head.time)?;
            }
        }
        Ok(())
    })
}

/// Identifies the transfer a payout answers, so it is queued at most once.
fn cause(transfer: &Transfer) -> String {
    format!("{}:{}", transfer.tx_hash, transfer.msg_index)
}
