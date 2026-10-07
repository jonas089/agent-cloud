// Small pieces every view uses.

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { Lease } from "./api";

/** Runs `load` now, whenever `key` changes, and every `everyMs`, keeping the last good value. */
export function usePolling<T>(load: () => Promise<T>, everyMs: number, key: unknown = null) {
  const [value, setValue] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloads, setReloads] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    let live = true;
    const run = () =>
      loadRef.current().then(
        (next) => live && (setValue(next), setError(null)),
        (e: Error) => live && setError(e.message),
      );
    run();
    const timer = setInterval(run, everyMs);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [everyMs, key, reloads]);
  return { value, error, reload: () => setReloads((n) => n + 1) };
}

/** A clock that ticks every second, for countdowns. */
export function useNow(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

export function Copy({ text, children }: { text: string; children?: ReactNode }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={`copy${copied ? " copied" : ""}`}
      title="Copy"
      onClick={() => {
        navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1400);
        });
      }}
    >
      {children ?? <code>{text}</code>}
    </button>
  );
}

export function CodeBlock({ code }: { code: string }) {
  return (
    <div className="code">
      <pre>{code}</pre>
      <Copy text={code}>
        <code>Copy</code>
      </Copy>
    </div>
  );
}

const STATUS_LABEL: Record<Lease["status"], string> = { pending: "Awaiting payment", active: "Active", ended: "Ended" };

export function LeaseChip({ lease }: { lease: Lease }) {
  return <span className={`status status-${lease.status}`}>{STATUS_LABEL[lease.status]}</span>;
}

export function Stat({ label, value, sub }: { label: string; value: ReactNode; sub?: ReactNode }) {
  return (
    <div className="stat">
      <span className="stat-label">{label}</span>
      <span className="stat-value">{value}</span>
      {sub && <span className="stat-sub">{sub}</span>}
    </div>
  );
}

export function ExplorerLink({ base, kind, id, children }: { base: string; kind: "tx" | "address"; id: string; children?: ReactNode }) {
  return (
    <a href={`${base}/${kind}/${id}`} target="_blank" rel="noreferrer">
      {children ?? id}
    </a>
  );
}
