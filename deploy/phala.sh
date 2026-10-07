#!/usr/bin/env bash
# Builds the provider and sandbox images, pushes them, and deploys the Phala CVM that runs
# them, or updates the CVM already recorded in data/phala-app-id. It never touches a CVM it
# did not create.
#
#   deploy/phala.sh --market URL [--payout ADDRESS] [--repo ghcr.io/<you>/agentcloud-provider]
#                   [--instance-type tdx.small] [--name agentcloud] [--sandbox-image REPO@sha256:...]
#
# --sandbox-image uses a sandbox image already pushed elsewhere (pinned by digest) instead of
# building it here, for machines whose uplink cannot carry the large image.
#
# Rent from every lease, and the first payments the escrow forwards, go to the payout address.
#
# The Phala API key comes from PHALA_CLOUD_API_KEY or data/phala.key. The CVM runs a
# production OS image (--no-dev-os): nobody, the operator included, gets a shell in it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="$ROOT/data"
MARKET=""
PAYOUT="celestia1d2n9pft5frjentrgfdk0tpkwa5jepf9jyc5pmg"
REPO="ghcr.io/jonas089/agentcloud-provider"
SANDBOX_REPO="ghcr.io/jonas089/agentcloud-sandbox"
SANDBOX_IMAGE=""
INSTANCE_TYPE="tdx.small"
# Node 18 (prod9): auto-selection can land on nodes whose gateway never registers the CVM.
NODE_ID="18"
OS_IMAGE="dstack-0.5.9"
NAME="agentcloud"

usage() { sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
say() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --market) MARKET="${2%/}"; shift 2 ;;
    --payout) PAYOUT="$2"; shift 2 ;;
    --repo) REPO="$2"; shift 2 ;;
    --instance-type) INSTANCE_TYPE="$2"; shift 2 ;;
    --node-id) NODE_ID="$2"; shift 2 ;;
    --sandbox-image) SANDBOX_IMAGE="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown option $1" >&2; usage 1 ;;
  esac
done
[[ -n $MARKET && $PAYOUT == celestia1* ]] || usage 1
[[ $NAME == agentcloud* ]] || die "the CVM name must start with agentcloud"
command -v docker >/dev/null && command -v phala >/dev/null || die "needs docker (with buildx) and the phala CLI"
if [[ -z ${PHALA_CLOUD_API_KEY:-} ]]; then
  [[ -f $DATA/phala.key ]] || die "set PHALA_CLOUD_API_KEY or put the key in data/phala.key"
  PHALA_CLOUD_API_KEY="$(tr -d '[:space:]' < "$DATA/phala.key")"
fi
export PHALA_CLOUD_API_KEY
mkdir -p "$DATA"

# Both images are pinned by digest in the compose file, so the attestation covers exactly
# the code that runs: the provider and the sandbox every lease gets.
build_and_push() { # <repository> <dockerfile> <context>; prints the pinned reference
  rm -f "$DATA/image.json"
  docker buildx build --platform linux/amd64 -f "$2" --tag "$1:latest" \
    --metadata-file "$DATA/image.json" --push "$3" >&2 || return 1
  local digest
  digest="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["containerimage.digest"])' "$DATA/image.json")" \
    || return 1
  [[ $digest == sha256:* ]] || return 1
  echo "$1@$digest"
}
say "building and pushing the provider and sandbox images"
# Assigned one by one with `|| die`: a failure inside $(...) would not stop the script.
PROVIDER_IMAGE="$(build_and_push "$REPO" "$ROOT/deploy/provider.Dockerfile" "$ROOT")" \
  || die "building or pushing $REPO failed; nothing was deployed (is 'docker login ghcr.io' allowed to push?)"
if [[ -z $SANDBOX_IMAGE ]]; then
  SANDBOX_IMAGE="$(build_and_push "$SANDBOX_REPO" "$ROOT/crates/provider/sandbox/Dockerfile" "$ROOT/crates/provider/sandbox")" \
    || die "building or pushing $SANDBOX_REPO failed; nothing was deployed"
fi
[[ $SANDBOX_IMAGE == *@sha256:* ]] || die "the sandbox image must be pinned by digest (REPO@sha256:...)"
say "provider $PROVIDER_IMAGE"
say "sandbox  $SANDBOX_IMAGE"

COMPOSE="$DATA/phala-compose.yml"
sed -e "s|__PROVIDER_IMAGE__|$PROVIDER_IMAGE|" -e "s|__SANDBOX_IMAGE__|$SANDBOX_IMAGE|" \
    -e "s|__MARKET__|$MARKET|" -e "s|__PAYOUT__|$PAYOUT|" \
  "$ROOT/deploy/phala-compose.yml" > "$COMPOSE"

if [[ -f $DATA/phala-app-id ]]; then
  APP_ID="$(cat "$DATA/phala-app-id")"
  # Only ever touch a CVM this script created: refuse anything not named agentcloud*.
  phala cvms get --cvm-id "$APP_ID" --json 2>/dev/null | grep -q "\"name\": *\"$NAME" \
    || die "CVM $APP_ID is not named $NAME*; refusing to touch it"
  say "updating CVM $APP_ID"
  phala deploy --cvm-id "$APP_ID" --compose "$COMPOSE" --wait
else
  say "deploying a new $INSTANCE_TYPE CVM named $NAME"
  OUT="$(phala deploy --name "$NAME" --compose "$COMPOSE" --instance-type "$INSTANCE_TYPE" --node-id "$NODE_ID" \
    --image "$OS_IMAGE" --no-dev-os --wait --json 2>&1)" \
    || { printf '%s\n' "$OUT" >&2; die "phala deploy failed"; }
  # The CLI prints progress before its JSON, so take the first app id found in any JSON object.
  APP_ID="$(printf '%s' "$OUT" | python3 -c '
import json, re, sys
raw = sys.stdin.read()
for m in re.finditer(r"[{\[]", raw):
    try:
        doc, _ = json.JSONDecoder().raw_decode(raw[m.start():])
    except ValueError:
        continue
    stack = [doc]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            for key in ("app_id", "appId"):
                if isinstance(node.get(key), str) and node[key]:
                    print(node[key]); raise SystemExit
            stack.extend(node.values())
        elif isinstance(node, list):
            stack.extend(node)
')"
  [[ -n $APP_ID ]] || { printf '%s\n' "$OUT" >&2; die "could not read the app id; check 'phala cvms ls' for a stray $NAME"; }
  echo "$APP_ID" > "$DATA/phala-app-id"
fi
say "CVM $APP_ID is up; its offer appears on $MARKET once the sandbox image is built"
echo "    logs: phala logs --cvm-id $APP_ID"
