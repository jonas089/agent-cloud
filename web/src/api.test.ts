import { expect, test } from "vitest";
import { sshKeyCommitment } from "./api";

test("SSH key commitment matches the Rust implementation and ignores the comment", async () => {
  // Same vector as `key_commitment_ignores_the_comment` in crates/protocol/src/api.rs.
  const a = await sshKeyCommitment("ssh-ed25519 AAAAC3Nz you@laptop");
  expect(a).toBe(await sshKeyCommitment("ssh-ed25519   AAAAC3Nz other"));
  expect(a).toBe("460a4843966ebd8c680179a576c8f689");
});
