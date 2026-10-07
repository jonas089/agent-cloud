// How amounts, addresses and times are written across the app.

const UTIA_PER_TIA = 1_000_000;

/** `12.5 TIA`, `0.00001 TIA`: as many decimals as the amount needs, at most six. */
export function tia(utia: number): string {
  const value = utia / UTIA_PER_TIA;
  return `${value.toLocaleString("en-US", { maximumFractionDigits: 6 })} TIA`;
}

/** A price, to four significant digits: `2 TIA`, `0.08333 TIA`. Exact amounts use [`tia`]. */
export function tiaPrice(utia: number): string {
  const value = Number((utia / UTIA_PER_TIA).toPrecision(4));
  return `${value.toLocaleString("en-US", { maximumFractionDigits: 6 })} TIA`;
}

export function toUtia(tiaAmount: string): number {
  const value = Number(tiaAmount);
  if (!Number.isFinite(value) || value <= 0) throw new Error("Enter an amount above zero");
  return Math.round(value * UTIA_PER_TIA);
}

/** `celestia1abc…wxyz` */
export function shorten(address: string, keep = 6): string {
  return address.length <= keep * 2 + 9 ? address : `${address.slice(0, keep + 9)}…${address.slice(-keep)}`;
}

/** `45s`, `12m`, `3h 20m`, `2d 4h` */
export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

/** A clock reading today, a date otherwise. */
export function when(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const today = new Date().toDateString() === date.toDateString();
  return date.toLocaleString([], today ? { hour: "2-digit", minute: "2-digit" } : { dateStyle: "medium", timeStyle: "short" });
}

export function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}
