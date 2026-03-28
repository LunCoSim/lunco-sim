#!/bin/bash
# Start OpenMCT MCC locally
# Serves the Mission Control Center web interface

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MCC_DIR="$PROJECT_ROOT/html"
PORT="${1:-8080}"

echo "🚀 Starting OpenMCT MCC Server"
echo "=============================="
echo "Directory: $MCC_DIR"
echo "Port: $PORT"
echo ""

if [ ! -d "$MCC_DIR/openmct" ]; then
    echo "❌ Error: OpenMCT directory not found in $MCC_DIR"
    exit 1
fi

echo "OpenMCT will be available at: http://localhost:$PORT/openmct/"
echo "Press Ctrl+C to stop the server"
echo ""

# Use the existing web_server.py to handle CORS/COOP/COEP if needed, 
# or just simple python server if it's enough for local MCC dev.
# Since MCC might need to fetch telemetry from Godot (port 8082), 
# CORS headers are important.
cd "$MCC_DIR"
python3 "$SCRIPT_DIR/web_server.py" --port "$PORT" --root "." --no-browser
