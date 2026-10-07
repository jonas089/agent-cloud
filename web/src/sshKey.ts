// An Ed25519 SSH key pair made in the browser, for renters without a key at hand. The private
// key never leaves the page: it is offered as a download and only the public half is sent to
// the market. Uses @noble/curves with the browser's random generator, which unlike WebCrypto's
// key functions also works on plain http.

import { ed25519 } from "@noble/curves/ed25519";

export interface SshKeyPair {
  publicKey: string;
  privateKey: string;
}

export async function generateSshKey(comment: string): Promise<SshKeyPair> {
  const seed = crypto.getRandomValues(new Uint8Array(32));
  const publicRaw = ed25519.getPublicKey(seed);
  const publicBlob = concat(sshString("ssh-ed25519"), sshString(publicRaw));
  return {
    publicKey: `ssh-ed25519 ${base64(publicBlob)} ${comment}`,
    privateKey: openSshPrivateKey(publicBlob, publicRaw, seed, comment),
  };
}

/** The unencrypted `openssh-key-v1` format that `ssh -i` reads. */
function openSshPrivateKey(publicBlob: Uint8Array, publicRaw: Uint8Array, seed: Uint8Array, comment: string): string {
  const check = crypto.getRandomValues(new Uint8Array(4));
  let secret = concat(
    check,
    check,
    sshString("ssh-ed25519"),
    sshString(publicRaw),
    sshString(concat(seed, publicRaw)),
    sshString(comment),
  );
  const padding = (8 - (secret.length % 8)) % 8;
  secret = concat(secret, Uint8Array.from({ length: padding }, (_, i) => i + 1));
  const body = concat(
    new TextEncoder().encode("openssh-key-v1\0"),
    sshString("none"),
    sshString("none"),
    sshString(new Uint8Array()),
    uint32(1),
    sshString(publicBlob),
    sshString(secret),
  );
  const lines = base64(body).match(/.{1,70}/g) ?? [];
  return ["-----BEGIN OPENSSH PRIVATE KEY-----", ...lines, "-----END OPENSSH PRIVATE KEY-----", ""].join("\n");
}

function sshString(value: string | Uint8Array): Uint8Array {
  const bytes = typeof value === "string" ? new TextEncoder().encode(value) : value;
  return concat(uint32(bytes.length), bytes);
}

function uint32(value: number): Uint8Array {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value);
  return bytes;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function base64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}
