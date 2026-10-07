//! REST client for the few queries and the one transaction type agentcloud needs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use base64::Engine;
use cosmrs::bank::MsgSend;
use cosmrs::tx::{Body, Fee, Msg, SignDoc, SignerInfo};
use cosmrs::Coin;
use protocol::api::{ChainAccount, ChainConfig};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{cosmrs_error, Wallet};

/// Transfers per page when searching. The SDK caps pages at 100.
const PAGE_SIZE: usize = 100;

/// How long a learned gas price is trusted before the node is asked again.
const GAS_PRICE_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct Chain {
    http: reqwest::Client,
    config: ChainConfig,
    /// The node's minimum gas price, and when it was learned.
    gas_price: Arc<Mutex<Option<(f64, Instant)>>>,
}

/// One bank send, taken out of a successful transaction.
#[derive(Clone, Debug)]
pub struct Transfer {
    pub tx_hash: String,
    /// Position of the message in its transaction, so two sends in one tx stay distinct.
    pub msg_index: u32,
    pub height: u64,
    pub time: i64,
    pub sender: String,
    pub recipient: String,
    /// In the chain's base denom; transfers of other denoms are skipped.
    pub amount: u64,
    pub memo: String,
    /// The fee its transaction paid, in the base denom.
    pub fee: u64,
}

/// A transaction signed and ready to broadcast. Its hash is known before broadcasting, so a
/// caller can record it first and later learn whether it landed.
pub struct SignedTx {
    pub hash: String,
    pub bytes: Vec<u8>,
    /// The signer's account sequence it was signed for. Two transactions with the same
    /// sequence can never both land.
    pub sequence: u64,
}

/// What a transaction pays: its gas limit and the fee for it, both from the chain.
#[derive(Clone, Copy, Debug)]
pub struct TxFee {
    pub gas: u64,
    pub amount: u64,
}

/// The newest block the node knows about.
#[derive(Clone, Copy, Debug)]
pub struct Head {
    pub height: u64,
    pub time: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct IncludedTx {
    pub height: u64,
    pub succeeded: bool,
}

impl Chain {
    pub fn new(config: ChainConfig) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client");
        Self { http, config, gas_price: Arc::new(Mutex::new(None)) }
    }

    pub fn config(&self) -> &ChainConfig {
        &self.config
    }

    pub async fn head(&self) -> anyhow::Result<Head> {
        let block: Value = self.get("/cosmos/base/tendermint/v1beta1/blocks/latest", &[]).await?;
        let header = &block["block"]["header"];
        Ok(Head { height: number(&header["height"])?, time: timestamp(&header["time"])? })
    }

    /// Whether `address` is a well-formed account address on this chain.
    pub fn is_address(&self, address: &str) -> bool {
        address.parse::<cosmrs::AccountId>().is_ok_and(|id| id.prefix() == self.config.bech32_prefix)
    }

    pub async fn balance(&self, address: &str) -> anyhow::Result<u64> {
        let path = format!("/cosmos/bank/v1beta1/balances/{address}/by_denom");
        let balance: Value = self.get(&path, &[("denom", &self.config.denom)]).await?;
        number(&balance["balance"]["amount"])
    }

    /// `None` until the address has received funds for the first time.
    pub async fn account(&self, address: &str) -> anyhow::Result<Option<ChainAccount>> {
        let path = format!("/cosmos/auth/v1beta1/accounts/{address}");
        let Some(found) = self.get_optional::<Value>(&path, &[]).await? else { return Ok(None) };
        let account = &found["account"];
        Ok(Some(ChainAccount {
            account_number: number(&account["account_number"])?,
            sequence: number(&account["sequence"])?,
        }))
    }

    /// The minimum gas price the node we broadcast through enforces, learned at most every
    /// ten minutes. Nodes do not reliably publish it, but each names it when turning down a
    /// transaction that pays too little, so `wallet` sends itself a free 1 utia transfer and
    /// the rejection gives the price. (A node that accepted it would move 1 utia from the
    /// wallet to itself, for nothing.) A wallet not on chain yet falls back to the node's
    /// configured minimum.
    pub async fn gas_price(&self, wallet: &Wallet) -> anyhow::Result<f64> {
        if let Some((price, learned)) = *self.gas_price.lock().expect("gas price lock") {
            if learned.elapsed() < GAS_PRICE_TTL {
                return Ok(price);
            }
        }
        let me = wallet.address().to_string();
        let price = if self.account(&me).await?.is_some() {
            let probe = TxFee { gas: 100_000, amount: 0 };
            let tx = self.sign_send(wallet, &me, 1, "agentcloud gas price probe", probe).await?;
            match self.broadcast(&tx.bytes).await {
                Ok(_) => 0.0,
                Err(error) => minimum_from_rejection(&error.to_string())?,
            }
        } else {
            self.configured_gas_price().await?
        };
        *self.gas_price.lock().expect("gas price lock") = Some((price, Instant::now()));
        Ok(price)
    }

    async fn configured_gas_price(&self) -> anyhow::Result<f64> {
        let config: Value = self.get("/cosmos/base/node/v1beta1/config", &[]).await?;
        let price = config["minimum_gas_price"].as_str().unwrap_or_default();
        let amount = price.strip_suffix(self.config.denom.as_str()).unwrap_or(price);
        amount.parse().with_context(|| format!("the node publishes no usable minimum gas price ({price:?})"))
    }

    /// What a send costs right now: the gas the chain uses when simulating it, times the
    /// configured adjustment, priced at the chain's gas price.
    pub async fn estimate_send(&self, wallet: &Wallet, to: &str, amount: u64, memo: &str) -> anyhow::Result<TxFee> {
        // Simulation does not check the fee, but paying one costs gas, so the draft pays a token
        // fee for its gas use to include the fee transfer.
        let draft = self.sign_send(wallet, to, amount, memo, TxFee { gas: 1_000_000, amount: 1 }).await?;
        let gas = (self.simulate(&draft.bytes).await? as f64 * self.config.gas_adjustment).ceil() as u64;
        let amount = (gas as f64 * self.gas_price(wallet).await?).ceil() as u64;
        Ok(TxFee { gas, amount })
    }

    /// Signs a bank send of `amount` base units with `memo`, paying `fee`.
    pub async fn sign_send(
        &self,
        wallet: &Wallet,
        to: &str,
        amount: u64,
        memo: &str,
        fee: TxFee,
    ) -> anyhow::Result<SignedTx> {
        let from = wallet.address().to_string();
        let account =
            self.account(&from).await?.with_context(|| format!("{from} does not exist on chain yet, fund it first"))?;
        let denom: cosmrs::Denom = self.config.denom.parse().map_err(cosmrs_error)?;
        let message = MsgSend {
            from_address: wallet.address().clone(),
            to_address: to.parse().map_err(|_| anyhow::anyhow!("invalid address {to}"))?,
            amount: vec![Coin { denom: denom.clone(), amount: amount.into() }],
        };
        let body = Body::new(vec![message.to_any().map_err(cosmrs_error)?], memo, 0u32);
        let fee = Fee::from_amount_and_gas(Coin { denom, amount: fee.amount.into() }, fee.gas);
        let auth_info = SignerInfo::single_direct(Some(wallet.key().public_key()), account.sequence).auth_info(fee);
        let chain_id = self.config.chain_id.parse().map_err(|e| anyhow::anyhow!("chain id: {e}"))?;
        let sign_doc = SignDoc::new(&body, &auth_info, &chain_id, account.account_number).map_err(cosmrs_error)?;
        let bytes = sign_doc.sign(wallet.key()).and_then(|raw| raw.to_bytes()).map_err(cosmrs_error)?;
        Ok(SignedTx { hash: tx_hash(&bytes), bytes, sequence: account.sequence })
    }

    /// The gas the chain uses executing these transaction bytes, without committing them.
    pub async fn simulate(&self, tx_bytes: &[u8]) -> anyhow::Result<u64> {
        let body = serde_json::json!({ "tx_bytes": base64::engine::general_purpose::STANDARD.encode(tx_bytes) });
        let url = format!("{}/cosmos/tx/v1beta1/simulate", self.config.rest);
        let simulated: Value = parse(self.http.post(&url).json(&body).send().await?).await?;
        number(&simulated["gas_info"]["gas_used"])
    }

    /// Submits signed transaction bytes and returns the hash once the node accepted them
    /// into its mempool. Inclusion is a separate question for [`Chain::wait_for`].
    pub async fn broadcast(&self, tx_bytes: &[u8]) -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Response {
            tx_response: TxResponse,
        }
        let body = serde_json::json!({
            "tx_bytes": base64::engine::general_purpose::STANDARD.encode(tx_bytes),
            "mode": "BROADCAST_MODE_SYNC",
        });
        let url = format!("{}/cosmos/tx/v1beta1/txs", self.config.rest);
        let response = self.http.post(&url).json(&body).send().await?;
        let Response { tx_response } = parse(response).await?;
        if tx_response.code != 0 {
            bail!("transaction rejected (code {}): {}", tx_response.code, tx_response.raw_log);
        }
        Ok(tx_response.txhash)
    }

    /// Where the transaction landed, or `None` if the node has not seen it in a block.
    pub async fn tx(&self, hash: &str) -> anyhow::Result<Option<IncludedTx>> {
        #[derive(Deserialize)]
        struct Response {
            tx_response: TxResponse,
        }
        let path = format!("/cosmos/tx/v1beta1/txs/{hash}");
        let Some(Response { tx_response }) = self.get_optional(&path, &[]).await? else { return Ok(None) };
        Ok(Some(IncludedTx { height: tx_response.height.parse()?, succeeded: tx_response.code == 0 }))
    }

    /// Polls until the transaction is in a block or `timeout` passes.
    pub async fn wait_for(&self, hash: &str, timeout: Duration) -> anyhow::Result<Option<IncludedTx>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(included) = self.tx(hash).await? {
                return Ok(Some(included));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Every successful send to `recipient` in blocks `from..=to`, oldest first.
    pub async fn transfers_to(&self, recipient: &str, from: u64, to: u64) -> anyhow::Result<Vec<Transfer>> {
        let query = format!("transfer.recipient='{recipient}' AND tx.height>={from} AND tx.height<={to}");
        let transfers = self.search(&query).await?;
        Ok(transfers.into_iter().filter(|t| t.recipient == recipient).collect())
    }

    /// Every successful send signed by `sender`, oldest first.
    pub async fn transfers_from(&self, sender: &str) -> anyhow::Result<Vec<Transfer>> {
        let transfers = self.search(&format!("message.sender='{sender}'")).await?;
        Ok(transfers.into_iter().filter(|t| t.sender == sender).collect())
    }

    /// The bank sends in every successful transaction matching an event query, oldest first.
    async fn search(&self, query: &str) -> anyhow::Result<Vec<Transfer>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            tx_responses: Vec<TxResponse>,
            #[serde(default)]
            total: String,
        }
        let limit = PAGE_SIZE.to_string();
        let mut transfers = Vec::new();
        for page in 1.. {
            let page_number = page.to_string();
            let params = [
                ("query", query),
                ("order_by", "ORDER_BY_ASC"),
                ("limit", limit.as_str()),
                ("page", page_number.as_str()),
            ];
            let Page { tx_responses, total } = self.get("/cosmos/tx/v1beta1/txs", &params).await?;
            let done = tx_responses.len() < PAGE_SIZE || page * PAGE_SIZE >= total.parse().unwrap_or(0);
            for response in tx_responses.into_iter().filter(|r| r.code == 0) {
                transfers.extend(response.sends(&self.config.denom)?);
            }
            if done {
                break;
            }
        }
        Ok(transfers)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> anyhow::Result<T> {
        self.get_optional(path, params).await?.with_context(|| format!("{path} not found"))
    }

    /// A GET where "not found" is an answer rather than an error.
    async fn get_optional<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<Option<T>> {
        let url = format!("{}{path}", self.config.rest);
        let response = self.http.get(&url).query(params).send().await.with_context(|| format!("GET {path}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        parse(response).await.map(Some)
    }
}

#[derive(Deserialize)]
struct TxResponse {
    #[serde(default)]
    height: String,
    txhash: String,
    code: u32,
    #[serde(default)]
    raw_log: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    tx: Value,
}

impl TxResponse {
    /// Its bank sends in `denom`; sends of other denoms are skipped.
    fn sends(&self, denom: &str) -> anyhow::Result<Vec<Transfer>> {
        let body = &self.tx["body"];
        let memo = body["memo"].as_str().unwrap_or_default();
        let height = self.height.parse()?;
        let time = timestamp(&self.timestamp.clone().into())?;
        let messages = body["messages"].as_array().map(Vec::as_slice).unwrap_or_default();
        let fees = self.tx["auth_info"]["fee"]["amount"].as_array().map(Vec::as_slice).unwrap_or_default();
        let fee =
            fees.iter().find(|coin| coin["denom"] == denom).map_or(0, |coin| number(&coin["amount"]).unwrap_or(0));
        let sends = messages.iter().enumerate().filter_map(|(index, message)| {
            if message["@type"] != "/cosmos.bank.v1beta1.MsgSend" {
                return None;
            }
            let amount = message["amount"].as_array()?.iter().find(|coin| coin["denom"] == denom)?;
            Some(Transfer {
                tx_hash: self.txhash.clone(),
                msg_index: index as u32,
                height,
                time,
                sender: message["from_address"].as_str().unwrap_or_default().to_string(),
                recipient: message["to_address"].as_str().unwrap_or_default().to_string(),
                amount: number(&amount["amount"]).unwrap_or(0),
                memo: memo.to_string(),
                fee,
            })
        });
        Ok(sends.collect())
    }
}

async fn parse<T: DeserializeOwned>(response: reqwest::Response) -> anyhow::Result<T> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("{status}: {}", text.chars().take(300).collect::<String>());
    }
    serde_json::from_str(&text).context("unexpected response from the chain")
}

/// The SDK encodes 64-bit integers as JSON strings.
fn number(value: &Value) -> anyhow::Result<u64> {
    match value {
        Value::String(s) => Ok(s.parse()?),
        Value::Number(n) => n.as_u64().context("not an unsigned integer"),
        _ => bail!("expected a number, got {value}"),
    }
}

fn timestamp(value: &Value) -> anyhow::Result<i64> {
    let text = value.as_str().context("expected a timestamp")?;
    Ok(chrono::DateTime::parse_from_rfc3339(text).with_context(|| format!("bad timestamp {text}"))?.timestamp())
}

/// The price in "... (min gas price: 0.004 utia/gas) ..." from a node's insufficient-fee error.
fn minimum_from_rejection(error: &str) -> anyhow::Result<f64> {
    let after = error.split("min gas price:").nth(1).with_context(|| format!("no gas price in: {error}"))?;
    let number: String = after.trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    number.parse().with_context(|| format!("unreadable gas price in: {error}"))
}

fn tx_hash(tx_bytes: &[u8]) -> String {
    hex::encode_upper(Sha256::digest(tx_bytes))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_minimum_from_a_rejection() {
        let error = "transaction rejected (code 13): insufficient minimum gas price for this node; got: 0utia, \
                     required: 320utia (min gas price: 0.004 utia/gas): insufficient fee";
        assert_eq!(super::minimum_from_rejection(error).unwrap(), 0.004);
        assert!(super::minimum_from_rejection("rejected: out of gas").is_err());
    }
}
