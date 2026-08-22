#!/usr/bin/env bash
# Launch two luncosim instances side by side: a networking HOST on the left
# half of the screen and a CLIENT (joined over WebTransport) on the right —
# the exact layout used to eyeball host/client rover sync.
#
#   scripts/run_host_client.sh            # left=host(4101)  right=client(4102)
#   scripts/run_host_client.sh quarters   # top-left=host    bottom-left=client
#
# Notes:
#  * Uses --window-pos (left|right or top-left|bottom-left) so the windows
#    place themselves; forced placement does NOT pollute the persisted
#    window geometry (SkipWindowGeometrySave), so a normal launch later
#    still opens at your saved bounds.
#  * Each instance is detached with `setsid` so it survives this script
#    exiting (a bare `&` gets SIGHUP-reaped and dies after ~10 s).
#  * --api 4101/4102 expose the HTTP API for sync inspection.
set -euo pipefail
cd "$(dirname "$0")/.."

HOST_API=4101
CLIENT_API=4102
HOST_LOG=/tmp/luncosim-host.log
CLIENT_LOG=/tmp/luncosim-client.log
READY_ATTEMPTS=120

if [[ "${1:-halves}" == "quarters" ]]; then
  HOST_POS=top-left
  CLIENT_POS=bottom-left
else
  HOST_POS=left
  CLIENT_POS=right
fi

# Build once up front so the two launches don't race the build lock.
echo "building luncosim (networking)…"
RUSTC_WRAPPER="${RUSTC_WRAPPER:-}" cargo build --bin luncosim --features networking -j4

# Politely ask any prior instances on these API ports to exit, then require the
# API port to disappear. A blind sleep hides an overlapping process and makes
# the next launch race the old session.
stop_previous() {
  local port="$1"
  curl -s -m 1 -X POST "http://127.0.0.1:$port/api/commands" \
    -H 'Content-Type: application/json' \
    -d '{"type":"ExecuteCommand","command":"Exit","params":{}}' \
    >/dev/null 2>&1 || true
  for _ in $(seq 1 "$READY_ATTEMPTS"); do
    if ! curl -sf -m 1 "http://127.0.0.1:$port/api/ready" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "previous luncosim session on API port $port did not exit" >&2
  exit 1
}

wait_ready() {
  local port="$1"
  for _ in $(seq 1 "$READY_ATTEMPTS"); do
    if curl -sf -m 1 "http://127.0.0.1:$port/api/ready" \
      | python3 -c 'import json,sys; d=json.load(sys.stdin); raise SystemExit(0 if d.get("ready") and not d.get("world_hold") else 1)'; then
      return 0
    fi
    sleep 0.1
  done
  echo "luncosim on API port $port did not become ready" >&2
  exit 1
}

stop_previous "$HOST_API"
stop_previous "$CLIENT_API"

BIN=target/debug/luncosim
RL='info,wgpu=error,naga=warn'

echo "launching HOST  ($HOST_POS, api $HOST_API)…"
# Distinct LUNCO_PEER_ID per instance — both share this machine's persisted
# install id otherwise, colliding on journal author ids (see journal_plane).
setsid nohup env RUST_LOG="$RL" LUNCO_PEER_ID=local-host "$BIN" --host --window-pos "$HOST_POS" --api "$HOST_API" \
  >"$HOST_LOG" 2>&1 </dev/null & disown
wait_ready "$HOST_API"

echo "launching CLIENT ($CLIENT_POS, api $CLIENT_API)…"
setsid nohup env RUST_LOG="$RL" LUNCO_PEER_ID=local-client "$BIN" --connect 127.0.0.1 --window-pos "$CLIENT_POS" --api "$CLIENT_API" \
  >"$CLIENT_LOG" 2>&1 </dev/null & disown
wait_ready "$CLIENT_API"

echo "host log:   $HOST_LOG"
echo "client log: $CLIENT_LOG"
echo "done. host=:$HOST_API client=:$CLIENT_API  (status bars show 'HOST :5888 · N peer' / 'CLIENT → 127.0.0.1:5888')"
