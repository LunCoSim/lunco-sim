#!/bin/bash
# Deploy OpenMCT to /var/www/html/mcc
# This script copies OpenMCT files to the web server directory

set -e  # Exit on any error

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$SCRIPT_DIR/html/openmct"
TARGET_DIR="/var/www/html/mcc"

echo "=== Starting OpenMCT deployment ==="
echo "Source: $SOURCE_DIR"
echo "Target: $TARGET_DIR"
echo ""

# Check if source directory exists
if [ ! -d "$SOURCE_DIR" ]; then
    echo "Error: Source directory not found: $SOURCE_DIR"
    exit 1
fi

# Create target directory if it doesn't exist
echo "Creating target directory..."
sudo mkdir -p "$TARGET_DIR"

# Copy OpenMCT files
echo "Copying OpenMCT files..."
sudo cp -r "$SOURCE_DIR"/* "$TARGET_DIR/"

# Set appropriate permissions
echo "Setting permissions..."
sudo chown -R www-data:www-data "$TARGET_DIR"
sudo chmod -R 755 "$TARGET_DIR"
sudo find "$TARGET_DIR" -type f -exec chmod 644 {} \;

# Verify deployment
echo ""
echo "Verifying deployment..."
if [ -f "$TARGET_DIR/index.html" ] && [ -f "$TARGET_DIR/openmct-config.js" ]; then
    echo "✅ Deployment successful!"
    echo ""
    echo "OpenMCT is now available at:"
    echo "  https://alpha.lunco.space/mcc/"
    echo ""
    echo "Files deployed:"
    ls -lh "$TARGET_DIR"
else
    echo "❌ Deployment verification failed!"
    echo "Expected files not found in $TARGET_DIR"
    exit 1
fi

echo ""
echo "=== Deployment completed successfully ==="
