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
HOST_LOG=/tmp/net_smoke_host.log
CLIENT_LOG=/tmp/net_smoke_client.log

echo "==> building net_smoke (--features networking, -j2)"
cargo build -p lunco-networking --bin net_smoke --features networking -j2 || exit 2

rm -f "$HOST_LOG" "$CLIENT_LOG"

echo "==> launching host on :$PORT"
# Distinct LUNCO_PEER_ID per instance: both processes share this machine's
# persisted install id otherwise, colliding on journal author ids.
LUNCO_PEER_ID=smoke-host ./target/debug/net_smoke --host "$PORT" >"$HOST_LOG" 2>&1 &
HOST_PID=$!
# Ensure the host is torn down even if the client hangs.
trap 'kill "$HOST_PID" 2>/dev/null' EXIT

# Let the host bind before the client dials.
sleep 2

echo "==> launching client -> 127.0.0.1:$PORT"
LUNCO_PEER_ID=smoke-client ./target/debug/net_smoke --connect "127.0.0.1:$PORT" >"$CLIENT_LOG" 2>&1

wait "$HOST_PID" 2>/dev/null

echo "----- client verdict -----"
grep -E "RESULT|checks:" "$CLIENT_LOG" || true
echo "----- host arbitration -----"
grep -E "CLAIMED|DENIED|rejected DriveRover" "$HOST_LOG" | head -n 6 || true
echo "----- journal sync (client→host leg: peer_entries should be >0) -----"
grep -E "HOST-JOURNAL" "$HOST_LOG" | tail -n 2 || true
grep -E "authored journal entry" "$HOST_LOG" "$CLIENT_LOG" || true
echo "----- scripted merge policy (rhai, both peers) -----"
grep -E "activated scripted merge policy|merged_markers|policy_active" "$HOST_LOG" "$CLIENT_LOG" | tail -n 4 || true

if grep -q "RESULT: PASS" "$CLIENT_LOG"; then
  echo "net_smoke: PASS"
  exit 0
fi
echo "net_smoke: FAIL (see $CLIENT_LOG / $HOST_LOG)"
exit 1
