// Market: every offer, and the rent flow. Renting is one transfer signed in Keplr: the first
// hour plus the escrow fee goes to the escrow, which either forwards it to the provider (the
// lease starts) or refunds it (someone took the last slot first). Its memo commits to the
// renter's SSH key, so the enclave can check on chain that nobody swapped the key in transit.

import { useState } from "react";
import { market, sshKeyCommitment } from "../api";
import type { Governance, Lease, Offer } from "../api";
import type { Session } from "../App";
import { duration, shorten, tia, tiaPrice } from "../format";
import { contractUrl, readCodeControl } from "../governance";
import { generateSshKey } from "../sshKey";
import { usePolling } from "../ui";
import { sendTia } from "../wallet";

export function MarketView({ session }: { session: Session }) {
  const offers = usePolling(market.offers, 10_000);
  const [renting, setRenting] = useState<Offer | null>(null);
  const list = offers.value ?? [];
  const online = list.filter((offer) => offer.online);

  return (
    <>
      <section className="hero">
        <div>
          <h1>Confidential compute for your agents</h1>
          <p className="lead">
            An SSH sandbox inside an Intel TDX enclave, paid by the hour in TIA. Nobody can read what runs in it or the
            API keys you give it, us included. Your agent gets its own wallet and pays its own rent.
          </p>
        </div>
        <div className="hero-stats">
          <div>
            <strong>{online.length}</strong>
            <span>machines online</span>
          </div>
          <div>
            <strong>{online.reduce((sum, offer) => sum + offer.free_slots, 0)}</strong>
            <span>free slots</span>
          </div>
        </div>
      </section>

      {offers.error && <p className="error">Could not load offers: {offers.error}</p>}
      {offers.value && list.length === 0 && (
        <div className="panel empty">
          No machines are online right now. Check back in a few minutes.
        </div>
      )}
      <div className="offer-grid">
        {list.map((offer) => (
          <OfferCard key={offer.id} offer={offer} onRent={() => setRenting(offer)} />
        ))}
      </div>

      {renting && <RentDialog offer={renting} session={session} onClose={() => setRenting(null)} />}
    </>
  );
}

function OfferCard({ offer, onRent }: { offer: Offer; onRent: () => void }) {
  const available = offer.online && offer.free_slots > 0;
  return (
    <article className={`panel offer${available ? "" : " unavailable"}`}>
      <div className="offer-head">
        <div>
          <h2>{offer.name}</h2>
          <span className="muted">{offer.region}</span>
        </div>
        <span className={`status ${offer.online ? "status-active" : "status-ended"}`}>
          {offer.online ? "Online" : "Offline"}
        </span>
      </div>
      {offer.description && <p className="offer-description">{offer.description}</p>}
      <ul className="specs">
        <li>
          up to <strong>{offer.cpus}</strong> vCPU
        </li>
        <li>
          up to <strong>{offer.memory_mb}</strong> MB RAM
        </li>
        <li>
          <strong>{offer.disk_gb}</strong> GB disk
        </li>
        {offer.gpu && (
          <li>
            <strong>{offer.gpu}</strong>
          </li>
        )}
      </ul>
      <div className="price">
        <span className="price-amount">
          {tiaPrice(offer.price_utia_per_hour * 24)}
          <span className="price-unit"> / day</span>
        </span>
        <span className="muted">billed hourly, {tiaPrice(offer.price_utia_per_hour)} per hour</span>
      </div>
      <dl className="facts">
        <dt>Grace period</dt>
        <dd>{duration(offer.grace_seconds)} without payment, then wiped</dd>
        <dt>Slots</dt>
        <dd>
          {offer.free_slots} of {offer.slots} free
        </dd>
        {offer.attestation_url && (
          <>
            <dt>Enclave</dt>
            <dd>
              <a href={offer.attestation_url} target="_blank" rel="noreferrer">
                Verify the attestation
              </a>
            </dd>
          </>
        )}
        {offer.governance && (
          <>
            <dt>Code</dt>
            <dd>
              <CodeStatus governance={offer.governance} />
            </dd>
          </>
        )}
      </dl>
      <button className="primary" disabled={!available} onClick={onRent}>
        {!offer.online ? "Provider offline" : offer.free_slots === 0 ? "Fully rented" : "Rent"}
      </button>
    </article>
  );
}

/** Whether the offer's code is frozen, read from its app contract by this browser. */
function CodeStatus({ governance }: { governance: Governance }) {
  const control = usePolling(() => readCodeControl(governance), 60_000, governance.app_contract);
  const contract = (
    <a href={contractUrl(governance)} target="_blank" rel="noreferrer" className="mono">
      {shorten(governance.app_contract, 6)}
    </a>
  );
  const verdict = control.error ? (
    <span className="muted">Could not read the contract.</span>
  ) : !control.value ? (
    <span className="muted">Checking on chain.</span>
  ) : control.value.frozen ? (
    <span>
      <span className="frozen">Frozen.</span> Owner is 0x0, so nobody can change the code.
    </span>
  ) : (
    <span>Upgradeable by its owner {shorten(control.value.owner, 4)}.</span>
  );
  return (
    <span>
      Governed by contract {contract}. {verdict}
    </span>
  );
}

type Step =
  | { kind: "form" }
  | { kind: "working"; message: string }
  | { kind: "done"; lease: Lease; txHash: string }
  | { kind: "failed"; message: string };

function RentDialog({ offer, session, onClose }: { offer: Offer; session: Session; onClose: () => void }) {
  const { config } = session;
  const [sshKey, setSshKey] = useState("");
  const [step, setStep] = useState<Step>({ kind: "form" });
  const firstPayment = offer.price_utia_per_hour + config.escrow_fee_utia;

  const generate = async () => {
    try {
      const pair = await generateSshKey(`agentcloud-${offer.id}`);
      download(`agentcloud-${offer.id}.key`, pair.privateKey);
      setSshKey(pair.publicKey);
    } catch {
      setStep({ kind: "failed", message: "This browser cannot make Ed25519 keys. Paste a public key instead." });
    }
  };

  const rent = async () => {
    try {
      const renter = session.address ?? (await session.connect());
      setStep({ kind: "working", message: "Reserving the lease" });
      const { lease, payment } = await market.createLease(offer.id, renter, sshKey.trim());
      // The memo is what the enclave checks the key against, so it must be our key's.
      if (!payment.memo.endsWith(`:${await sshKeyCommitment(sshKey)}`)) {
        throw new Error("The market asked to commit to a different SSH key. Nothing was paid.");
      }
      setStep({ kind: "working", message: "Approve the first payment in Keplr" });
      const txHash = await sendTia(config.chain, renter, payment.to, payment.amount_utia, payment.memo);
      setStep({ kind: "working", message: "Waiting for the payment to land on chain" });
      setStep({ kind: "done", lease: await waitWhilePending(lease.id), txHash });
    } catch (e) {
      setStep({ kind: "failed", message: (e as Error).message });
    }
  };

  return (
    <div className="overlay" onClick={onClose}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <h2>Rent {offer.name}</h2>
          <button className="close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>

        {step.kind === "form" && (
          <>
            <label className="field">
              <span>Your SSH public key</span>
              <textarea
                rows={3}
                value={sshKey}
                onChange={(e) => setSshKey(e.target.value)}
                placeholder="ssh-ed25519 AAAA... you@laptop"
              />
            </label>
            <p className="hint">
              Paste the contents of <code>~/.ssh/id_ed25519.pub</code>, or{" "}
              <button className="linklike" onClick={generate}>
                generate a key here
              </button>
              . A generated private key downloads to your computer and never leaves it.
            </p>
            <dl className="summary">
              <dt>First payment</dt>
              <dd>
                {tia(firstPayment)}
                <span className="sub">
                  1 hour of rent plus a {tia(config.escrow_fee_utia)} escrow fee, refunded minus the fee if someone
                  else is faster
                </span>
              </dd>
              <dt>Then</dt>
              <dd>
                {tiaPrice(offer.price_utia_per_hour)} per hour
                <span className="sub">paid by your agent from its own wallet, which you fund on Manage agents</span>
              </dd>
              <dt>Ends</dt>
              <dd>
                {duration(offer.grace_seconds)} after payments stop
                <span className="sub">the sandbox and everything in it is deleted</span>
              </dd>
            </dl>
            <button className="primary" disabled={!sshKey.trim()} onClick={rent}>
              {session.address ? `Pay ${tia(firstPayment)} and rent` : "Connect Keplr and rent"}
            </button>
          </>
        )}

        {step.kind === "working" && <p className="wait">{step.message}</p>}

        {step.kind === "done" && <RentResult lease={step.lease} txHash={step.txHash} session={session} />}

        {step.kind === "failed" && (
          <>
            <p className="error">{step.message}</p>
            <button className="secondary" onClick={() => setStep({ kind: "form" })}>
              Back
            </button>
          </>
        )}
      </div>
    </div>
  );
}

function RentResult({ lease, txHash, session }: { lease: Lease; txHash: string; session: Session }) {
  const tx = (
    <a href={`${session.config.chain.explorer}/tx/${txHash}`} target="_blank" rel="noreferrer">
      View the payment
    </a>
  );
  if (lease.status === "active") {
    return (
      <>
        <p className="success">The lease is active. Your sandbox starts within a minute.</p>
        <p className="hint">
          Next, fund your agent's wallet so it can keep paying its rent. {tx}
        </p>
        <button className="primary" onClick={() => session.go("agents")}>
          Open Manage agents
        </button>
      </>
    );
  }
  if (lease.status === "pending") {
    return (
      <p className="wait">
        The payment was sent but the market has not seen it yet. Check Manage agents in a minute. {tx}
      </p>
    );
  }
  return (
    <p className="error">
      {lease.end_reason === "outbid"
        ? "Someone else rented the last slot first. Your payment is being refunded, minus the escrow fee."
        : `The lease ended (${lease.end_reason}).`}{" "}
      {tx}
    </p>
  );
}

/** Polls the lease until the market has seen its first payment, for up to two minutes. */
async function waitWhilePending(id: string): Promise<Lease> {
  const deadline = Date.now() + 120_000;
  for (;;) {
    const lease = await market.lease(id);
    if (lease.status !== "pending" || Date.now() > deadline) return lease;
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
}

function download(name: string, contents: string) {
  const url = URL.createObjectURL(new Blob([contents], { type: "application/x-pem-file" }));
  const link = Object.assign(document.createElement("a"), { href: url, download: name });
  link.click();
  URL.revokeObjectURL(url);
}
