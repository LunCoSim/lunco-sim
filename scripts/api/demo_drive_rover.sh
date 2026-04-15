#!/usr/bin/env bash
# LunCoSim Rover Drive Demo
#
# Usage:
#   ./scripts/api/demo_drive_rover.sh [PORT]
#
# Demonstrates:
#   1. Discover available commands
#   2. Find a rover entity
#   3. Drive it forward
#   4. Brake to stop
#
# Requires a running sim with --api flag and at least one rover in the scene.

set -e

PORT="${1:-3000}"
BASE="http://127.0.0.1:${PORT}/api"

echo "🚗 LunCoSim Rover Drive Demo (port ${PORT})"
echo "============================================="
echo ""

# Step 1: Discover schema
echo "📖 Step 1: Discovering available commands..."
COMMANDS=$(curl -s "${BASE}/commands/schema" | jq -r '.data.commands[].name' 2>/dev/null)
echo "Available commands:"
echo "${COMMANDS}" | sed 's/^/  • /'
echo ""

# Step 2: Find a rover
echo "🔍 Step 2: Finding rover entities..."
ROVER_ID=$(curl -s "${BASE}/entities" | jq -r '
  .data.entities[] | select(.api_id != null) | .api_id
' | head -1)

if [ -z "$ROVER_ID" ] || [ "$ROVER_ID" = "null" ]; then
    echo "⚠️  No entities found. Make sure the sim has rovers spawned."
    echo "   Hint: In the sim, spawn a rover first."
    exit 1
fi

echo "Found rover: ${ROVER_ID}"
echo ""

# Step 3: Drive forward
echo "🏎️  Step 3: Driving forward (5 seconds)..."
curl -s -X POST "${BASE}/commands" \
  -H "Content-Type: application/json" \
  -d "{
    \"command\": \"DriveRover\",
    \"params\": {
      \"target\": \"${ROVER_ID}\",
      \"forward\": 0.8,
      \"steer\": 0.0
    }
  }" | jq .
echo ""

sleep 5

# Step 4: Brake
echo "🛑 Step 4: Braking..."
curl -s -X POST "${BASE}/commands" \
  -H "Content-Type: application/json" \
  -d "{
    \"command\": \"BrakeRover\",
    \"params\": {
      \"target\": \"${ROVER_ID}\",
      \"intensity\": 1.0
    }
  }" | jq .
echo ""

echo "✅ Demo completed!"
