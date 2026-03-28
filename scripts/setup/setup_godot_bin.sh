#!/bin/bash

# Simple Godot Engine Linux Installer
# Usage: sudo ./install_godot.sh <version>
# Example: sudo ./install_godot.sh 4.3
# Example: sudo ./install_godot.sh 4.7.dev3

set -e

if [ $# -ne 1 ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 4.3"
    echo "Example: $0 4.7.dev3"
    exit 1
fi

VERSION="$1"
TARGET_BIN="/usr/local/bin/godot"

# Check for required tools
for tool in wget unzip sudo; do
    if ! command -v $tool &> /dev/null; then
        echo "Error: $tool is required."
        exit 1
    fi
done

# Parse version and flavor
if [[ "$VERSION" =~ ^([0-9]+\.[0-9]+)(\.([a-zA-Z0-9]+))?$ ]]; then
    BASE_VERSION="${BASH_REMATCH[1]}"
    FLAVOR="${BASH_REMATCH[3]:-stable}"
else
    echo "Error: Invalid version format. Use X.Y or X.Y.flavorN"
    exit 1
fi

echo "=========================================="
echo "Godot Engine Installer"
echo "=========================================="
echo "Version: $VERSION (Base: $BASE_VERSION, Flavor: $FLAVOR)"
echo "Target:  $TARGET_BIN"
echo "=========================================="

# Create a temporary directory
TMP_DIR=$(mktemp -d)
cd "$TMP_DIR"

# Construct URL
# Pattern: https://downloads.godotengine.org/?version=4.7&flavor=dev3&slug=linux.x86_64.zip&platform=linux.64
URL="https://downloads.godotengine.org/?version=${BASE_VERSION}&flavor=${FLAVOR}&slug=linux.x86_64.zip&platform=linux.64"

echo "[1/4] Downloading Godot..."
if ! wget -O godot.zip "$URL"; then
    echo "Error: Failed to download Godot. Check version and flavor."
    rm -rf "$TMP_DIR"
    exit 1
fi

echo "[2/4] Extracting..."
unzip godot.zip

# Find the binary (it's usually the only non-zip file or has a specific pattern)
GODOT_BIN=$(ls | grep -v "godot.zip" | head -n 1)

if [ -z "$GODOT_BIN" ]; then
    echo "Error: Could not find Godot binary in the zip."
    rm -rf "$TMP_DIR"
    exit 1
fi

echo "[3/4] Installing to $TARGET_BIN..."
sudo mv "$GODOT_BIN" "$TARGET_BIN"
sudo chmod +x "$TARGET_BIN"

echo "[4/4] Cleaning up..."
rm -rf "$TMP_DIR"

echo ""
echo "=========================================="
echo "Installation Complete!"
echo "=========================================="
$TARGET_BIN --version
echo "=========================================="
