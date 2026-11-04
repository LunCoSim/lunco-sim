#!/usr/bin/env bash
#
# Two-peer networking smoke test (headless, no GUI).
#
# Spins up the real `net_smoke` host + client over the production lightyear
# wire and verifies, end-to-end:
#   - exclusive possession  (the client is denied the host's rover),
#   - each peer owns its own rover,
#   - ownership-gated control (drives on an unowned rover are rejected),
#   - ownership-table sync to the client,
#   - the possess → drive → snapshot round-trip.
#
# The client harness logs `RESULT: PASS`/`FAIL`; this script exits nonzero on
# FAIL so it can gate CI. Pure-logic coverage of the policy/registry lives in
# `cargo test -p lunco-core` (session::tests).
#
# Usage:  scripts/net_smoke.sh [port]      (default 5888)
set -uo pipefail
cd "$(dirname "$0")/.."

PORT="${1:-5888}"
[[ "$PORT" =~ ^[0-9]+$ && "$PORT" -ge 1 && "$PORT" -le 65533 ]] || exit 2
HOST_API=$((PORT+1))
CLIENT_API=$((PORT+2))
OUTPUT=".cache/net-smoke/${PORT}-$(date +%Y%m%dT%H%M%S)"
mkdir -p "$OUTPUT"
HOST_LOG="$OUTPUT/host.log"
CLIENT_LOG="$OUTPUT/client.log"

echo "==> building net_smoke with HTTP inspection (-j4)"
cargo build --locked -p lunco-luncosim --bin net_smoke --no-default-features --features networking,transport-http -j4 || exit 2

python3 - "$PORT" "$HOST_API" "$CLIENT_API" <<'PY' || exit 2
import socket,sys
for port,kind in [(int(sys.argv[1]),socket.SOCK_DGRAM),
                  (int(sys.argv[2]),socket.SOCK_STREAM),(int(sys.argv[3]),socket.SOCK_STREAM)]:
    with socket.socket(socket.AF_INET,kind) as listener:
        if kind==socket.SOCK_STREAM:
            listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
        listener.bind(('127.0.0.1',port))
PY

echo "==> launching host on :$PORT"
# Distinct LUNCO_PEER_ID per instance: both processes share this machine's
# persisted install id otherwise, colliding on journal author ids.
LUNCO_PEER_ID="smoke-host-$PORT" ./target/debug/net_smoke --host "$PORT" --api "$HOST_API" >"$HOST_LOG" 2>&1 &
HOST_PID=$!
# Ensure the host is torn down even if the client hangs.
cleanup() {
  for pid in "$HOST_PID" "${CLIENT_PID:-}"; do
    if [[ -n "$pid" && "$(ps -o ppid= -p "$pid" 2>/dev/null)" -eq "$$" ]]; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT

# Let the host bind before the client dials.
sleep 2

echo "==> launching client -> 127.0.0.1:$PORT"
LUNCO_PEER_ID="smoke-client-$PORT" ./target/debug/net_smoke --connect "127.0.0.1:$PORT" --api "$CLIENT_API" >"$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
python3 - "$HOST_PID" "$CLIENT_PID" "$HOST_API" "$CLIENT_API" "$OUTPUT" <<'PY' || exit 2
from pathlib import Path
import sys,time,urllib.request
for pid in sys.argv[1:3]:
    assert Path(f'/proc/{pid}/cwd').resolve()==Path.cwd()
    assert Path(f'/proc/{pid}/exe').resolve()==Path('target/debug/net_smoke').resolve()
for peer,port in zip(['host','client'],sys.argv[3:5]):
    deadline=time.monotonic()+10
    while True:
        try:
            with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/commands/schema',timeout=1) as response:
                Path(sys.argv[5],f'{peer}-schema.json').write_bytes(response.read())
            break
        except OSError:
            if time.monotonic()>=deadline:
                raise
            time.sleep(.1)
PY
wait "$CLIENT_PID"
CLIENT_EXIT=$?

wait "$HOST_PID"
HOST_EXIT=$?
trap - EXIT

echo "----- client verdict -----"
rg "RESULT|checks:" "$CLIENT_LOG" || true
echo "----- host arbitration -----"
rg "CLAIMED|DENIED|rejected SetPorts" "$HOST_LOG" | head -n 6 || true
echo "----- journal sync (client→host leg: peer_entries should be >0) -----"
rg "HOST-JOURNAL" "$HOST_LOG" | tail -n 2 || true
rg "authored journal entry" "$HOST_LOG" "$CLIENT_LOG" || true
echo "----- scripted merge policy (rhai, both peers) -----"
rg "activated scripted merge policy|merged_markers|policy_active" "$HOST_LOG" "$CLIENT_LOG" | tail -n 4 || true

python3 - "$PORT" "$HOST_API" "$CLIENT_API" <<'PY' || exit 2
import socket,sys
for port,kind in [(int(sys.argv[1]),socket.SOCK_DGRAM),
                  (int(sys.argv[2]),socket.SOCK_STREAM),(int(sys.argv[3]),socket.SOCK_STREAM)]:
    with socket.socket(socket.AF_INET,kind) as listener:
        if kind==socket.SOCK_STREAM:
            listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
        listener.bind(('127.0.0.1',port))
PY

if [[ "$HOST_EXIT" -eq 0 && "$CLIENT_EXIT" -eq 0 ]] \
  && rg -q "RESULT: PASS" "$CLIENT_LOG" \
  && rg -q "rejected undeclared/local wire command 'SetPortsBatch'" "$HOST_LOG"; then
  echo "net_smoke: PASS"
  exit 0
fi
echo "net_smoke: FAIL (see $CLIENT_LOG / $HOST_LOG)"
exit 1
