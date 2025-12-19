#!/bin/bash

# Deploy 3D simulation script
# This script builds the 3D sim, compresses assets, and deploys to web server
# Usage: ./deploy_3dsim.sh [path_to_godot] [build_type]
#   path_to_godot: Optional path to Godot executable (defaults to 'godot' in PATH)
#   build_type: Optional build type - 'debug' or 'release' (defaults to 'debug')

set -e  # Exit on any error

# Use provided Godot path or default to 'godot' in PATH
GODOT_PATH="${1:-godot}"

# Use provided build type or default to 'debug'
BUILD_TYPE="${2:-debug}"

# Validate build type
if [[ "$BUILD_TYPE" != "debug" && "$BUILD_TYPE" != "release" ]]; then
    echo "Error: Invalid build type '$BUILD_TYPE'. Must be 'debug' or 'release'."
    exit 1
fi

# Set export flag based on build type
if [[ "$BUILD_TYPE" == "release" ]]; then
    EXPORT_FLAG="--export-release"
else
    EXPORT_FLAG="--export-debug"
fi

echo "=== Starting 3D Sim deployment ==="
echo "Using Godot executable: $GODOT_PATH"
echo "Build type: $BUILD_TYPE"

# Step 1: Create build directory
echo "Creating build directory..."
mkdir -p build/3dsim

# Step 2: Build the 3D sim
echo "Building 3D sim..."
"$GODOT_PATH" $EXPORT_FLAG --headless "Web_3DSim" build/3dsim/index.html

# Step 2.5: Prepare web assets
echo "Preparing web assets..."
# Download web3.min.js
curl -L -o build/3dsim/web3.min.js https://cdn.jsdelivr.net/npm/web3@4.13.0/dist/web3.min.js

# Copy custom JS files
cp html/contract-abi.js build/3dsim/
cp html/web3-integration.js build/3dsim/

# Step 3: Compress index.pck and index.wasm with maximum gzip compression
echo "Compressing index.pck..."
gzip -9f build/3dsim/index.pck

echo "Compressing index.wasm..."
gzip -9f build/3dsim/index.wasm

# Step 4: Copy everything to /var/www/html
echo "Copying files to /var/www/html..."
sudo cp -r build/3dsim/* /var/www/html/

echo "=== Deployment completed successfully ==="
