#!/bin/bash
# Run Godot tests
# Usage: ./run_tests.sh [godot_path]

set -e

# Ensure we are in the project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../.."

GODOT_BIN="${1:-godot}"

echo "Running tests using: $GODOT_BIN"
"$GODOT_BIN" --headless --script apps/modelica/tests/run_tests.gd
