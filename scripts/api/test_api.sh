#!/usr/bin/env bash
# LunCoSim API Test Scripts
# 
# Usage:
#   ./scripts/api/test_api.sh [PORT]
#
# Tests the API against a running LunCoSim instance.
# Make sure the sim is running with `--api` flag first:
#   cargo run --bin rover_sandbox_usd -- --api 3000

set -e

PORT="${1:-3000}"
BASE="http://127.0.0.1:${PORT}/api"

echo "🚀 LunCoSim API Tests (port ${PORT})"
echo "====================================="
echo ""

# Wait for API to be ready
echo "⏳ Waiting for API to be ready..."
for i in {1..10}; do
    if curl -s "${BASE}/health" > /dev/null 2>&1; then
        echo "✅ API is ready"
        break
    fi
    if [ $i -eq 10 ]; then
        echo "❌ API not responding on port ${PORT}"
        echo "   Make sure the sim is running with: cargo run --bin rover_sandbox_usd -- --api ${PORT}"
        exit 1
    fi
    sleep 1
done
echo ""

# 1. Health Check
echo "📡 1. Health Check"
HEALTH=$(curl -s "${BASE}/health")
echo "${HEALTH}" | jq . 2>/dev/null || echo "  ${HEALTH}"
echo ""

# 2. Discover Schema
echo "🔍 2. Discover Available Commands"
SCHEMA=$(curl -s "${BASE}/commands/schema")
if echo "${SCHEMA}" | jq . > /dev/null 2>&1; then
    CMD_COUNT=$(echo "${SCHEMA}" | jq '.data.commands | length' 2>/dev/null || echo "0")
    echo "  Found ${CMD_COUNT} commands:"
    echo "${SCHEMA}" | jq -r '.data.commands[] | "    • \(.name) (\(.fields | length) fields)"' 2>/dev/null || echo "  (unable to parse commands)"
else
    echo "  Raw response: ${SCHEMA}"
fi
echo ""

# 3. List Entities
echo "📋 3. List Entities"
# Wait a moment for entities to spawn
sleep 2
ENTITIES=$(curl -s "${BASE}/entities")
if echo "${ENTITIES}" | jq . > /dev/null 2>&1; then
    ENTITY_COUNT=$(echo "${ENTITIES}" | jq '.data.count // 0' 2>/dev/null || echo "0")
    echo "  Found ${ENTITY_COUNT} entities:"
    echo "${ENTITIES}" | jq -r '.data.entities[:5][] | "    • \(.api_id) (index: \(.entity_index))"' 2>/dev/null || echo "  (unable to parse entities)"
    if [ "$ENTITY_COUNT" -gt 5 ] 2>/dev/null; then
        echo "    ... and $((ENTITY_COUNT - 5)) more"
    fi
else
    echo "  Raw response: ${ENTITIES}"
fi
echo ""

# 4. Get first entity
echo "🔎 4. Query First Entity"
FIRST_ID=$(echo "${ENTITIES}" | jq -r '.data.entities[0].api_id // empty' 2>/dev/null)
if [ -n "$FIRST_ID" ]; then
    ENTITY_DATA=$(curl -s "${BASE}/entities/${FIRST_ID}")
    echo "${ENTITY_DATA}" | jq . 2>/dev/null || echo "  ${ENTITY_DATA}"
else
    echo "  (no entities available — spawn a rover first in the sim)"
fi
echo ""

# 5. Query an entity that doesn't exist (error case)
echo "❌ 5. Query Non-existent Entity (expect 404)"
ERROR_RESP=$(curl -s -w "\n  HTTP Status: %{http_code}" "${BASE}/entities/00000000000000000000000000")
echo "  ${ERROR_RESP}"
echo ""

echo "✅ All tests completed"
echo ""
echo "💡 Try the rover drive demo:"
echo "   ./scripts/api/demo_drive_rover.sh ${PORT}"
