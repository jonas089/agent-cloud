// Manage agents: the renter's leases, how to log in to each, and the wallet each agent pays
// its rent from. Cancelling is a transfer signed by the renter, so it needs Keplr too.

import { useState } from "react";
import { expiresAt, market, paidUntil, sendFee, sshCommand, sshConfig } from "../api";
import type { Account, Lease } from "../api";
import type { Session } from "../App";
import { duration, tia, tiaPrice, toUtia, when } from "../format";
import { CodeBlock, Copy, LeaseChip, useNow, usePolling } from "../ui";
import { sendTia } from "../wallet";

const END_REASON: Record<NonNullable<Lease["end_reason"]>, string> = {
  unpaid: "the first payment never arrived",
  outbid: "someone else took the last slot first, payment refunded",
  expired: "rent stopped for longer than the grace period",
  cancelled: "cancelled by you",
};

export function AgentsView({ session }: { session: Session }) {
  const { address } = session;
  const account = usePolling(() => (address ? market.account(address) : Promise.resolve(null)), 8_000, address);
  const offers = usePolling(market.offers, 30_000);

  if (!address) {
    return (
      <div className="panel narrow">
        <h2>Manage agents</h2>
        <p className="muted">Connect Keplr to see your sandboxes, how to reach them and their wallets.</p>
        <button className="primary" onClick={() => session.connect().catch(() => {})}>
          Connect Keplr
        </button>
      </div>
    );
  }

  const all = account.value?.leases ?? [];
  const live = all.filter((lease) => lease.status !== "ended");
  const ended = all.filter((lease) => lease.status === "ended");
  const offerName = (id: string) => offers.value?.find((offer) => offer.id === id)?.name ?? id;

  return (
    <div className="stack">
      {account.error && <p className="error">{account.error}</p>}

      <section>
        <h2 className="section-title">Your agents</h2>
        {account.value && live.length === 0 && (
          <div className="panel empty">
            Nothing running right now. <a href="#market">Start an agent</a>.
          </div>
        )}
        <div className="lease-grid">
          {live.map((lease) => (
            <LeaseCard
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

      {ended.length > 0 && (
        <section>
          <h2 className="section-title">Past leases</h2>
          <div className="panel table-panel">
            <table className="table">
              <thead>
                <tr>
                  <th>Lease</th>
                  <th>Machine</th>
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

interface LeaseCardProps {
  lease: Lease;
  name: string;
  account: Account;
  session: Session;
  onSent: () => void;
}

function LeaseCard({ lease, name, account, session, onSent }: LeaseCardProps) {
  const now = useNow();
  const [notice, setNotice] = useState<string | null>(null);
  const until = paidUntil(lease);
  const expires = expiresAt(lease);
  const wallet = account.agent_wallets.find((w) => w.lease_id === lease.id);
  const perHour = lease.price_utia_per_hour + sendFee(session.config.chain);

  const cancel = async () => {
    if (!window.confirm("End this lease now? The sandbox and everything in it are deleted.")) return;
    try {
      setNotice("Approve the cancellation in Keplr");
      await sendTia(session.config.chain, account.address, lease.payout_address, 1, `agentcloud:cancel:${lease.id}`);
      setNotice("Cancellation sent. The sandbox is wiped within a minute and the agent's wallet is returned to you.");
    } catch (e) {
      setNotice(`Could not cancel: ${(e as Error).message}`);
    }
  };

  return (
    <article className="panel lease">
      <div className="offer-head">
        <div>
          <h3>{name}</h3>
          <span className="muted mono">lease {lease.id}</span>
        </div>
        <LeaseChip lease={lease} />
      </div>

      {lease.status === "pending" && (
        <p className="wait">Waiting for the first payment. Unpaid leases lapse after 30 minutes.</p>
      )}

      {lease.status === "active" && (
        <>
          {lease.connection ? (
            <>
              <span className="label">Add to ~/.ssh/config</span>
              <CodeBlock code={sshConfig(lease.connection, lease.id, `~/Downloads/agentcloud-${lease.offer_id}.key`)} />
              <span className="label">Then connect and start an agent</span>
              <CodeBlock code={`ssh agentcloud-${lease.id}\nagent init openai`} />
              <p className="hint">
                Change <code>IdentityFile</code> if your key lives elsewhere. One-off without the config:{" "}
                <Copy text={sshCommand(lease.connection, `~/Downloads/agentcloud-${lease.offer_id}.key`)}>
                  <code>copy the full ssh command</code>
                </Copy>
                .
              </p>
            </>
          ) : (
            <p className="wait">The enclave is checking your key commitment on chain and starting your sandbox.</p>
          )}

          {wallet && (
            <FundWallet
              address={wallet.address}
              balance={wallet.balance_utia}
              hours={Math.floor(wallet.balance_utia / perHour)}
              session={session}
              from={account.address}
              onSent={onSent}
            />
          )}

          <dl className="facts">
            <dt>Paid until</dt>
            <dd>
              {until && (until > now ? `${when(until)}, ${duration(until - now)} left` : `${duration(now - until)} overdue`)}
            </dd>
            <dt>Wiped at</dt>
            <dd>{expires && `${when(expires)} unless paid, in ${duration(expires - now)}`}</dd>
            <dt>Rent</dt>
            <dd>{tiaPrice(lease.price_utia_per_hour)} per hour</dd>
            <dt>Paid so far</dt>
            <dd>{tia(lease.paid_utia)}</dd>
          </dl>
          {notice ? (
            <p className="notice">{notice}</p>
          ) : (
            <button className="danger" onClick={cancel}>
              Cancel lease
            </button>
          )}
        </>
      )}
    </article>
  );
}

interface FundWalletProps {
  address: string;
  balance: number;
  /** Whole hours of rent the balance pays for. */
  hours: number;
  session: Session;
  from: string;
  onSent: () => void;
}

/** The agent's wallet, with a Keplr top-up. */
function FundWallet({ address, balance, hours, session, from, onSent }: FundWalletProps) {
  const [amount, setAmount] = useState("2");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const send = async () => {
    try {
      setMessage(null);
      await sendTia(session.config.chain, from, address, toUtia(amount), "agentcloud agent wallet top-up");
      setMessage({ ok: true, text: "Sent. The balance updates once it lands." });
      onSent();
    } catch (e) {
      setMessage({ ok: false, text: (e as Error).message });
    }
  };

  return (
    <div className="agent-wallet">
      <span className="label">Agent wallet</span>
      <div className="agent-wallet-row">
        <Copy text={address} />
        <strong>{tia(balance)}</strong>
      </div>
      <p className={hours < 2 ? "error" : "hint"}>
        {hours === 0 ? "Not enough for the next hour. Fund it before the grace period runs out." : `Pays about ${duration(hours * 3600)} of rent.`}
      </p>
      <div className="form-row">
        <label className="field grow">
          <span>Top up in TIA</span>
          <input value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" />
        </label>
        <button className="primary inline" onClick={send}>
          Fund
        </button>
      </div>
      {message && <p className={message.ok ? "success" : "error"}>{message.text}</p>}
    </div>
  );
}
