#!/bin/bash

echo "=== Verifying Telemetry System ==="
echo ""

echo "1. Checking if telemetry API is running..."
if curl -s http://localhost:8082/api/entities > /dev/null 2>&1; then
    echo "   ✓ Telemetry API is running on port 8082"
else
    echo "   ✗ Telemetry API is NOT running"
    exit 1
fi

echo ""
echo "2. Checking for entities..."
ENTITIES=$(curl -s http://localhost:8082/api/entities)
COUNT=$(echo "$ENTITIES" | python3 -c "import sys, json; print(len(json.load(sys.stdin)['entities']))")

if [ "$COUNT" -gt 0 ]; then
    echo "   ✓ Found $COUNT entity/entities"
    echo ""
    echo "   Entity details:"
    echo "$ENTITIES" | python3 -m json.tool
else
    echo "   ✗ No entities found"
    echo "   Response: $ENTITIES"
fi

echo ""
echo "3. Checking OpenMCT dictionary..."
DICT=$(curl -s http://localhost:8082/api/dictionary)
MEASUREMENTS=$(echo "$DICT" | python3 -c "import sys, json; print(len(json.load(sys.stdin)['measurements']))")

if [ "$MEASUREMENTS" -gt 0 ]; then
    echo "   ✓ Found $MEASUREMENTS measurement(s) in dictionary"
else
    echo "   ✗ No measurements in dictionary"
fi

echo ""
echo "=== Next Steps ==="
if [ "$COUNT" -gt 0 ]; then
    echo "✓ Telemetry system is working!"
    echo "  Open OpenMCT at: http://localhost:8080/openmct/"
    echo "  You should see your rover under 'LunCoSim Entities'"
else
    echo "✗ Rover not detected yet"
    echo "  1. Make sure you've restarted the Godot server"
    echo "  2. Spawn a rover in the simulation"
    echo "  3. Wait a few seconds for discovery"
    echo "  4. Run this script again"
fi
