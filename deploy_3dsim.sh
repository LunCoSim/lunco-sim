#!/bin/bash

# Deploy 3D simulation script
# This script builds the 3D sim, compresses assets, and deploys to web server

set -e  # Exit on any error

echo "=== Starting 3D Sim deployment ==="

# Step 1: Build the 3D sim
echo "Building 3D sim..."
godot --export-release --headless "Web_3DSim" build/3dsim/index.html

# Step 2: Compress index.pck and index.wasm with maximum gzip compression
echo "Compressing index.pck..."
gzip -9f build/3dsim/index.pck

echo "Compressing index.wasm..."
gzip -9f build/3dsim/index.wasm

# Step 3: Copy everything to /var/www/html
echo "Copying files to /var/www/html..."
sudo cp -r build/3dsim/* /var/www/html/

echo "=== Deployment completed successfully ==="
