#!/usr/bin/env bash
# Installs the agentcloud market (API, ledger, escrow and web app) as the systemd service
# agentcloud-market, or updates it.
#
#   ./install.sh [--port 8420] [--bind 0.0.0.0]
#
# Running it again rebuilds and restarts only when the code changed. It touches nothing
# outside this checkout except its own unit file, and refuses a port that is already taken.
# State and config live in ./data, which updates never overwrite. The sandboxes themselves run
# in a Phala CVM, deployed with deploy/phala.sh.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA="$ROOT/data"
CONFIG="$DATA/market.toml"
UNIT=/etc/systemd/system/agentcloud-market.service
PORT=""
BIND=""

usage() { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
say() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "$1 is required. $2"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --bind) BIND="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown option $1" >&2; usage 1 ;;
  esac
done

[[ $(uname -s) == Linux ]] && command -v systemctl >/dev/null \
  || die "install.sh sets up a systemd service on Linux. Elsewhere, run: cargo run -p agentcloud-market"
[[ -z ${SUDO_USER:-} ]] || die "run this without sudo, as the user the market should run as. It asks for sudo itself."
need cargo "Install Rust with: curl https://sh.rustup.rs -sSf | sh"
need npm "Install Node.js 18 or newer."
if [[ $EUID -eq 0 ]]; then SUDO=""; else need sudo "Install sudo or run as root."; SUDO="sudo"; fi
mkdir -p "$DATA/bin"

# ------------------------------------------------------------------------------------- config

if [[ ! -f $CONFIG ]]; then
  sed -e "s|^listen = .*|listen = \"${BIND:-0.0.0.0}:${PORT:-8420}\"|" \
      -e "s|^data_dir = .*|data_dir = \"$DATA/market\"|" \
      -e "s|^web_dir = .*|web_dir = \"$ROOT/web/dist\"|" \
      "$ROOT/config/market.toml.example" > "$CONFIG"
  say "wrote $CONFIG"
elif [[ -n $PORT || -n $BIND ]]; then
  current="$(sed -n 's/^listen = "\(.*\)"/\1/p' "$CONFIG")"
  sed -i "s|^listen = .*|listen = \"${BIND:-${current%:*}}:${PORT:-${current##*:}}\"|" "$CONFIG"
fi
listen="$(sed -n 's/^listen = "\(.*\)"/\1/p' "$CONFIG")"
PORT="${listen##*:}"

# Never take a port another program already holds.
if ! systemctl is-active --quiet agentcloud-market && ss -ltnH "sport = :$PORT" | grep -q .; then
  die "port $PORT is already in use. Pick another with --port."
fi

# -------------------------------------------------------------------------------------- build

# A hash of everything that goes into the build, so unchanged code is not rebuilt or restarted.
fingerprint="$(cd "$ROOT" && find Cargo.toml Cargo.lock crates web/src web/public web/index.html web/package.json \
  web/package-lock.json web/vite.config.ts -type f | LC_ALL=C sort | xargs sha256sum | sha256sum | cut -c1-16)"
stamp="$DATA/.installed-market"
changed=no
if [[ -x $DATA/bin/agentcloud-market && -f $stamp && $(cat "$stamp") == "$fingerprint" ]]; then
  say "agentcloud-market is up to date"
else
  say "building the web app"
  (cd "$ROOT/web" && npm ci --no-audit --no-fund --loglevel=error && npm run build --silent)
  say "building agentcloud-market"
  (cd "$ROOT" && cargo build --release --locked -p agentcloud-market)
  install -m 755 "$ROOT/target/release/agentcloud-market" "$DATA/bin/agentcloud-market.new"
  mv -f "$DATA/bin/agentcloud-market.new" "$DATA/bin/agentcloud-market"
  echo "$fingerprint" > "$stamp"
  changed=yes
fi

# ------------------------------------------------------------------------------------ service

unit="[Unit]
Description=agentcloud market
After=network-online.target
Wants=network-online.target

[Service]
User=$(id -un)
WorkingDirectory=$ROOT
ExecStart=$DATA/bin/agentcloud-market --config $CONFIG
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target"

if [[ ! -f $UNIT ]] || [[ $(cat "$UNIT") != "$unit" ]]; then
  printf '%s\n' "$unit" | $SUDO tee "$UNIT" >/dev/null
  $SUDO systemctl daemon-reload
  changed=yes
fi
$SUDO systemctl enable --quiet agentcloud-market
if [[ $changed == yes ]] || ! systemctl is-active --quiet agentcloud-market; then
  say "starting agentcloud-market"
  $SUDO systemctl restart agentcloud-market
fi

for _ in $(seq 20); do curl -fsS "http://127.0.0.1:$PORT/api/config" >/dev/null 2>&1 && break; sleep 1; done
escrow="$(curl -fsS "http://127.0.0.1:$PORT/api/config" | sed -n 's/.*"escrow_address":"\([^"]*\)".*/\1/p')" \
  || die "the market is not answering on port $PORT; see: journalctl -u agentcloud-market -n 50"
say "market running on port $PORT"
echo "    escrow account: $escrow"
echo "    logs:           journalctl -u agentcloud-market -f"
