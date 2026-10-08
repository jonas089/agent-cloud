"""Encrypts a KEY=VALUE env file for a dstack CVM, as Phala's tools do: an ephemeral X25519
key agrees a secret with the app's env key, which seals {"env": [{key, value}, ...]} with
AES-256-GCM. Prints ephemeral public key || IV || ciphertext as hex.

    python3 deploy/seal-env.py <app env pubkey hex> <env file>
"""

import json
import os
import sys

from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers.aead import AESGCM


def main() -> None:
    remote_hex, env_file = sys.argv[1], sys.argv[2]
    env = []
    for line in open(env_file):
        name, sep, value = line.strip().partition("=")
        if sep and name and not name.startswith("#"):
            env.append({"key": name, "value": value})
    ephemeral = X25519PrivateKey.generate()
    shared = ephemeral.exchange(X25519PublicKey.from_public_bytes(bytes.fromhex(remote_hex)))
    iv = os.urandom(12)
    sealed = AESGCM(shared).encrypt(iv, json.dumps({"env": env}).encode(), None)
    print((ephemeral.public_key().public_bytes_raw() + iv + sealed).hex())


if __name__ == "__main__":
    main()
