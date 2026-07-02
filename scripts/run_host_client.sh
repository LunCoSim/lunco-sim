#!/usr/bin/env bash
# Launch two sandbox instances side by side: a networking HOST on the left
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
HOST_LOG=/tmp/sandbox_host.log
CLIENT_LOG=/tmp/sandbox_client.log

if [[ "${1:-halves}" == "quarters" ]]; then
  HOST_POS=top-left
  CLIENT_POS=bottom-left
else
  HOST_POS=left
  CLIENT_POS=right
fi

# Build once up front so the two launches don't race the build lock.
echo "building sandbox (networking)…"
cargo build --bin sandbox --features networking -j2

# Politely ask any prior instances on these API ports to exit.
for p in "$HOST_API" "$CLIENT_API"; do
  curl -s -m 1 -X POST "http://127.0.0.1:$p/api/commands" \
    -H 'Content-Type: application/json' -d '{"type":"Exit"}' >/dev/null 2>&1 || true
done
sleep 1

BIN=target/debug/sandbox
RL='info,wgpu=error,naga=warn'

echo "launching HOST  ($HOST_POS, api $HOST_API)…"
# Distinct LUNCO_PEER_ID per instance — both share this machine's persisted
# install id otherwise, colliding on journal author ids (see journal_plane).
setsid nohup env RUST_LOG="$RL" LUNCO_PEER_ID=local-host "$BIN" --host --window-pos "$HOST_POS" --api "$HOST_API" \
  >"$HOST_LOG" 2>&1 </dev/null & disown
sleep 4   # let the host bind :5888 + write the cert digest before the client dials

echo "launching CLIENT ($CLIENT_POS, api $CLIENT_API)…"
setsid nohup env RUST_LOG="$RL" LUNCO_PEER_ID=local-client "$BIN" --connect 127.0.0.1 --window-pos "$CLIENT_POS" --api "$CLIENT_API" \
  >"$CLIENT_LOG" 2>&1 </dev/null & disown

echo "host log:   $HOST_LOG"
echo "client log: $CLIENT_LOG"
echo "done. host=:$HOST_API client=:$CLIENT_API  (status bars show 'HOST :5888 · N peer' / 'CLIENT → 127.0.0.1:5888')"
