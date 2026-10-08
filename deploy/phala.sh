#!/usr/bin/env bash
# Deploys and manages agentcloud instances: Phala CVMs that each run the provider.
#
#   deploy/phala.sh images [--sandbox-image REPO@sha256:...]
#       Build and push the provider (and sandbox) images and pin them in deploy/images.lock.
#       --sandbox-image keeps a sandbox image already pushed elsewhere, for machines whose
#       uplink cannot carry it.
#   deploy/phala.sh up NAME [--code]
#       Create instance NAME from deploy/instances/NAME.env, or push its adjustable settings to
#       the running CVM. --code also deploys the current images to an instance that is not
#       frozen.
#   deploy/phala.sh freeze NAME
#       Disable upgrades and renounce ownership of the instance's app contract. Irreversible:
#       from then on nobody can change the code its keys are released to.
#   deploy/phala.sh status NAME
#
# Needs docker (with buildx), the phala CLI and, for on-chain KMS, foundry's cast. The Phala
# API key comes from PHALA_CLOUD_API_KEY or data/phala.key; the deployer key from
# DEPLOYER_KEY or data/deployer.key. Only CVMs this script created (recorded in data/phala/)
# are ever touched.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="$ROOT/data"
STATE_DIR="$DATA/phala"
LOCK="$ROOT/deploy/images.lock"
REPO="ghcr.io/jonas089/agentcloud-provider"
# Node 18 (prod9): auto-selection can land on nodes whose gateway never registers the CVM.
NODE_ID="18"
OS_IMAGE="dstack-0.5.9"
# The on-chain KMS chains Phala Cloud offers, and where their app contracts are visible.
# Empty for the off-chain KMS ("phala"), which has no contract.
kms_rpc() { case "$1" in base) echo https://mainnet.base.org ;; ethereum) echo https://ethereum-rpc.publicnode.com ;; esac; }
kms_explorer() { case "$1" in base) echo https://basescan.org ;; ethereum) echo https://etherscan.io ;; esac; }
# Settings in an instance file that configure the CVM rather than being passed into it.
DEPLOY_ONLY="INSTANCE_TYPE KMS"

usage() { sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }
say() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "needs $1"; }

phala_key() {
  [[ -n ${PHALA_CLOUD_API_KEY:-} ]] && return
  [[ -f $DATA/phala.key ]] || die "set PHALA_CLOUD_API_KEY or put the key in data/phala.key"
  PHALA_CLOUD_API_KEY="$(tr -d '[:space:]' < "$DATA/phala.key")"
  export PHALA_CLOUD_API_KEY
}

deployer_key() {
  if [[ -z ${DEPLOYER_KEY:-} ]]; then
    [[ -f $DATA/deployer.key ]] || die "set DEPLOYER_KEY or put the key in data/deployer.key"
    DEPLOYER_KEY="$(tr -d '[:space:]' < "$DATA/deployer.key")"
  fi
  [[ $DEPLOYER_KEY == 0x* ]] || DEPLOYER_KEY="0x$DEPLOYER_KEY"
}

# Reads deploy/instances/NAME.env into INSTANCE (deploy-only settings) and CVM_ENV (a file of
# the settings passed into the CVM).
load_instance() {
  NAME="$1"
  [[ $NAME == agentcloud* ]] || die "instance names start with agentcloud"
  local file="$ROOT/deploy/instances/$NAME.env"
  [[ -f $file ]] || die "no $file"
  INSTANCE_TYPE="$(sed -n 's/^INSTANCE_TYPE=//p' "$file")"
  KMS="$(sed -n 's/^KMS=//p' "$file")"
  KMS="${KMS:-base}"
  mkdir -p "$STATE_DIR"
  CVM_ENV="$STATE_DIR/$NAME.cvm.env"
  grep -E '^[A-Z_]+=' "$file" | grep -vE "^($(echo $DEPLOY_ONLY | tr ' ' '|'))=" > "$CVM_ENV"
  STATE="$STATE_DIR/$NAME.app-id"
}

# The compose file for the pinned images, with on-chain governance when the KMS has it.
render_compose() {
  [[ -f $LOCK ]] || die "no $LOCK; run deploy/phala.sh images"
  local provider sandbox governance=""
  provider="$(sed -n 's/^PROVIDER_IMAGE=//p' "$LOCK")"
  sandbox="$(sed -n 's/^SANDBOX_IMAGE=//p' "$LOCK")"
  [[ $provider == *@sha256:* && $sandbox == *@sha256:* ]] || die "$LOCK must pin both images by digest"
  if [[ -n $(kms_rpc "$KMS") ]]; then
    governance="        [offer.governance]
        rpc = \"$(kms_rpc "$KMS")\"
        app_contract = \"0x\${DSTACK_APP_ID}\"
        explorer = \"$(kms_explorer "$KMS")\"
"
  fi
  COMPOSE="$STATE_DIR/$NAME.compose.yml"
  python3 - "$ROOT/deploy/phala-compose.yml" "$COMPOSE" "$provider" "$sandbox" "$governance" <<'EOF'
import sys
template, out, provider, sandbox, governance = sys.argv[1:]
text = open(template).read()
text = text.replace("__PROVIDER_IMAGE__", provider).replace("__SANDBOX_IMAGE__", sandbox)
text = text.replace("__GOVERNANCE__\n", governance)
open(out, "w").write(text)
EOF
}

# A CVM is only ever touched if this script recorded it and Phala still names it NAME.
recorded_app_id() {
  [[ -f $STATE ]] || return 1
  local app_id; app_id="$(cat "$STATE")"
  phala cvms get --cvm-id "$app_id" --json 2>/dev/null | grep -q "\"name\": *\"$NAME\"" \
    || die "CVM $app_id is not named $NAME; refusing to touch it"
  echo "$app_id"
}

contract_owner() { # <app id>
  cast call "0x$1" 'owner()(address)' --rpc-url "$(kms_rpc "$KMS")"
}

# ------------------------------------------------------------------------------- commands

cmd_images() {
  need docker
  local sandbox=""
  [[ ${1:-} == --sandbox-image ]] && sandbox="${2:-}"
  build_and_push() { # <tag> <dockerfile> <context>; prints the pinned reference
    rm -f "$DATA/image.json"
    docker buildx build --platform linux/amd64 -f "$2" --tag "$REPO:$1" \
      --metadata-file "$DATA/image.json" --push "$3" >&2 || return 1
    local digest
    digest="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["containerimage.digest"])' "$DATA/image.json")"
    [[ $digest == sha256:* ]] && echo "$REPO@$digest"
  }
  say "building and pushing the provider image"
  local provider
  provider="$(build_and_push provider "$ROOT/deploy/provider.Dockerfile" "$ROOT")" || die "provider image failed"
  if [[ -z $sandbox ]]; then
    say "building and pushing the sandbox image"
    sandbox="$(build_and_push sandbox "$ROOT/crates/provider/sandbox/Dockerfile" "$ROOT/crates/provider/sandbox")" \
      || die "sandbox image failed"
  fi
  [[ $sandbox == *@sha256:* ]] || die "the sandbox image must be pinned by digest"
  python3 - "$LOCK" "$provider" "$sandbox" <<'EOF'
import re, sys
path, provider, sandbox = sys.argv[1:]
text = open(path).read()
text = re.sub(r"(?m)^PROVIDER_IMAGE=.*$", f"PROVIDER_IMAGE={provider}", text)
text = re.sub(r"(?m)^SANDBOX_IMAGE=.*$", f"SANDBOX_IMAGE={sandbox}", text)
open(path, "w").write(text)
EOF
  say "pinned in deploy/images.lock; commit it"
  cat "$LOCK" | grep _IMAGE=
}

cmd_up() {
  load_instance "$1"
  local code="${2:-}"
  need phala; phala_key
  local app_id onchain=()
  if [[ -n $(kms_rpc "$KMS") ]]; then
    need cast; deployer_key
    onchain=(--private-key "$DEPLOYER_KEY" --rpc-url "$(kms_rpc "$KMS")")
  fi
  if app_id="$(recorded_app_id)"; then
    if [[ $code == --code ]]; then
      render_compose
      say "deploying the current images to $NAME ($app_id)"
      phala deploy --cvm-id "$app_id" --compose "$COMPOSE" -e "$CVM_ENV" ${onchain[@]+"${onchain[@]}"} --wait
    else
      say "updating the adjustable settings of $NAME ($app_id)"
      phala envs update --cvm-id "$app_id" -e "$CVM_ENV" ${onchain[@]+"${onchain[@]}"}
    fi
    return
  fi
  render_compose
  say "creating $NAME: ${INSTANCE_TYPE:-tdx.small}, KMS $KMS, node $NODE_ID"
  local out
  out="$(phala deploy --name "$NAME" --compose "$COMPOSE" -e "$CVM_ENV" --instance-type "${INSTANCE_TYPE:-tdx.small}" \
    --node-id "$NODE_ID" --image "$OS_IMAGE" --no-dev-os --kms "$KMS" ${onchain[@]+"${onchain[@]}"} --wait --json 2>&1)" \
    || { printf '%s\n' "$out" >&2; die "phala deploy failed"; }
  # The CLI prints progress before its JSON, so take the first app id found in any object.
  app_id="$(printf '%s' "$out" | python3 -c '
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
                    print(node[key].removeprefix("0x")); raise SystemExit
            stack.extend(node.values())
        elif isinstance(node, list):
            stack.extend(node)
')"
  [[ -n $app_id ]] || { printf '%s\n' "$out" >&2; die "no app id in the output; check 'phala cvms ls' for $NAME"; }
  echo "$app_id" > "$STATE"
  say "$NAME is up as $app_id"
  [[ -n $(kms_rpc "$KMS") ]] && echo "    app contract: $(kms_explorer "$KMS")/address/0x$app_id"
  echo "    smoke-test it, then: deploy/phala.sh freeze $NAME"
}

cmd_freeze() {
  load_instance "$1"
  [[ -n $(kms_rpc "$KMS") ]] || die "$NAME uses the off-chain KMS, which has no contract to freeze"
  need cast; need phala; phala_key; deployer_key
  local app_id contract rpc
  rpc="$(kms_rpc "$KMS")"
  app_id="$(recorded_app_id)" || die "no recorded CVM for $NAME"
  contract="0x$app_id"
  local owner; owner="$(contract_owner "$app_id")"
  if [[ $owner == 0x0000000000000000000000000000000000000000 ]]; then say "$NAME is already frozen"; return; fi
  local me; me="$(cast wallet address --private-key "$DEPLOYER_KEY")"
  [[ $(echo "$owner" | tr '[:upper:]' '[:lower:]') == $(echo "$me" | tr '[:upper:]' '[:lower:]') ]] \
    || die "the deployer key does not own $contract (owner is $owner)"
  say "freezing $NAME: $contract on $KMS. This cannot be undone."
  cast send "$contract" 'disableUpgrades()' --private-key "$DEPLOYER_KEY" --rpc-url "$rpc" >/dev/null
  cast send "$contract" 'renounceOwnership()' --private-key "$DEPLOYER_KEY" --rpc-url "$rpc" >/dev/null
  for _ in $(seq 10); do
    [[ $(contract_owner "$app_id") == 0x0000000000000000000000000000000000000000 ]] && break
    sleep 3
  done
  [[ $(contract_owner "$app_id") == 0x0000000000000000000000000000000000000000 ]] || die "owner is not 0x0 yet; check $contract"
  say "$NAME is frozen: owner is 0x0, upgrades disabled"
  echo "    $(kms_explorer "$KMS")/address/$contract"
}

cmd_status() {
  load_instance "$1"
  need phala; phala_key
  local app_id; app_id="$(recorded_app_id)" || die "no recorded CVM for $NAME"
  phala cvms get --cvm-id "$app_id" 2>&1 | head -12
  if [[ -n $(kms_rpc "$KMS") ]]; then
    need cast
    echo "app contract: $(kms_explorer "$KMS")/address/0x$app_id"
    echo "owner:        $(contract_owner "$app_id")"
  fi
}

case "${1:-}" in
  images) shift; cmd_images "$@" ;;
  up) [[ -n ${2:-} ]] || usage 1; shift; cmd_up "$@" ;;
  freeze) [[ -n ${2:-} ]] || usage 1; cmd_freeze "$2" ;;
  status) [[ -n ${2:-} ]] || usage 1; cmd_status "$2" ;;
  -h|--help|help|"") usage ;;
  *) usage 1 ;;
esac
