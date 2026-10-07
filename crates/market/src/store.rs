//! The market's only state: a small SQLite database.
//!
//! Every read and write goes through [`Store::read`] or [`Store::write`], which hand out a
//! [`Db`] with typed queries. A write runs in one transaction, so applying a payment, updating
//! its lease and queueing the escrow payout it causes either all happen or none do.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use protocol::api::{EndReason, Lease, LeaseStatus, OfferSpec, Payment, SandboxReport};
use rusqlite::{params, OptionalExtension, Row};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS offers (
    id         TEXT PRIMARY KEY,
    spec       TEXT NOT NULL,
    last_seen  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS leases (
    id                    TEXT PRIMARY KEY,
    offer_id              TEXT NOT NULL,
    renter                TEXT NOT NULL,
    ssh_key               TEXT NOT NULL,
    status                TEXT NOT NULL,
    end_reason            TEXT,
    created_at            INTEGER NOT NULL,
    started_at            INTEGER,
    ended_at              INTEGER,
    paid_utia             INTEGER NOT NULL,
    price_utia_per_hour INTEGER NOT NULL,
    grace_seconds         INTEGER NOT NULL,
    payout_address        TEXT NOT NULL,
    connection            TEXT,
    agent_wallet          TEXT
);
CREATE INDEX IF NOT EXISTS leases_by_renter ON leases (renter);
CREATE INDEX IF NOT EXISTS leases_by_offer ON leases (offer_id, status);
CREATE TABLE IF NOT EXISTS payments (
    tx_hash     TEXT NOT NULL,
    msg_index   INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    time        INTEGER NOT NULL,
    sender      TEXT NOT NULL,
    recipient   TEXT NOT NULL,
    amount_utia INTEGER NOT NULL,
    memo        TEXT NOT NULL,
    lease_id    TEXT,
    outcome     TEXT NOT NULL,
    fee_utia    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tx_hash, msg_index)
);
CREATE INDEX IF NOT EXISTS payments_by_lease ON payments (lease_id);
CREATE TABLE IF NOT EXISTS payouts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    cause       TEXT NOT NULL UNIQUE,
    recipient   TEXT NOT NULL,
    amount_utia INTEGER NOT NULL,
    memo        TEXT NOT NULL,
    status      TEXT NOT NULL,
    tx_hash     TEXT,
    sequence    INTEGER,
    tx_bytes    BLOB,
    error       TEXT
);
CREATE TABLE IF NOT EXISTS cursors (
    address     TEXT PRIMARY KEY,
    next_height INTEGER NOT NULL
);
";

pub struct Store(Mutex<rusqlite::Connection>);

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let connection = rusqlite::Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(SCHEMA)?;
        add_missing_columns(&connection)?;
        Ok(Self(Mutex::new(connection)))
    }

    pub fn read<T>(&self, f: impl FnOnce(&Db) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let connection = self.0.lock().expect("store lock poisoned");
        f(&Db(&connection))
    }

    pub fn write<T>(&self, f: impl FnOnce(&Db) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut connection = self.0.lock().expect("store lock poisoned");
        let transaction = connection.transaction()?;
        let result = f(&Db(&transaction))?;
        transaction.commit()?;
        Ok(result)
    }
}

pub struct Db<'c>(&'c rusqlite::Connection);

/// An offer as stored, before the market adds liveness and free slots.
pub struct StoredOffer {
    pub id: String,
    pub spec: OfferSpec,
    pub last_seen: i64,
}

/// A transfer the escrow owes, created by a payment and settled by the escrow worker.
pub struct Payout {
    pub id: i64,
    pub recipient: String,
    pub amount_utia: u64,
    pub memo: String,
    /// Set once signed; the transaction may or may not have landed.
    pub tx_hash: Option<String>,
    pub sequence: Option<u64>,
    /// The signed transaction, rebroadcast as is until it lands or provably cannot.
    pub tx_bytes: Option<Vec<u8>>,
}

const LEASE_COLUMNS: &str = "id, offer_id, renter, ssh_key, status, end_reason, created_at, started_at, ended_at, \
     paid_utia, price_utia_per_hour, grace_seconds, payout_address, connection, agent_wallet";

impl Db<'_> {
    // ---------------------------------------------------------------- offers

    pub fn upsert_offer(&self, id: &str, spec: &OfferSpec, now: i64) -> anyhow::Result<()> {
        self.0.execute(
            "INSERT INTO offers (id, spec, last_seen) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET spec = excluded.spec, last_seen = excluded.last_seen",
            params![id, serde_json::to_string(spec)?, now],
        )?;
        Ok(())
    }

    pub fn offer(&self, id: &str) -> anyhow::Result<Option<StoredOffer>> {
        self.0
            .query_row("SELECT id, spec, last_seen FROM offers WHERE id = ?1", [id], offer_from_row)
            .optional()?
            .transpose()
    }

    pub fn offers_seen_since(&self, since: i64) -> anyhow::Result<Vec<StoredOffer>> {
        let mut statement =
            self.0.prepare("SELECT id, spec, last_seen FROM offers WHERE last_seen >= ?1 ORDER BY last_seen DESC")?;
        let rows = statement.query_map([since], offer_from_row)?;
        rows.map(|row| row?).collect()
    }

    // ---------------------------------------------------------------- leases

    pub fn insert_lease(&self, lease: &Lease) -> anyhow::Result<()> {
        self.0.execute(
            &format!("INSERT INTO leases ({LEASE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"),
            params![
                lease.id,
                lease.offer_id,
                lease.renter,
                lease.ssh_key,
                enum_text(&lease.status)?,
                lease.end_reason.as_ref().map(enum_text).transpose()?,
                lease.created_at,
                lease.started_at,
                lease.ended_at,
                lease.paid_utia as i64,
                lease.price_utia_per_hour as i64,
                lease.grace_seconds as i64,
                lease.payout_address,
                lease.connection.as_ref().map(serde_json::to_string).transpose()?,
                lease.agent_wallet,
            ],
        )?;
        Ok(())
    }

    pub fn lease(&self, id: &str) -> anyhow::Result<Option<Lease>> {
        self.leases_where("id = ?1", [id]).map(|mut leases| leases.pop())
    }

    pub fn leases_of_renter(&self, renter: &str) -> anyhow::Result<Vec<Lease>> {
        self.leases_where("renter = ?1 ORDER BY created_at DESC", [renter])
    }

    pub fn leases_with_status(&self, status: LeaseStatus) -> anyhow::Result<Vec<Lease>> {
        self.leases_where("status = ?1", [enum_text(&status)?])
    }

    pub fn leases_of_offer(&self, offer_id: &str, status: LeaseStatus) -> anyhow::Result<Vec<Lease>> {
        self.leases_where("offer_id = ?1 AND status = ?2", [offer_id.to_string(), enum_text(&status)?])
    }

    pub fn activate_lease(&self, id: &str, started_at: i64, paid_utia: u64) -> anyhow::Result<()> {
        self.0.execute(
            "UPDATE leases SET status = 'active', started_at = ?2, paid_utia = ?3 WHERE id = ?1",
            params![id, started_at, paid_utia as i64],
        )?;
        Ok(())
    }

    pub fn credit_lease(&self, id: &str, amount_utia: u64) -> anyhow::Result<()> {
        self.0
            .execute("UPDATE leases SET paid_utia = paid_utia + ?2 WHERE id = ?1", params![id, amount_utia as i64])?;
        Ok(())
    }

    pub fn end_lease(&self, id: &str, reason: EndReason, at: i64) -> anyhow::Result<()> {
        self.0.execute(
            "UPDATE leases SET status = 'ended', end_reason = ?2, ended_at = ?3 WHERE id = ?1",
            params![id, enum_text(&reason)?, at],
        )?;
        Ok(())
    }

    pub fn set_sandbox(&self, id: &str, report: &SandboxReport) -> anyhow::Result<()> {
        self.0.execute(
            "UPDATE leases SET connection = ?2, agent_wallet = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(&report.connection)?, report.agent_wallet],
        )?;
        Ok(())
    }

    fn leases_where<P: rusqlite::Params>(&self, condition: &str, params: P) -> anyhow::Result<Vec<Lease>> {
        let mut statement = self.0.prepare(&format!("SELECT {LEASE_COLUMNS} FROM leases WHERE {condition}"))?;
        let rows = statement.query_map(params, lease_from_row)?;
        rows.map(|row| row?).collect()
    }

    // ---------------------------------------------------------------- payments

    pub fn has_payment(&self, tx_hash: &str, msg_index: u32) -> anyhow::Result<bool> {
        let found = self
            .0
            .query_row(
                "SELECT 1 FROM payments WHERE tx_hash = ?1 AND msg_index = ?2",
                params![tx_hash, msg_index],
                |_| Ok(()),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn insert_payment(&self, payment: &Payment, msg_index: u32) -> anyhow::Result<()> {
        self.0.execute(
            "INSERT INTO payments (tx_hash, msg_index, height, time, sender, recipient, amount_utia, memo, lease_id, outcome, fee_utia)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                payment.tx_hash,
                msg_index,
                payment.height as i64,
                payment.time,
                payment.sender,
                payment.recipient,
                payment.amount_utia as i64,
                payment.memo,
                payment.lease_id,
                payment.outcome,
                payment.fee_utia as i64,
            ],
        )?;
        Ok(())
    }

    /// Payments for the renter's leases or sent by the renter, newest first.
    pub fn payments_of_renter(&self, renter: &str, limit: u32) -> anyhow::Result<Vec<Payment>> {
        let mut statement = self.0.prepare(
            "SELECT tx_hash, height, time, sender, recipient, amount_utia, memo, lease_id, outcome, fee_utia FROM payments
             WHERE sender = ?1 OR lease_id IN (SELECT id FROM leases WHERE renter = ?1)
             ORDER BY height DESC, msg_index DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![renter, limit], |row| {
            Ok(Payment {
                tx_hash: row.get(0)?,
                height: row.get::<_, i64>(1)? as u64,
                time: row.get(2)?,
                sender: row.get(3)?,
                recipient: row.get(4)?,
                amount_utia: row.get::<_, i64>(5)? as u64,
                memo: row.get(6)?,
                lease_id: row.get(7)?,
                outcome: row.get(8)?,
                fee_utia: row.get::<_, i64>(9)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The average fee of the latest agentcloud transfers, a live estimate of what one costs.
    pub fn typical_fee(&self) -> anyhow::Result<Option<u64>> {
        let average: Option<f64> = self.0.query_row(
            "SELECT AVG(fee_utia) FROM (SELECT fee_utia FROM payments
             WHERE fee_utia > 0 AND memo LIKE 'agentcloud:%' ORDER BY height DESC LIMIT 50)",
            [],
            |row| row.get(0),
        )?;
        Ok(average.map(|fee| fee.round() as u64))
    }

    // ---------------------------------------------------------------- escrow payouts

    /// Queues a payout unless one for the same cause already exists.
    pub fn queue_payout(&self, cause: &str, recipient: &str, amount_utia: u64, memo: &str) -> anyhow::Result<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO payouts (cause, recipient, amount_utia, memo, status) VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![cause, recipient, amount_utia as i64, memo],
        )?;
        Ok(())
    }

    /// The oldest payout that is not settled yet.
    pub fn next_payout(&self) -> anyhow::Result<Option<Payout>> {
        let payout = self
            .0
            .query_row(
                "SELECT id, recipient, amount_utia, memo, tx_hash, sequence, tx_bytes FROM payouts
                 WHERE status IN ('queued', 'sent') ORDER BY id LIMIT 1",
                [],
                |row| {
                    Ok(Payout {
                        id: row.get(0)?,
                        recipient: row.get(1)?,
                        amount_utia: row.get::<_, i64>(2)? as u64,
                        memo: row.get(3)?,
                        tx_hash: row.get(4)?,
                        sequence: row.get::<_, Option<i64>>(5)?.map(|s| s as u64),
                        tx_bytes: row.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(payout)
    }

    /// Recorded before broadcasting, so a crash can never lose track of a signed payout.
    pub fn payout_signed(&self, id: i64, tx_hash: &str, sequence: u64, tx_bytes: &[u8]) -> anyhow::Result<()> {
        self.0.execute(
            "UPDATE payouts SET status = 'sent', tx_hash = ?2, sequence = ?3, tx_bytes = ?4 WHERE id = ?1",
            params![id, tx_hash, sequence as i64, tx_bytes],
        )?;
        Ok(())
    }

    pub fn payout_settled(&self, id: i64, error: Option<&str>) -> anyhow::Result<()> {
        let status = if error.is_some() { "failed" } else { "done" };
        self.0.execute("UPDATE payouts SET status = ?2, error = ?3 WHERE id = ?1", params![id, status, error])?;
        Ok(())
    }

    /// Back to the queue, for a payout whose sequence was used by something else.
    pub fn payout_requeued(&self, id: i64) -> anyhow::Result<()> {
        self.0.execute(
            "UPDATE payouts SET status = 'queued', tx_hash = NULL, sequence = NULL, tx_bytes = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- chain cursors

    pub fn cursor(&self, address: &str) -> anyhow::Result<Option<u64>> {
        let next = self
            .0
            .query_row("SELECT next_height FROM cursors WHERE address = ?1", [address], |row| row.get::<_, i64>(0))
            .optional()?;
        Ok(next.map(|height| height as u64))
    }

    pub fn set_cursor(&self, address: &str, next_height: u64) -> anyhow::Result<()> {
        self.0.execute(
            "INSERT INTO cursors (address, next_height) VALUES (?1, ?2)
             ON CONFLICT (address) DO UPDATE SET next_height = excluded.next_height",
            params![address, next_height as i64],
        )?;
        Ok(())
    }
}

/// Columns added after a database was first created. SQLite has no `ADD COLUMN IF NOT EXISTS`.
fn add_missing_columns(connection: &rusqlite::Connection) -> anyhow::Result<()> {
    const ADDED: [(&str, &str, &str); 2] =
        [("payments", "fee_utia", "INTEGER NOT NULL DEFAULT 0"), ("payouts", "tx_bytes", "BLOB")];
    for (table, column, kind) in ADDED {
        let exists: bool = connection.query_row(
            &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            connection.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
        }
    }
    Ok(())
}

fn offer_from_row(row: &Row) -> rusqlite::Result<anyhow::Result<StoredOffer>> {
    let (id, spec, last_seen): (String, String, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
    Ok(serde_json::from_str(&spec).map(|spec| StoredOffer { id, spec, last_seen }).context("corrupt offer"))
}

fn lease_from_row(row: &Row) -> rusqlite::Result<anyhow::Result<Lease>> {
    let decode = || -> anyhow::Result<Lease> {
        Ok(Lease {
            id: row.get(0)?,
            offer_id: row.get(1)?,
            renter: row.get(2)?,
            ssh_key: row.get(3)?,
            status: parse_enum(&row.get::<_, String>(4)?)?,
            end_reason: row.get::<_, Option<String>>(5)?.as_deref().map(parse_enum).transpose()?,
            created_at: row.get(6)?,
            started_at: row.get(7)?,
            ended_at: row.get(8)?,
            paid_utia: row.get::<_, i64>(9)? as u64,
            price_utia_per_hour: row.get::<_, i64>(10)? as u64,
            grace_seconds: row.get::<_, i64>(11)? as u64,
            payout_address: row.get(12)?,
            connection: row.get::<_, Option<String>>(13)?.as_deref().map(serde_json::from_str).transpose()?,
            agent_wallet: row.get(14)?,
        })
    };
    Ok(decode())
}

/// Status and end reason are stored as their snake_case serde names, e.g. `active`.
fn enum_text<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(text) => Ok(text),
        other => anyhow::bail!("expected a unit variant, got {other}"),
    }
}

fn parse_enum<T: serde::de::DeserializeOwned>(text: &str) -> anyhow::Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(text.to_string()))?)
}

#[cfg(test)]
mod tests {
    use protocol::api::{Lease, LeaseStatus, Payment};

    use super::Store;

    #[test]
    fn leases_and_payments_round_trip() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let lease = Lease {
            id: "l1".into(),
            offer_id: "o1".into(),
            renter: "renter".into(),
            ssh_key: "ssh-ed25519 AAAA".into(),
            status: LeaseStatus::Active,
            end_reason: None,
            created_at: 1,
            started_at: Some(2),
            ended_at: None,
            paid_utia: 3,
            price_utia_per_hour: 4,
            grace_seconds: 5,
            payout_address: "payout".into(),
            connection: None,
            agent_wallet: Some("wallet".into()),
        };
        let payment = Payment {
            tx_hash: "AB".into(),
            height: 7,
            time: 8,
            sender: "wallet".into(),
            recipient: "payout".into(),
            amount_utia: 4,
            memo: "agentcloud:pay:l1".into(),
            lease_id: Some("l1".into()),
            outcome: "credited".into(),
            fee_utia: 302,
        };
        store
            .write(|db| {
                db.insert_lease(&lease)?;
                db.insert_payment(&payment, 0)
            })
            .unwrap();
        let (leases, payments, typical) = store
            .read(|db| Ok((db.leases_of_renter("renter")?, db.payments_of_renter("renter", 10)?, db.typical_fee()?)))
            .unwrap();
        assert_eq!(leases[0].agent_wallet.as_deref(), Some("wallet"));
        assert_eq!(payments[0].fee_utia, 302);
        assert_eq!(typical, Some(302));
    }
}
