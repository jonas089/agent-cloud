//! The agent's side of the market API. Every request is signed with the agent key; the
//! market checks the signature on the routes under `/api/agent`.

use std::time::Duration;

use anyhow::{bail, Context};
use protocol::agent_auth::AgentKey;
use protocol::api::{Lease, MarketConfig, Offer, OfferSpec, SandboxReport, Status};
use protocol::unix_now;
use reqwest::Method;

pub struct MarketClient {
    http: reqwest::Client,
    base: String,
    key: AgentKey,
}

impl MarketClient {
    pub fn new(base: &str, key: AgentKey) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client");
        Self { http, base: base.trim_end_matches('/').to_string(), key }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Publishes the offer and proves the agent is alive.
    pub async fn heartbeat(&self, offer: &OfferSpec) -> anyhow::Result<Offer> {
        let body = serde_json::to_vec(offer)?;
        Ok(self.send(Method::PUT, "/api/agent/offer", body).await?.json().await?)
    }

    /// The leases that should have a running sandbox right now.
    pub async fn active_leases(&self) -> anyhow::Result<Vec<Lease>> {
        Ok(self.send(Method::GET, "/api/agent/leases", Vec::new()).await?.json().await?)
    }

    pub async fn report_sandbox(&self, lease_id: &str, report: &SandboxReport) -> anyhow::Result<()> {
        let path = format!("/api/agent/leases/{lease_id}/sandbox");
        self.send(Method::PUT, &path, serde_json::to_vec(report)?).await.map(drop)
    }

    pub async fn config(&self) -> anyhow::Result<MarketConfig> {
        Ok(self.send(Method::GET, "/api/config", Vec::new()).await?.json().await?)
    }

    pub async fn status(&self) -> anyhow::Result<Status> {
        Ok(self.send(Method::GET, "/api/status", Vec::new()).await?.json().await?)
    }

    async fn send(&self, method: Method, path: &str, body: Vec<u8>) -> anyhow::Result<reqwest::Response> {
        let mut request = self.http.request(method.clone(), format!("{}{path}", self.base));
        for (name, value) in self.key.sign_request(method.as_str(), path, &body, unix_now()) {
            request = request.header(name, value);
        }
        if !body.is_empty() {
            request = request.header("content-type", "application/json").body(body);
        }
        let response = request.send().await.with_context(|| format!("{method} {path}"))?;
        if !response.status().is_success() {
            let status = response.status();
            bail!("{method} {path}: {status} {}", response.text().await.unwrap_or_default());
        }
        Ok(response)
    }
}
