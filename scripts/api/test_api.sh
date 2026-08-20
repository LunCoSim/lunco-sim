#!/usr/bin/env bash
# Probe an already-running production luncosim instance.
#
# Usage:
#   ./scripts/api/test_api.sh [PORT]
#
# Start the instance separately with the canonical binary, for example:
#   target/debug/luncosim --no-ui --api 4101

set -euo pipefail

PORT="${1:-4101}"
READY_TIMEOUT_S="${LUNCOSIM_API_READY_TIMEOUT_S:-120}"
BASE="http://127.0.0.1:${PORT}/api"

echo "🚀 LunCoSim API checks (port ${PORT})"
echo "===================================="

echo "⏳ Waiting for /api/ready to report a usable world..."
started_at=$SECONDS
while true; do
    ready="$(curl --fail-with-body --silent --show-error "${BASE}/ready" 2>/dev/null || true)"
    if jq -e '.data.ready == true and .data.world_hold == false' >/dev/null 2>&1 <<<"${ready}"; then
        echo "✅ Runtime is ready"
        break
    fi
    if (( SECONDS - started_at >= READY_TIMEOUT_S )); then
        echo "❌ Runtime did not become ready within ${READY_TIMEOUT_S}s" >&2
        echo "   Start it with: target/debug/luncosim --no-ui --api ${PORT}" >&2
        exit 1
    fi
    sleep 1
done

echo
echo "📡 1. Health"
curl --fail-with-body --silent --show-error "${BASE}/health" | jq .

echo
echo "🔍 2. Live command schema"
schema="$(curl --fail-with-body --silent --show-error "${BASE}/commands/schema")"
jq -e '.data.commands | type == "array"' >/dev/null <<<"${schema}"
jq -r '.data.commands[] | "    • \(.name) (\(.fields | length) fields)"' <<<"${schema}"

echo
echo "📋 3. ListEntities through the command funnel"
entities="$(curl --fail-with-body --silent --show-error \
    -X POST "${BASE}/commands" \
    -H 'Content-Type: application/json' \
    -d '{"type":"ListEntities"}')"
jq -e '.data.entities | type == "array"' >/dev/null <<<"${entities}"
jq -r '.data.entities[:5][] | "    • [\(.type)] \(.api_id): \(.name)"' <<<"${entities}"

echo
echo "✅ API checks completed against the live production session"
