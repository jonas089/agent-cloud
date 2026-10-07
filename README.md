# Confidential Agent Cloud

SSH sandboxes for AI agents inside an Intel TDX enclave on Phala Cloud, paid by the minute in
TIA on Celestia's Mocha testnet. Nobody can read what runs in a sandbox, the operator
included, and every agent gets a wallet of its own to pay its rent. There are no accounts: a
Celestia address is an identity.

## How a lease works

1. **Reserve.** The renter picks an offer and gives an SSH public key. The market creates a
   pending lease and answers with the first payment: one minute of rent plus an escrow fee, to
   the escrow, with the memo `agentcloud:activate:<lease>:<key commitment>`.
2. **Activate.** The renter signs it in Keplr. The first payment to land claims the slot and
   the escrow forwards it to the provider; one that finds the offer full is refunded minus the
   fee, so renters racing for the last slot are settled by block order.
3. **Start.** The provider, inside the enclave, checks on chain that the renter committed to
   the SSH key the market handed over, creates the agent's wallet, and starts the sandbox with
   the key and the wallet in it.
4. **Pay.** The enclave pays rent from the agent's wallet, one minute ahead, settling any debt
   after an outage in one transfer. Anyone can fund the wallet: the renter, or the agent.
5. **End.** When rent stops for longer than the grace period, or the renter sends
   `agentcloud:cancel:<lease>`, the sandbox is deleted and the wallet's balance goes back to
   the renter.

The market reads every transfer to the escrow and to the provider's payout address, and
judges expiry against the newest block it has fully read, so a slow node can delay an expiry
but never cause a wrong one.

## Layout

| path | what it is |
|---|---|
| `crates/protocol` | shared types: API bodies, lease accounting, memos, key commitments, agent request signing |
| `crates/chain` | minimal Celestia client: mnemonic wallets, bank sends, transfer search over REST |
| `crates/market` | the market: HTTP API, SQLite store, ledger, escrow, serves the web app |
| `crates/provider` | runs in the enclave: sandboxes, agent wallets, rent |
| `crates/provider/sandbox` | the sandbox image: Dockerfile, entrypoint, the `agent` command, the agent template |
| `web` | React app: Market, Getting started, Manage agents, Account |
| `deploy` | the provider's image, its Phala compose file, and the script that deploys it |
| `config` | commented example market config |
| `install.sh` | installs or updates the market as a systemd service |

## Run

```sh
./install.sh --port 8420                                         # the market, on a Linux server
deploy/phala.sh --market http://<market>:8420 --payout <celestia address>   # the enclave
```

`install.sh` needs Rust and Node.js 18+, builds only when the code changed, and refuses a
port that is taken. `deploy/phala.sh` needs Docker with buildx and the `phala` CLI, and reads
the Phala API key from `PHALA_CLOUD_API_KEY` or `data/phala.key`. It builds the provider
image, pushes it, pins it by digest in the compose file and deploys a production CVM, or
updates the one recorded in `data/phala-app-id`.

## Move the market to another server (e.g. ark)

The market is one service with its state in `data/market/` (the SQLite ledger and the escrow
key); the enclave only needs its URL. Nothing else on the target server is touched.

```sh
# on the new server: pick a free port first
ss -ltn | grep -q ':8420 ' && echo "8420 is taken, pick another"
git clone <repo> agentcloud && cd agentcloud

# on the old server: stop the market and hand over its state, keeping escrow funds and leases
sudo systemctl stop agentcloud-market
rsync -a data/market/ <new-server>:agentcloud/data/market/

# on the new server
./install.sh --port 8420
# here: point the enclave at the new market (updates the recorded CVM in place)
deploy/phala.sh --market http://<new-server>:8420
```

The CVM restarts with the new compose; the provider remounts every home and starts every
sandbox again, so running agents lose a minute, not their data. Leases keep their prepaid time.

## Develop

```sh
cargo run -p agentcloud-market             # http://127.0.0.1:8420, config in data/market.toml
(cd web && npm install && npm run dev)     # hot-reloading UI on :5173, proxying /api
cargo test && (cd web && npm test)
```

## Inside a sandbox

Ubuntu with Python, Node 22, Claude Code, Codex, Gemini CLI and the Anthropic, OpenAI, Google
and Mistral SDKs. The `agent` command does the rest:

```sh
agent init openai                  # or claude, gemini, grok, deepseek, openrouter, mistral,
                                   #    claude-code, codex, gemini-cli
agent secret set OPENAI_API_KEY    # stored in ~/.env, never echoed
agent logs -f                      # agent status | restart | stop | once | wallet
git push <sandbox>:app.git main    # or deploy any repo with an agent.sh
```

`~/app/agent.sh` runs on boot and is restarted when it exits, with backoff on crash loops.

## Resources

Ten sandboxes share one `tdx.small` CVM (1 vCPU, 2 GB). Each has its own user id, a read-only
root, a 3 GB home, a 128 MB in-memory `/tmp`, at most 1 core and 768 MB with 128 MB guaranteed,
equal CPU shares, 512 processes, and no network path to the host, private networks or the other
sandboxes (DNS to the host's resolvers excepted). Where the kernel has loop devices the home is
a fixed-size ext4 image; a Phala CVM has none, so there the quota is soft: a home over 3 GB gets
its agent stopped and a `DISK_QUOTA_EXCEEDED.txt` notice until the renter makes room.
Under memory pressure the sandbox using most is killed first; the provider never is. That
fits API-driven agents (trading bots, monitors, chat and research loops, Claude Code or Codex
on single tasks), not local models.

## Trust

- **The enclave** runs a production image with no shell. Its compose file, measured into the
  attestation, pins the provider image by digest and carries the whole provider config.
- **Sandboxes** run as an unprivileged user with every capability dropped, no privilege
  escalation, CPU, memory and process limits, on a network without traffic between them.
  Their files, agent wallets and anything renters copy in (API keys) live only in enclave
  memory and on its encrypted disk.
- **The market** is not trusted with secrets. It only relays public keys, and cannot swap a
  renter's key for its own: the enclave installs a key only if the renter's signed first
  payment commits to it.
- **The escrow** is the market's only custodial piece and only ever holds first payments in
  flight. Its key is `data/market/escrow.mnemonic` and must not be used for anything else:
  payouts rely on owning the account's sequence so they never pay twice.
