//! Lease accounting. Rent is prepaid by the hour: every utia credited buys
//! `3600 / price_utia_per_hour` seconds counted from the first payment's block time. A lease
//! ends once its prepaid time plus the provider's grace period has passed.
//!
//! A payer tops a lease up by whole hours a few minutes before its prepaid time runs out, so
//! a lease costs one transfer an hour. Whatever accrued during an outage is owed in full and
//! paid in that same single transfer.

use crate::api::Lease;

pub const BILLING_PERIOD_SECONDS: i64 = 3600;
/// How long before the prepaid time runs out the next hour is paid.
pub const PAY_AHEAD_SECONDS: i64 = 300;

impl Lease {
    /// The moment the prepaid time runs out, once the lease has started.
    pub fn paid_until(&self) -> Option<i64> {
        let started = self.started_at?;
        let price = self.price_utia_per_hour.max(1) as u128;
        let seconds = self.paid_utia as u128 * BILLING_PERIOD_SECONDS as u128 / price;
        Some(started.saturating_add(seconds.min(i64::MAX as u128) as i64))
    }

    /// The moment the lease ends unless another payment arrives.
    pub fn expires_at(&self) -> Option<i64> {
        Some(self.paid_until()?.saturating_add(self.grace_seconds as i64))
    }

    /// Whole hours a payer should pay now. Zero while more than [`PAY_AHEAD_SECONDS`] are
    /// prepaid; otherwise every hour owed plus the next one.
    pub fn hours_due(&self, now: i64) -> u64 {
        let Some(paid_until) = self.paid_until() else { return 0 };
        let shortfall = now + PAY_AHEAD_SECONDS - paid_until;
        if shortfall <= 0 {
            return 0;
        }
        (shortfall as u64).div_ceil(BILLING_PERIOD_SECONDS as u64)
    }

    pub fn amount_due(&self, now: i64) -> u64 {
        self.hours_due(now) * self.price_utia_per_hour
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{Lease, LeaseStatus};

    const HOUR: u64 = 83_333;

    fn lease(paid_utia: u64) -> Lease {
        Lease {
            id: "l".into(),
            offer_id: "o".into(),
            renter: "r".into(),
            ssh_key: String::new(),
            status: LeaseStatus::Active,
            end_reason: None,
            created_at: 0,
            started_at: Some(1_000),
            ended_at: None,
            paid_utia,
            price_utia_per_hour: HOUR,
            grace_seconds: 600,
            payout_address: "p".into(),
            connection: None,
            agent_wallet: None,
        }
    }

    #[test]
    fn prepaid_time_follows_payments() {
        assert_eq!(lease(HOUR).paid_until(), Some(4_600));
        assert_eq!(lease(HOUR / 2).paid_until(), Some(2_799));
        assert_eq!(lease(HOUR).expires_at(), Some(5_200));
    }

    #[test]
    fn nothing_due_while_well_ahead() {
        assert_eq!(lease(HOUR).hours_due(1_000), 0);
        assert_eq!(lease(HOUR).hours_due(4_300), 0);
    }

    #[test]
    fn one_hour_due_shortly_before_running_out() {
        assert_eq!(lease(HOUR).hours_due(4_301), 1);
        assert_eq!(lease(HOUR).amount_due(4_400), HOUR);
    }

    #[test]
    fn outage_debt_is_paid_in_one_transfer() {
        // Paid until 4_600, then three hours without a payment: three owed plus the next.
        assert_eq!(lease(HOUR).hours_due(4_600 + 3 * 3600), 4);
    }
}
