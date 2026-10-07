// Keplr is the only wallet: a Celestia address is the user's whole account. Transactions are
// signed in Keplr and broadcast through the market, because the public nodes send no CORS
// headers and a browser cannot reach them directly.

import { MsgSend } from "cosmjs-types/cosmos/bank/v1beta1/tx";
import { PubKey } from "cosmjs-types/cosmos/crypto/secp256k1/keys";
import { SignMode } from "cosmjs-types/cosmos/tx/signing/v1beta1/signing";
import { AuthInfo, SignDoc, TxBody, TxRaw } from "cosmjs-types/cosmos/tx/v1beta1/tx";
import { gasForSend, market } from "./api";
import type { ChainConfig } from "./api";

declare global {
  interface Window {
    keplr?: {
      enable(chainId: string): Promise<void>;
      experimentalSuggestChain(info: unknown): Promise<void>;
      getOfflineSigner(chainId: string): DirectSigner;
    };
  }
}

/** The part of Keplr's offline signer used here: account keys and SIGN_MODE_DIRECT. */
interface DirectSigner {
  getAccounts(): Promise<readonly { address: string; pubkey: Uint8Array }[]>;
  signDirect(address: string, doc: SignDoc): Promise<{ signed: SignDoc; signature: { signature: string } }>;
}

const REMEMBERED = "agentcloud-keplr-connected";

/** Asks Keplr for the account, adding the chain to Keplr first if it does not know it. */
export async function connect(chain: ChainConfig): Promise<string> {
  const keplr = window.keplr;
  if (!keplr) throw new Error("Keplr is not installed. Get it at keplr.app, then reload this page.");
  try {
    await keplr.enable(chain.chain_id);
  } catch {
    await keplr.experimentalSuggestChain(chainInfo(chain));
    await keplr.enable(chain.chain_id);
  }
  const address = await firstAddress(chain);
  remember(true);
  return address;
}

/** Reconnects silently if this site was connected before; never opens a Keplr prompt. */
export async function restore(chain: ChainConfig): Promise<string | null> {
  if (!window.keplr || !recalled()) return null;
  try {
    await window.keplr.enable(chain.chain_id);
    return await firstAddress(chain);
  } catch {
    return null;
  }
}

export function disconnect(): void {
  remember(false);
}

/** Signs a TIA transfer in Keplr and broadcasts it. Resolves with the transaction hash. */
export async function sendTia(chain: ChainConfig, from: string, to: string, amountUtia: number, memo: string): Promise<string> {
  if (!window.keplr) throw new Error("Keplr is not installed");
  const signer = window.keplr.getOfflineSigner(chain.chain_id);
  const account = (await signer.getAccounts()).find((a) => a.address === from);
  if (!account) throw new Error("Keplr is connected to another account; reconnect it");
  const { account_number, sequence } = await market.chainAccount(from);
  const recipientExists = await market.chainAccount(to).then(() => true, () => false);
  const gas = gasForSend(chain, sequence, recipientExists);

  const send = MsgSend.fromPartial({ fromAddress: from, toAddress: to, amount: [{ denom: chain.denom, amount: String(amountUtia) }] });
  const bodyBytes = TxBody.encode(
    TxBody.fromPartial({ messages: [{ typeUrl: "/cosmos.bank.v1beta1.MsgSend", value: MsgSend.encode(send).finish() }], memo }),
  ).finish();
  const authInfoBytes = AuthInfo.encode(
    AuthInfo.fromPartial({
      signerInfos: [
        {
          publicKey: { typeUrl: "/cosmos.crypto.secp256k1.PubKey", value: PubKey.encode({ key: account.pubkey }).finish() },
          modeInfo: { single: { mode: SignMode.SIGN_MODE_DIRECT } },
          sequence: BigInt(sequence),
        },
      ],
      fee: {
        amount: [{ denom: chain.denom, amount: String(Math.ceil(gas * chain.gas_price)) }],
        gasLimit: BigInt(gas),
      },
    }),
  ).finish();
  const doc = SignDoc.fromPartial({ bodyBytes, authInfoBytes, chainId: chain.chain_id, accountNumber: BigInt(account_number) });
  const { signed, signature } = await signer.signDirect(from, doc);

  const raw = TxRaw.fromPartial({
    bodyBytes: signed.bodyBytes,
    authInfoBytes: signed.authInfoBytes,
    signatures: [Uint8Array.from(atob(signature.signature), (c) => c.charCodeAt(0))],
  });
  const { tx_hash } = await market.broadcast(btoa(String.fromCharCode(...TxRaw.encode(raw).finish())));
  return tx_hash;
}

async function firstAddress(chain: ChainConfig): Promise<string> {
  const [account] = await window.keplr!.getOfflineSigner(chain.chain_id).getAccounts();
  if (!account) throw new Error("Keplr returned no account");
  return account.address;
}

function chainInfo(chain: ChainConfig) {
  const currency = { coinDenom: "TIA", coinMinimalDenom: chain.denom, coinDecimals: 6 };
  const prefix = chain.bech32_prefix;
  return {
    chainId: chain.chain_id,
    chainName: `Celestia ${chain.chain_id}`,
    rpc: chain.rpc,
    rest: chain.rest,
    bip44: { coinType: 118 },
    bech32Config: {
      bech32PrefixAccAddr: prefix,
      bech32PrefixAccPub: `${prefix}pub`,
      bech32PrefixValAddr: `${prefix}valoper`,
      bech32PrefixValPub: `${prefix}valoperpub`,
      bech32PrefixConsAddr: `${prefix}valcons`,
      bech32PrefixConsPub: `${prefix}valconspub`,
    },
    currencies: [currency],
    feeCurrencies: [{ ...currency, gasPriceStep: { low: chain.gas_price, average: chain.gas_price, high: chain.gas_price * 2 } }],
    stakeCurrency: currency,
  };
}

function remember(connected: boolean): void {
  try {
    if (connected) localStorage.setItem(REMEMBERED, "1");
    else localStorage.removeItem(REMEMBERED);
  } catch {
    // Without storage the wallet still works; it just asks again after a reload.
  }
}

function recalled(): boolean {
  try {
    return localStorage.getItem(REMEMBERED) === "1";
  } catch {
    return false;
  }
}
