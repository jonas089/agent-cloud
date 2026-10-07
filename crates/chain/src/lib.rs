//! Just enough Celestia for agentcloud: hold a key, send TIA with a memo, and find the
//! transfers an address received. Everything goes through the Cosmos SDK REST API.

mod client;
mod wallet;

pub use client::{Chain, Head, IncludedTx, SignedTx, Transfer};
pub use wallet::Wallet;

/// cosmrs reports errors as `eyre` reports, which do not convert into `anyhow` on their own.
fn cosmrs_error(report: cosmrs::ErrorReport) -> anyhow::Error {
    anyhow::anyhow!("{report:#}")
}
