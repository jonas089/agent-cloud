// Whether an offer's code can still change, read straight from its app contract on chain by
// the visitor's own browser, so the answer does not depend on trusting the market.
//
// Phala's on-chain KMS only releases a CVM's keys to code its app contract allows. The
// contract's owner can allow new code; once ownership is renounced the owner is the zero
// address and nobody can.

import type { Governance } from "./api";

export interface CodeControl {
  /** The contract's owner: the zero address once ownership is renounced. */
  owner: string;
  frozen: boolean;
}

const ZERO = "0x0000000000000000000000000000000000000000";
/** `owner()` */
const OWNER_SELECTOR = "0x8da5cb5b";

export async function readCodeControl(governance: Governance): Promise<CodeControl> {
  const response = await fetch(governance.rpc, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "eth_call",
      params: [{ to: governance.app_contract, data: OWNER_SELECTOR }, "latest"],
    }),
  });
  const { result, error } = await response.json();
  if (error || typeof result !== "string") throw new Error(error?.message ?? "no answer from the chain");
  const owner = `0x${result.slice(-40)}`;
  return { owner, frozen: owner.toLowerCase() === ZERO };
}

export function contractUrl(governance: Governance): string {
  return `${governance.explorer}/address/${governance.app_contract}`;
}
