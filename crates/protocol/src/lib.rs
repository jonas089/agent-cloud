//! The vocabulary every component of agentcloud shares.
//!
//! - [`api`]: the JSON the market serves and accepts.
//! - [`lease`]: how a lease's prepaid time, expiry and debt follow from its payments.
//! - [`memo`]: the on-chain memo that ties a TIA transfer to a lease.
//! - [`agent_auth`]: how a provider agent signs its requests to the market.

pub mod agent_auth;
pub mod api;
pub mod lease;
pub mod memo;

/// Seconds since the unix epoch, the only clock unit used across components.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("system clock before 1970").as_secs()
        as i64
}
