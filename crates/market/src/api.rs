//! The market's HTTP API under `/api`. Everything else serves the web app.
//!
//! Public routes need no account: wallets are identities, and what a request can do is
//! bounded by what that wallet then pays or signs on chain. Routes under `/api/agent` are
//! signed by a provider agent (see [`protocol::agent_auth`]).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use protocol::agent_auth::{SignedRequest, KEY_HEADER, SIGNATURE_HEADER, TIMESTAMP_HEADER};
use protocol::api::{
    ssh_key_commitment, Account, AgentWallet, ApiError, Broadcast, Broadcasted, ChainAccount, Lease, LeaseCreated,
    LeaseStatus, MarketConfig, NewLease, Offer, OfferSpec, PaymentRequest, SandboxReport, Simulated, Status,
};
use protocol::memo::Memo;
use protocol::unix_now;
use rand::Rng;
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};

use crate::store::StoredOffer;
use crate::Market;

/// A renter may hold this many leases waiting for their first payment.
const MAX_PENDING_PER_RENTER: usize = 5;
/// Offers silent for longer than this drop off the market.
const LISTED_FOR_SECONDS: i64 = 24 * 3600;

pub fn router(market: Arc<Market>) -> Router {
    let web = ServeDir::new(&market.config.web_dir)
        .not_found_service(ServeFile::new(market.config.web_dir.join("index.html")));
    let api = Router::new()
        .route("/config", get(config))
        .route("/status", get(status))
        .route("/offers", get(offers))
        .route("/leases", post(create_lease).get(leases_of_renter))
        .route("/leases/{id}", get(lease))
        .route("/accounts/{address}", get(account))
        .route("/chain/accounts/{address}", get(chain_account))
        .route("/chain/broadcast", post(broadcast))
        .route("/chain/simulate", post(simulate))
        .route("/agent/offer", put(agent_offer))
        .route("/agent/leases", get(agent_leases))
        .route("/agent/leases/{id}/sandbox", put(agent_sandbox))
        .fallback(|| async { Failure::NotFound("no such endpoint") });
    Router::new().nest("/api", api).fallback_service(web).with_state(market)
}

// -------------------------------------------------------------------- public

async fn config(State(market): State<Arc<Market>>) -> Result<Json<MarketConfig>, Failure> {
    Ok(Json(MarketConfig {
        chain: market.config.chain.clone(),
        escrow_address: market.escrow_address.clone(),
        escrow_fee_utia: market.config.escrow_fee_utia,
        gas_price: market.chain.gas_price(&market.escrow).await.map_err(Failure::Chain)?,
        typical_fee_utia: market.store.read(|db| db.typical_fee())?,
        repository: market.config.repository.clone(),
    }))
}

async fn status(State(market): State<Arc<Market>>) -> Result<Json<Status>, Failure> {
    let synced = *market.synced.read().expect("synced lock poisoned");
    let chain_height = market.chain.head().await.map_err(Failure::Chain)?.height;
    Ok(Json(Status { chain_height, indexed_height: synced.map_or(0, |head| head.height) }))
}

async fn offers(State(market): State<Arc<Market>>) -> Result<Json<Vec<Offer>>, Failure> {
    let offers = market.store.read(|db| {
        db.offers_seen_since(unix_now() - LISTED_FOR_SECONDS)?
            .into_iter()
            .map(|offer| present_offer(&market, db, offer))
            .collect()
    })?;
    Ok(Json(offers))
}

async fn create_lease(
    State(market): State<Arc<Market>>,
    Json(request): Json<NewLease>,
) -> Result<Json<LeaseCreated>, Failure> {
    request.check().map_err(Failure::invalid)?;
    if !market.chain.is_address(&request.renter) {
        return Err(Failure::BadRequest("renter is not a valid address".into()));
    }
    let lease = market.store.write(|db| {
        let offer = db.offer(&request.offer_id)?.ok_or(Failure::NotFound("no such offer"))?;
        let offer = present_offer(&market, db, offer)?;
        if !offer.online || offer.free_slots == 0 {
            return Err(Failure::Conflict("this offer is not taking new leases right now").into());
        }
        let pending = db.leases_of_renter(&request.renter)?.into_iter().filter(|l| l.status == LeaseStatus::Pending);
        if pending.count() >= MAX_PENDING_PER_RENTER {
            return Err(Failure::Conflict("too many unpaid leases, pay or wait for them to lapse").into());
        }
        let lease = Lease {
            id: new_lease_id(),
            offer_id: offer.id,
            renter: request.renter.clone(),
            ssh_key: request.ssh_key.trim().to_string(),
            status: LeaseStatus::Pending,
            end_reason: None,
            created_at: unix_now(),
            started_at: None,
            ended_at: None,
            paid_utia: 0,
            price_utia_per_hour: offer.spec.price_utia_per_hour,
            grace_seconds: offer.spec.grace_seconds,
            payout_address: offer.spec.payout_address,
            connection: None,
            agent_wallet: None,
        };
        db.insert_lease(&lease)?;
        Ok(lease)
    })?;
    let payment = PaymentRequest {
        to: market.escrow_address.clone(),
        amount_utia: lease.price_utia_per_hour + market.config.escrow_fee_utia,
        memo: Memo::Activate { lease: lease.id.clone(), key: ssh_key_commitment(&lease.ssh_key) }.to_string(),
    };
    Ok(Json(LeaseCreated { lease, payment }))
}

#[derive(Deserialize)]
struct RenterQuery {
    renter: String,
}

async fn leases_of_renter(
    State(market): State<Arc<Market>>,
    Query(query): Query<RenterQuery>,
) -> Result<Json<Vec<Lease>>, Failure> {
    Ok(Json(market.store.read(|db| db.leases_of_renter(&query.renter))?))
}

async fn lease(State(market): State<Arc<Market>>, Path(id): Path<String>) -> Result<Json<Lease>, Failure> {
    market.store.read(|db| db.lease(&id))?.map(Json).ok_or(Failure::NotFound("no such lease"))
}

async fn account(State(market): State<Arc<Market>>, Path(address): Path<String>) -> Result<Json<Account>, Failure> {
    if !market.chain.is_address(&address) {
        return Err(Failure::BadRequest("not a valid address".into()));
    }
    let (leases, payments) =
        market.store.read(|db| Ok((db.leases_of_renter(&address)?, db.payments_of_renter(&address, 100)?)))?;
    let balance_utia = market.chain.balance(&address).await.map_err(Failure::Chain)?;
    let mut agent_wallets = Vec::new();
    for lease in leases.iter().filter(|lease| lease.status != LeaseStatus::Ended) {
        let Some(wallet) = &lease.agent_wallet else { continue };
        let balance_utia = market.chain.balance(wallet).await.map_err(Failure::Chain)?;
        agent_wallets.push(AgentWallet { lease_id: lease.id.clone(), address: wallet.clone(), balance_utia });
    }
    let active = leases.iter().filter(|lease| lease.status == LeaseStatus::Active);
    let rent_utia_per_hour = active.clone().map(|lease| lease.price_utia_per_hour).sum();
    // One rent transfer per lease and hour, at what transfers have recently cost.
    let typical_fee = market.store.read(|db| db.typical_fee())?.unwrap_or(0);
    let gas_utia_per_hour = active.count() as u64 * typical_fee;
    Ok(Json(Account { address, balance_utia, leases, agent_wallets, payments, rent_utia_per_hour, gas_utia_per_hour }))
}

async fn chain_account(
    State(market): State<Arc<Market>>,
    Path(address): Path<String>,
) -> Result<Json<ChainAccount>, Failure> {
    let account = market.chain.account(&address).await.map_err(Failure::Chain)?;
    account.map(Json).ok_or(Failure::NotFound("this account has never received funds"))
}

async fn broadcast(
    State(market): State<Arc<Market>>,
    Json(request): Json<Broadcast>,
) -> Result<Json<Broadcasted>, Failure> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&request.tx_bytes)
        .map_err(|_| Failure::BadRequest("tx_bytes is not base64".into()))?;
    let tx_hash = market.chain.broadcast(&bytes).await.map_err(Failure::Chain)?;
    Ok(Json(Broadcasted { tx_hash }))
}

/// Gas estimation for wallets in the browser, which cannot reach the public nodes.
async fn simulate(
    State(market): State<Arc<Market>>,
    Json(request): Json<Broadcast>,
) -> Result<Json<Simulated>, Failure> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&request.tx_bytes)
        .map_err(|_| Failure::BadRequest("tx_bytes is not base64".into()))?;
    let gas_used = market.chain.simulate(&bytes).await.map_err(Failure::Chain)?;
    Ok(Json(Simulated { gas_used }))
}

// -------------------------------------------------------------------- provider agents

async fn agent_offer(
    State(market): State<Arc<Market>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Offer>, Failure> {
    let offer_id = authenticate(&method, &uri, &headers, &body)?;
    let spec: OfferSpec = serde_json::from_slice(&body).map_err(Failure::invalid)?;
    spec.check().map_err(Failure::invalid)?;
    if !market.chain.is_address(&spec.payout_address) {
        return Err(Failure::BadRequest("payout_address is not a valid address".into()));
    }
    let offer = market.store.write(|db| {
        db.upsert_offer(&offer_id, &spec, unix_now())?;
        let stored = db.offer(&offer_id)?.expect("just written");
        present_offer(&market, db, stored)
    })?;
    Ok(Json(offer))
}

/// The leases the agent must keep running. Any sandbox not in this list gets wiped.
async fn agent_leases(
    State(market): State<Arc<Market>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<Lease>>, Failure> {
    let offer_id = authenticate(&method, &uri, &headers, &[])?;
    Ok(Json(market.store.read(|db| db.leases_of_offer(&offer_id, LeaseStatus::Active))?))
}

async fn agent_sandbox(
    State(market): State<Arc<Market>>,
    Path(id): Path<String>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, Failure> {
    let offer_id = authenticate(&method, &uri, &headers, &body)?;
    let report: SandboxReport = serde_json::from_slice(&body).map_err(Failure::invalid)?;
    if !market.chain.is_address(&report.agent_wallet) {
        return Err(Failure::BadRequest("agent_wallet is not a valid address".into()));
    }
    market.store.write(|db| {
        let lease = db.lease(&id)?.ok_or(Failure::NotFound("no such lease"))?;
        if lease.offer_id != offer_id {
            return Err(Failure::Unauthorized("this lease belongs to another offer".into()).into());
        }
        db.set_sandbox(&id, &report)
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Signatures cover the full request path, so handlers pass the URI from [`OriginalUri`]:
/// inside the nested `/api` router, a plain `Uri` has that prefix stripped.
fn authenticate(method: &Method, uri: &axum::http::Uri, headers: &HeaderMap, body: &[u8]) -> Result<String, Failure> {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or_default();
    let request = SignedRequest {
        key: header(KEY_HEADER),
        timestamp: header(TIMESTAMP_HEADER),
        signature: header(SIGNATURE_HEADER),
    };
    request
        .verify(method.as_str(), uri.path(), body, unix_now())
        .map_err(|error| Failure::Unauthorized(error.to_string()))
}

// -------------------------------------------------------------------- shared

fn present_offer(market: &Market, db: &crate::store::Db, offer: StoredOffer) -> anyhow::Result<Offer> {
    let active = db.leases_of_offer(&offer.id, LeaseStatus::Active)?.len() as u32;
    Ok(Offer {
        online: unix_now() - offer.last_seen <= market.config.offline_after_seconds,
        free_slots: offer.spec.slots.saturating_sub(active),
        id: offer.id,
        spec: offer.spec,
        last_seen: offer.last_seen,
    })
}

fn new_lease_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..10).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

/// Every way a request can fail, each with its status code.
#[derive(Debug)]
enum Failure {
    BadRequest(String),
    Unauthorized(String),
    NotFound(&'static str),
    Conflict(&'static str),
    /// The chain's REST endpoint failed or refused.
    Chain(anyhow::Error),
    Internal(anyhow::Error),
}

impl Failure {
    fn invalid(error: impl std::fmt::Display) -> Self {
        Self::BadRequest(error.to_string())
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message) | Self::Unauthorized(message) => f.write_str(message),
            Self::NotFound(message) | Self::Conflict(message) => f.write_str(message),
            Self::Chain(error) => write!(f, "chain: {error:#}"),
            Self::Internal(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for Failure {}

/// Store closures return `anyhow` errors; a [`Failure`] raised inside one keeps its status.
impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        error.downcast::<Failure>().unwrap_or_else(Failure::Internal)
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Chain(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(error) => {
                tracing::error!("request failed: {error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, Json(ApiError { error: self.to_string() })).into_response()
    }
}
