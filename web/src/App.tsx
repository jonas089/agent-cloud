import { useEffect, useState } from "react";
import { market } from "./api";
import type { MarketConfig } from "./api";
import { shorten } from "./format";
import * as wallet from "./wallet";
import { MarketView } from "./views/MarketView";
import { GuideView } from "./views/GuideView";
import { AgentsView } from "./views/AgentsView";
import { AccountView } from "./views/AccountView";

const TABS = [
  { id: "market", label: "Market" },
  { id: "guide", label: "Getting started" },
  { id: "agents", label: "Manage agents" },
  { id: "account", label: "Account" },
] as const;

export type Tab = (typeof TABS)[number]["id"];

/** What every view gets: the market's settings and the connected wallet, if any. */
export interface Session {
  config: MarketConfig;
  address: string | null;
  connect: () => Promise<string>;
  go: (tab: Tab) => void;
}

function tabFromHash(): Tab {
  const id = window.location.hash.slice(1);
  return TABS.some((tab) => tab.id === id) ? (id as Tab) : "market";
}

export default function App() {
  const [config, setConfig] = useState<MarketConfig | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [address, setAddress] = useState<string | null>(null);
  const [walletError, setWalletError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(tabFromHash);

  useEffect(() => {
    market.config().then(setConfig, (e: Error) => setLoadError(e.message));
    const onHash = () => setTab(tabFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (config) wallet.restore(config.chain).then(setAddress);
  }, [config]);

  if (!config) {
    return <div className="boot">{loadError ? `The market is unreachable: ${loadError}` : "Loading the market"}</div>;
  }

  const connect = async () => {
    setWalletError(null);
    try {
      const connected = await wallet.connect(config.chain);
      setAddress(connected);
      return connected;
    } catch (e) {
      setWalletError((e as Error).message);
      throw e;
    }
  };
  const go = (next: Tab) => {
    window.location.hash = next;
    setTab(next);
  };
  const session: Session = { config, address, connect, go };

  return (
    <div className="app">
      <header className="topbar">
        <div className="topbar-inner">
          <a className="brand" href="#market" onClick={() => go("market")}>
            <img src="/mark.svg" alt="" width={26} height={26} />
            Confidential Agent Cloud
          </a>
          <nav className="tabs">
            {TABS.map((t) => (
              <a key={t.id} href={`#${t.id}`} className={`tab${t.id === tab ? " on" : ""}`}>
                {t.label}
              </a>
            ))}
          </nav>
          <div className="wallet-area">
            <span className="network">{config.chain.chain_id}</span>
            {address ? (
              <button
                className="wallet on"
                title="Disconnect"
                onClick={() => {
                  wallet.disconnect();
                  setAddress(null);
                }}
              >
                <span className="wallet-dot" />
                {shorten(address, 4)}
              </button>
            ) : (
              <button className="wallet" onClick={() => connect().catch(() => {})}>
                Connect Keplr
              </button>
            )}
          </div>
        </div>
        {walletError && (
          <div className="topbar-error" onClick={() => setWalletError(null)}>
            {walletError}
          </div>
        )}
      </header>
      <main className="page">
        {tab === "market" && <MarketView session={session} />}
        {tab === "guide" && <GuideView session={session} />}
        {tab === "agents" && <AgentsView session={session} />}
        {tab === "account" && <AccountView session={session} />}
      </main>
      <footer className="footer">
        Confidential agent compute in Intel TDX enclaves, paid by the hour in TIA on Celestia {config.chain.chain_id}. Escrow{" "}
        <a href={`${config.chain.explorer}/address/${config.escrow_address}`} target="_blank" rel="noreferrer">
          {shorten(config.escrow_address)}
        </a>
      </footer>
    </div>
  );
}
