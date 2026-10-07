//! The memo of a TIA transfer says which lease it belongs to and why it was sent.
//!
//! `agentcloud:activate:<lease>:<key>`  renter to escrow: the first payment, committing to the
//!                                    renter's SSH key (see [`crate::api::ssh_key_commitment`])
//! `agentcloud:pay:<lease>`             agent wallet (or anyone) to provider: rent
//! `agentcloud:cancel:<lease>`          renter to provider: end the lease now
//! `agentcloud:forward:<lease>`         escrow to provider: the first payment, passed on
//! `agentcloud:refund:<lease>`          escrow or agent wallet to renter: money given back

use std::fmt;
use std::str::FromStr;

const PREFIX: &str = "agentcloud";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Memo {
    Activate { lease: String, key: String },
    Pay(String),
    Cancel(String),
    Forward(String),
    Refund(String),
}

impl Memo {
    pub fn lease_id(&self) -> &str {
        match self {
            Memo::Activate { lease, .. } => lease,
            Memo::Pay(id) | Memo::Cancel(id) | Memo::Forward(id) | Memo::Refund(id) => id,
        }
    }
}

impl fmt::Display for Memo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Memo::Activate { lease, key } => write!(f, "{PREFIX}:activate:{lease}:{key}"),
            Memo::Pay(id) => write!(f, "{PREFIX}:pay:{id}"),
            Memo::Cancel(id) => write!(f, "{PREFIX}:cancel:{id}"),
            Memo::Forward(id) => write!(f, "{PREFIX}:forward:{id}"),
            Memo::Refund(id) => write!(f, "{PREFIX}:refund:{id}"),
        }
    }
}

impl FromStr for Memo {
    type Err = anyhow::Error;

    fn from_str(memo: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = memo.trim().split(':').collect();
        let [prefix, kind, id, rest @ ..] = parts.as_slice() else { anyhow::bail!("not an agentcloud memo") };
        anyhow::ensure!(*prefix == PREFIX, "not an agentcloud memo");
        anyhow::ensure!(is_lease_id(id), "malformed lease id");
        let id = id.to_string();
        Ok(match (*kind, rest) {
            ("activate", [key]) if is_commitment(key) => Memo::Activate { lease: id, key: key.to_string() },
            ("pay", []) => Memo::Pay(id),
            ("cancel", []) => Memo::Cancel(id),
            ("forward", []) => Memo::Forward(id),
            ("refund", []) => Memo::Refund(id),
            _ => anyhow::bail!("unknown memo {memo}"),
        })
    }
}

/// Lease ids are short lowercase alphanumerics, so a memo always fits and never needs escaping.
pub fn is_lease_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn is_commitment(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::Memo;

    #[test]
    fn round_trips() {
        let activate = Memo::Activate { lease: "ab12".into(), key: "0123456789abcdef0123456789abcdef".into() };
        for memo in [activate, Memo::Pay("ab12".into()), Memo::Cancel("x".into()), Memo::Refund("9".into())] {
            assert_eq!(memo.to_string().parse::<Memo>().unwrap(), memo);
        }
        assert_eq!(Memo::Pay("ab12".into()).to_string(), "agentcloud:pay:ab12");
    }

    #[test]
    fn rejects_foreign_memos() {
        let memos = [
            "",
            "hello",
            "agentcloud:pay:",
            "agentcloud:steal:ab",
            "agentcloud:pay:AB",
            "other:pay:ab",
            "agentcloud:pay:ab:extra",
            "agentcloud:activate:ab",
            "agentcloud:activate:ab:nothex",
        ];
        for memo in memos {
            assert!(memo.parse::<Memo>().is_err(), "{memo}");
        }
    }
}
