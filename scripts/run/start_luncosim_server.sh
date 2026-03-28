#!/bin/bash
# Start the Godot headless simulation server
# Usage: ./run_server.sh [path_to_godot]

set -e

# Ensure we are in the project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../.."

# Default Godot path if not provided
GODOT_BIN=${1:-"godot"}

if [ ! -f "$GODOT_BIN" ]; then
    echo "Error: Godot binary not found at $GODOT_BIN"
    echo "Usage: $0 [path_to_godot]"
    exit 1
fi

echo "Starting headless server using: $GODOT_BIN"

$GODOT_BIN --server --headless --certificate res://.cert/fullchain.pem --key res://.cert/privkey.pem ./apps/3dsim/main.tscn
