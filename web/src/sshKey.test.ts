import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vitest";
import { generateSshKey } from "./sshKey";

test("OpenSSH reads the generated private key and derives the same public key", async () => {
  const pair = await generateSshKey("test@agentcloud");
  const file = join(mkdtempSync(join(tmpdir(), "agentcloud-")), "key");
  writeFileSync(file, pair.privateKey, { mode: 0o600 });
  const derived = execFileSync("ssh-keygen", ["-y", "-f", file]).toString().trim();
  expect(pair.publicKey.startsWith(derived)).toBe(true);
});
