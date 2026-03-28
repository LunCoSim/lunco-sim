#!/bin/bash
# Installs addons for Godot Engine using gd-plug
# Usage: ./install_addons.sh [godot_path]

set -e

# Ensure we are in the project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../.."

GODOT_BIN="${1:-godot}"

echo "Installing addons using: $GODOT_BIN"
"$GODOT_BIN" --headless -s plug.gd install
