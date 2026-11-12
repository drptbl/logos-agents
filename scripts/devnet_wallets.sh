#!/usr/bin/env bash
# Makes a funded agent wallet for each side of a job on the local devnet that
# `logos-devnet up` starts from logos-devnet.toml, for scripts/demo_escrow_job.py.
#
# Each wallet is new and gets one public account, funded from the devnet's
# faucet. It is created in its side's logos-devnet session, then lez_core is
# reloaded there, so the agent server opens the wallet itself: a server only
# trusts a wallet it opened.
#
# usage: scripts/devnet_wallets.sh [dir]   (default ~/.logos-agents-devnet)
# Prints the buyer's account, for --buyer-from.
set -euo pipefail
dir="${1:-$HOME/.logos-agents-devnet}"

base58() {
  python3 -I -c '
import sys
raw = bytes.fromhex(sys.argv[1])
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n, out = int.from_bytes(raw, "big"), ""
while n:
    n, r = divmod(n, 58)
    out = alphabet[r] + out
print("1" * (len(raw) - len(raw.lstrip(b"\0"))) + out)' "$1"
}

# The result of a logosctl call. Its reply is never printed, even on failure:
# create_new's carries the wallet's recovery phrase.
result() {
  python3 -I -c '
import json, sys
r = json.load(sys.stdin)
if r.get("status") != "ok":
    sys.exit(f"logosctl {sys.argv[1]} failed")
print(r["result"])' "$1"
}

for side in provider buyer; do
  wallet="$dir/$side"
  if [[ -e "$wallet/storage.json" ]]; then
    echo "$wallet already holds a wallet: remove it, or pass another dir" >&2
    exit 1
  fi
  export LOGOSCTL_CONFIG_DIR="${LOGOS_DEVNET_HOME:-$HOME/.logos-devnet}/sessions/agents-$side/logosctl"
  if [[ ! -d "$LOGOSCTL_CONFIG_DIR" ]]; then
    echo "no logos-devnet session agents-$side: run logos-devnet up in this repo first" >&2
    exit 1
  fi
  logos-devnet wallet-config "$wallet" >/dev/null
  logosctl module reload lez_core >/dev/null
  logosctl call lez_core create_new "str:$wallet/wallet_config.json" "str:$wallet/storage.json" str: 2>/dev/null | result create_new >/dev/null
  hex=$(logosctl call lez_core create_account_public | result create_account_public)
  account="Public/$(base58 "$hex")"
  logosctl call lez_core save | result save >/dev/null
  logosctl module reload lez_core >/dev/null
  logos-devnet fund "$account" >/dev/null
  echo "$account" >"$wallet/account"
  echo "$side: $account" >&2
done
cat "$dir/buyer/account"
