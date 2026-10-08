// Account: what the wallet holds, what its agents cost per hour, how long their wallets last
// at that rate, and every payment the market has seen for this wallet's leases.

import { market } from "../api";
import type { Session } from "../App";
import { duration, shorten, tia, tiaPrice, when } from "../format";
import { Copy, ExplorerLink, Stat, usePolling } from "../ui";

export function AccountView({ session }: { session: Session }) {
  const { address } = session;
  const account = usePolling(() => (address ? market.account(address) : Promise.resolve(null)), 15_000, address);

  if (!address) {
    return (
      <div className="panel narrow">
        <h2>Account</h2>
        <p className="muted">Your Celestia address is your account. Connect Keplr to see balances and spending.</p>
        <button className="primary" onClick={() => session.connect().catch(() => {})}>
          Connect Keplr
        </button>
      </div>
    );
  }
  if (!account.value) return <p className={account.error ? "error" : "wait"}>{account.error ?? "Loading your account"}</p>;

  const a = account.value;
  const burn = a.rent_utia_per_hour + a.gas_utia_per_hour;
  const agentFunds = a.agent_wallets.reduce((sum, wallet) => sum + wallet.balance_utia, 0);
  const explorer = session.config.chain.explorer;

  return (
    <div className="stack">
      <div className="stats">
        <Stat label="Wallet balance" value={tia(a.balance_utia)} sub={<Copy text={a.address}>{shorten(a.address)}</Copy>} />
        <Stat
          label="Burn rate"
          value={`${tiaPrice(burn)} / hour`}
          sub={`${tiaPrice(a.rent_utia_per_hour)} rent and ${tiaPrice(a.gas_utia_per_hour)} gas`}
        />
        <Stat label="Per day" value={tiaPrice(burn * 24)} sub={`${a.leases.filter((l) => l.status === "active").length} active leases`} />
        <Stat
          label="Agents last"
          value={burn === 0 ? "No spending" : agentFunds === 0 ? "Unfunded" : duration((agentFunds / burn) * 3600)}
          sub={`${tia(agentFunds)} across ${a.agent_wallets.length} agent wallet${a.agent_wallets.length === 1 ? "" : "s"}`}
        />
      </div>

      <section>
        <h2 className="section-title">Payments</h2>
        <div className="panel table-panel">
          {a.payments.length === 0 ? (
            <p className="empty">No payments yet.</p>
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>When</th>
                  <th>Amount</th>
                  <th>From</th>
                  <th>To</th>
                  <th>Lease</th>
                  <th>Result</th>
                </tr>
              </thead>
              <tbody>
                {a.payments.map((payment, index) => (
                  <tr key={`${payment.tx_hash}-${index}`}>
                    <td>
                      <ExplorerLink base={explorer} kind="tx" id={payment.tx_hash}>
                        {when(payment.time)}
                      </ExplorerLink>
                    </td>
                    <td>{tia(payment.amount_utia)}</td>
                    <td className="mono">{payment.sender === a.address ? "you" : shorten(payment.sender, 4)}</td>
                    <td className="mono">{shorten(payment.recipient, 4)}</td>
                    <td className="mono">{payment.lease_id}</td>
                    <td className="muted">{payment.outcome}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </section>
    </div>
  );
}
