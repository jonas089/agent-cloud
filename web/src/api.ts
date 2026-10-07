// The market's HTTP API. Types mirror `crates/protocol/src/api.rs`; amounts are utia and
// times are unix seconds.

export interface ChainConfig {
  chain_id: string;
  rest: string;
  rpc: string;
  denom: string;
  bech32_prefix: string;
  /** Simulated gas is multiplied by this before signing. */
  gas_adjustment: number;
  explorer: string;
}

export interface MarketConfig {
  chain: ChainConfig;
  escrow_address: string;
  escrow_fee_utia: number;
  /** The chain's minimum gas price right now. */
  gas_price: number;
  /** The average fee of recent agentcloud transfers, or null before the first one. */
  typical_fee_utia: number | null;
  /** Where the source lives, for the setup instructions. Empty when not configured. */
  repository: string;
}

export interface OfferSpec {
  name: string;
  description: string;
  region: string;
  host: string;
  /** Per sandbox ceilings; slots share the machine. */
  cpus: number;
  memory_mb: number;
  disk_gb: number;
  gpu: string | null;
  /** Where to check the attestation of the TEE the sandboxes run in. */
  attestation_url: string | null;
  price_utia_per_hour: number;
  grace_seconds: number;
  slots: number;
  payout_address: string;
}

export interface Offer extends OfferSpec {
  id: string;
  online: boolean;
  free_slots: number;
  last_seen: number;
}

export type LeaseStatus = "pending" | "active" | "ended";
export type EndReason = "unpaid" | "outbid" | "expired" | "cancelled";

export interface Connection {
  host: string;
  port: number;
  user: string;
  /** SSH wrapped in TLS, as a TEE gateway serves it. */
  tls: boolean;
}

export interface Lease {
  id: string;
  offer_id: string;
  renter: string;
  ssh_key: string;
  status: LeaseStatus;
  end_reason: EndReason | null;
  created_at: number;
  started_at: number | null;
  ended_at: number | null;
  paid_utia: number;
  price_utia_per_hour: number;
  grace_seconds: number;
  payout_address: string;
  connection: Connection | null;
  /** The agent's own wallet; it pays the rent after the first minute. */
  agent_wallet: string | null;
}

export interface PaymentRequest {
  to: string;
  amount_utia: number;
  memo: string;
}

export interface Payment {
  tx_hash: string;
  height: number;
  time: number;
  sender: string;
  recipient: string;
  amount_utia: number;
  memo: string;
  lease_id: string | null;
  outcome: string;
  fee_utia: number;
}

export interface Account {
  address: string;
  balance_utia: number;
  leases: Lease[];
  agent_wallets: { lease_id: string; address: string; balance_utia: number }[];
  payments: Payment[];
  rent_utia_per_hour: number;
  gas_utia_per_hour: number;
}

export interface ChainAccount {
  account_number: number;
  sequence: number;
}

export const market = {
  config: () => request<MarketConfig>("GET", "/api/config"),
  offers: () => request<Offer[]>("GET", "/api/offers"),
  lease: (id: string) => request<Lease>("GET", `/api/leases/${id}`),
  leases: (renter: string) => request<Lease[]>("GET", `/api/leases?renter=${renter}`),
  account: (address: string) => request<Account>("GET", `/api/accounts/${address}`),
  createLease: (offer_id: string, renter: string, ssh_key: string) =>
    request<{ lease: Lease; payment: PaymentRequest }>("POST", "/api/leases", { offer_id, renter, ssh_key }),
  chainAccount: (address: string) => request<ChainAccount>("GET", `/api/chain/accounts/${address}`),
  broadcast: (tx_bytes: string) => request<{ tx_hash: string }>("POST", "/api/chain/broadcast", { tx_bytes }),
  simulate: (tx_bytes: string) => request<{ gas_used: number }>("POST", "/api/chain/simulate", { tx_bytes }),
};

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const parsed = text ? JSON.parse(text) : undefined;
  if (!response.ok) throw new Error(parsed?.error ?? `${response.status} ${response.statusText}`);
  return parsed as T;
}

// ---------------------------------------------------------------- shared rules
// The same rules as `crates/protocol`.

/** The command that logs in to a sandbox, mirroring `Connection::ssh_command`. */
export function sshCommand(c: Connection, identity?: string): string {
  const key = identity ? `-i ${identity} ` : "";
  if (!c.tls) return `ssh ${key}-p ${c.port} ${c.user}@${c.host}`;
  const proxy = `openssl s_client -quiet -connect ${c.host}:${c.port} -servername ${c.host}`;
  return `ssh ${key}-o ProxyCommand='${proxy}' ${c.user}@${c.host}`;
}

/** A `~/.ssh/config` entry, so the sandbox is just `ssh agentcloud-<lease>`. */
export function sshConfig(c: Connection, lease: string, identity: string): string {
  const lines = [`Host agentcloud-${lease}`, `  HostName ${c.host}`, `  User ${c.user}`, `  Port ${c.port}`, `  IdentityFile ${identity}`];
  if (c.tls) lines.push(`  ProxyCommand openssl s_client -quiet -connect %h:%p -servername %h`);
  return lines.join("\n");
}

/** The renter's commitment to their SSH key, as `ssh_key_commitment` computes it. */
export async function sshKeyCommitment(sshKey: string): Promise<string> {
  const key = sshKey.trim().split(/\s+/).slice(0, 2).join(" ");
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key)));
  return Array.from(digest.slice(0, 16), (b) => b.toString(16).padStart(2, "0")).join("");
}

// ---------------------------------------------------------------- lease accounting

/** When the prepaid time runs out. */
export function paidUntil(lease: Lease): number | null {
  if (lease.started_at === null) return null;
  return lease.started_at + Math.floor((lease.paid_utia * 3600) / Math.max(lease.price_utia_per_hour, 1));
}

/** When the lease ends unless another payment arrives. */
export function expiresAt(lease: Lease): number | null {
  const until = paidUntil(lease);
  return until === null ? null : until + lease.grace_seconds;
}
