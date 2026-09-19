#!/usr/bin/env bash
# Run RemoveAttribute acceptance through a fresh production Rhai/API host.
# All USD behavior assertions live in assets/scripting/tests/test_usd_remove_attribute.rhai.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$REPO_ROOT/target/debug/luncosim"
TEST="$REPO_ROOT/assets/scripting/tests/test_usd_remove_attribute.rhai"
RUNNER="$REPO_ROOT/scripts/api/run_rhai_test.sh"

if [[ ! -x "$BIN" ]]; then
    echo "missing $BIN; build it with: cargo build -p lunco-luncosim --bin luncosim" >&2
    exit 2
fi

api_port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
"$BIN" --no-ui --api "$api_port" >/dev/null 2>&1 &
host_pid=$!
cleanup() {
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
}
trap cleanup EXIT

ready=false
for attempt in {1..160}; do
    if curl --silent --fail "http://127.0.0.1:$api_port/api/ready" >/dev/null; then
        ready=true
        break
    fi
    sleep 0.25
done
if [[ "$ready" != true ]]; then
    echo "headless LunCoSim API did not become ready" >&2
    exit 2
fi

"$BIN" rhai --api "$api_port" --stdout \
    -e 'let result = cmd("NewDocument", #{ kind: "usd" }); if result.ok { print("SCRATCH_DOCUMENT_REQUESTED"); } else { print("SCRATCH_DOCUMENT_FAILED " + result.error); }' \
    | rg -q '^SCRATCH_DOCUMENT_REQUESTED$'

document_ready=false
for attempt in {1..160}; do
    probe="$("$BIN" rhai --api "$api_port" --stdout \
        -e 'let documents = query("ListOpenDocuments"); if documents != () && documents.open_documents != () && documents.open_documents.len() == 1 && documents.open_documents[0].kind == "usd" { print("SCRATCH_DOCUMENT_READY"); } else { print("WAITING"); }')"
    if [[ "$probe" == *SCRATCH_DOCUMENT_READY* ]]; then
        document_ready=true
        break
    fi
    sleep 0.25
done
if [[ "$document_ready" != true ]]; then
    echo "scratch USD document did not become available" >&2
    exit 2
fi

"$RUNNER" "$api_port" "$TEST"
