// Manage agents: each running agent with how to reach it and how long it is paid for, the
// reservations still waiting for their first payment, and past leases. Paying, funding and
// cancelling are transfers signed in Keplr.

import { useState } from "react";
import { expiresAt, market, paidUntil, sshCommand, sshConfig, sshKeyCommitment } from "../api";
import type { Account, Lease } from "../api";
import type { Session } from "../App";
import { duration, shorten, tia, tiaPrice, toUtia, when } from "../format";
import { CodeBlock, Copy, useNow, usePolling } from "../ui";
import { sendTia } from "../wallet";

const END_REASON: Record<NonNullable<Lease["end_reason"]>, string> = {
  unpaid: "never paid",
  outbid: "slot taken first, refunded",
  expired: "rent ran out",
  cancelled: "cancelled",
};

/** Where a key generated on the Market tab is saved. */
const keyFile = (lease: Lease) => `~/Downloads/agentcloud-${lease.offer_id}.key`;

export function AgentsView({ session }: { session: Session }) {
  const { address } = session;
  const account = usePolling(() => (address ? market.account(address) : Promise.resolve(null)), 8_000, address);
  const offers = usePolling(market.offers, 30_000);

  if (!address) {
    return (
      <div className="panel narrow">
        <h2>Manage agents</h2>
        <p className="muted">Connect Keplr to see your agents, how to reach them and their wallets.</p>
        <button className="primary" onClick={() => session.connect().catch(() => {})}>
          Connect Keplr
        </button>
      </div>
    );
  }

  const leases = account.value?.leases ?? [];
  const active = leases.filter((lease) => lease.status === "active");
  const pending = leases.filter((lease) => lease.status === "pending");
  const ended = leases.filter((lease) => lease.status === "ended");
  const offerName = (id: string) => offers.value?.find((offer) => offer.id === id)?.name ?? "Sandbox";

  return (
    <div className="stack">
      {account.error && <p className="error">{account.error}</p>}

      <section>
        <h2 className="section-title">Your agents</h2>
        {account.value && active.length === 0 && (
          <div className="panel empty">
            No agent running. <a href="#market">Rent a sandbox</a> to start one.
          </div>
        )}
        <div className="stack">
          {active.map((lease) => (
            <AgentCard
              key={lease.id}
              lease={lease}
              name={offerName(lease.offer_id)}
              account={account.value!}
              session={session}
              onSent={account.reload}
            />
          ))}
        </div>
      </section>

      {pending.length > 0 && (
        <section>
          <h2 className="section-title">Waiting for payment</h2>
          <div className="panel list-panel">
            {pending.map((lease) => (
              <PendingRow key={lease.id} lease={lease} name={offerName(lease.offer_id)} session={session} />
            ))}
          </div>
        </section>
      )}

      {ended.length > 0 && (
        <section>
          <h2 className="section-title">Past leases</h2>
          <div className="panel table-panel">
            <table className="table">
              <thead>
                <tr>
                  <th>Lease</th>
                  <th>Sandbox</th>
                  <th>Paid</th>
                  <th>Ended</th>
                  <th>Why</th>
                </tr>
              </thead>
              <tbody>
                {ended.map((lease) => (
                  <tr key={lease.id}>
                    <td className="mono">{lease.id}</td>
                    <td>{offerName(lease.offer_id)}</td>
                    <td>{tia(lease.paid_utia)}</td>
                    <td>{lease.ended_at ? when(lease.ended_at) : ""}</td>
                    <td className="muted">{lease.end_reason ? END_REASON[lease.end_reason] : ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  );
}

interface AgentCardProps {
  lease: Lease;
  name: string;
  account: Account;
  session: Session;
  onSent: () => void;
}

function AgentCard({ lease, name, account, session, onSent }: AgentCardProps) {
  const now = useNow();
  const [notice, setNotice] = useState<string | null>(null);
  const until = paidUntil(lease) ?? now;
  const expires = expiresAt(lease) ?? now;
  const wallet = account.agent_wallets.find((w) => w.lease_id === lease.id);
  const health = until - now > 600 ? "good" : now < until ? "due" : "grace";

  const cancel = async () => {
    if (!window.confirm("End this lease now? The sandbox and everything in it are deleted.")) return;
    try {
      setNotice("Approve the cancellation in Keplr");
      await sendTia(session.config.chain, account.address, lease.payout_address, 1, `agentcloud:cancel:${lease.id}`);
      setNotice("Cancelled. The sandbox is wiped within a minute and the wallet's balance comes back to you.");
    } catch (e) {
      setNotice(`Could not cancel: ${(e as Error).message}`);
    }
  };

  return (
    <article className="panel agent">
      <header className="agent-head">
        <div>
          <h3>{name}</h3>
          <span className="muted mono">lease {lease.id}</span>
        </div>
        <span className={`status ${health === "good" ? "status-active" : "status-warn"}`}>
          {health === "good" ? "Running" : health === "due" ? "Rent due" : "In grace period"}
        </span>
      </header>

      <div className="agent-grid">
        <div className="agent-connect">
          {lease.connection ? (
            <>
              <ol className="steps">
                <li>
                  <span>
                    Add this to <code>~/.ssh/config</code>
                  </span>
                  <CodeBlock code={sshConfig(lease.connection, lease.id, keyFile(lease))} />
                </li>
                <li>
                  <span>Connect, pick a model and paste its API key</span>
                  <CodeBlock code={`ssh agentcloud-${lease.id}\nagent init openai\nagent secret set OPENAI_API_KEY`} />
                </li>
                <li>
                  <span>
                    Put the job in <code>~/app/TASK.md</code> and follow it with <code>agent logs -f</code>
                  </span>
                </li>
              </ol>
              <p className="hint">
                Key saved elsewhere? Change <code>IdentityFile</code>. No config file?{" "}
                <Copy text={sshCommand(lease.connection, keyFile(lease))}>
                  <span className="linklike">Copy a one-line ssh command</span>
                </Copy>
              </p>
            </>
          ) : (
            <p className="wait">Checking your key on chain and starting the sandbox. This takes a few seconds.</p>
          )}
        </div>

        <div className="agent-rent">
          <RentMeter until={until} now={now} expires={expires} />
          <dl className="facts">
            <dt>Paid until</dt>
            <dd>{when(until)}</dd>
            <dt>Wiped if unpaid</dt>
            <dd>{when(expires)}</dd>
            <dt>Rent</dt>
            <dd>{tiaPrice(lease.price_utia_per_hour)} per hour</dd>
            <dt>Paid so far</dt>
            <dd>{tiaPrice(lease.paid_utia)}</dd>
          </dl>
          {wallet && (
            <FundWallet
              address={wallet.address}
              balance={wallet.balance_utia}
              hourly={lease.price_utia_per_hour + (session.config.typical_fee_utia ?? 0)}
              session={session}
              from={account.address}
              onSent={onSent}
            />
          )}
          {notice ? (
            <p className="notice">{notice}</p>
          ) : (
            <button className="linklike danger-link" onClick={cancel}>
              Cancel lease
            </button>
          )}
        </div>
      </div>
    </article>
  );
}

/** How much of the prepaid hour is left, then how much of the grace period. */
function RentMeter({ until, now, expires }: { until: number; now: number; expires: number }) {
  const left = until - now;
  const share = Math.max(0, Math.min(1, left / 3600));
  return (
    <div className="meter">
      <div className="meter-head">
        <span className="label">Prepaid time</span>
        <strong>{left > 0 ? `${duration(left)} left` : `grace ends in ${duration(Math.max(0, expires - now))}`}</strong>
      </div>
      <div className="meter-track">
        <div className={`meter-fill${left <= 600 ? " low" : ""}`} style={{ width: `${share * 100}%` }} />
      </div>
    </div>
  );
}

interface FundWalletProps {
  address: string;
  balance: number;
  /** Rent plus gas for one hour. */
  hourly: number;
  session: Session;
  from: string;
  onSent: () => void;
}

/** The wallet the agent pays its rent from, with a Keplr top-up. */
function FundWallet({ address, balance, hourly, session, from, onSent }: FundWalletProps) {
  const [amount, setAmount] = useState("2");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const hours = Math.floor(balance / hourly);

  const send = async () => {
    try {
      setMessage({ ok: true, text: "Approve the transfer in Keplr" });
      await sendTia(session.config.chain, from, address, toUtia(amount), "agentcloud agent wallet top-up");
      setMessage({ ok: true, text: "Sent. The balance updates in a few seconds." });
      onSent();
    } catch (e) {
      setMessage({ ok: false, text: (e as Error).message });
    }
  };

  return (
    <div className="agent-wallet">
      <div className="agent-wallet-head">
        <span className="label">Agent wallet</span>
        <Copy text={address}>
          <code>{shorten(address, 6)}</code>
        </Copy>
      </div>
      <div className="agent-wallet-balance">
        <strong>{tiaPrice(balance)}</strong>
        <span className={hours < 2 ? "runway low" : "runway"}>
          {hours === 0 ? "not enough for the next hour" : `about ${duration(hours * 3600)} of rent`}
        </span>
      </div>
      <div className="fund-row">
        <input value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" aria-label="TIA to send" />
        <span className="unit">TIA</span>
        <button className="primary inline" onClick={send}>
          Fund
        </button>
      </div>
      {message && <p className={message.ok ? "hint" : "error"}>{message.text}</p>}
    </div>
  );
}

/** A reservation whose first payment has not landed: pay it now, or let it lapse. */
function PendingRow({ lease, name, session }: { lease: Lease; name: string; session: Session }) {
  const now = useNow();
  const [state, setState] = useState<string | null>(null);
  const lapses = lease.created_at + 30 * 60;
  const amount = lease.price_utia_per_hour + session.config.escrow_fee_utia;

  const pay = async () => {
    if (!session.address) return;
    try {
      setState("Approve the payment in Keplr");
      const memo = `agentcloud:activate:${lease.id}:${await sshKeyCommitment(lease.ssh_key)}`;
      await sendTia(session.config.chain, session.address, session.config.escrow_address, amount, memo);
      setState("Paid. The sandbox starts within a minute.");
    } catch (e) {
      setState(`Payment failed: ${(e as Error).message}`);
    }
  };

  return (
    <div className="pending-row">
      <div className="pending-main">
        <strong>{name}</strong> <span className="muted mono">lease {lease.id}</span>
        <div className="muted small">
          Reserved {when(lease.created_at)}, lapses {lapses > now ? `in ${duration(lapses - now)}` : "now"}
        </div>
      </div>
      {state ? (
        <span className="small pending-state">{state}</span>
      ) : (
        <button className="secondary inline" onClick={pay}>
          Pay {tiaPrice(amount)} now
        </button>
      )}
    </div>
  );
}
