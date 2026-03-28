#!/bin/bash
# Start Godot in the background and keep it running
# Usage: ./run_godot_persistent.sh [godot_path]

set -e

# Ensure we are in the project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../.."

GODOT_BIN="${1:-godot}"

echo "Starting Godot persistent server using: $GODOT_BIN"
nohup "$GODOT_BIN" --server > godot_server.log 2>&1 &
echo $! > godot.pid
echo "Godot started with PID $!"
