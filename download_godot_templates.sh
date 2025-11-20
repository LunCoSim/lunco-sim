#!/bin/bash

# Script to download and install Godot export templates for a specified version
# This script downloads both release and debug templates and installs them
# to the correct location where Godot expects to find them.
#
# Usage: ./download_godot_templates.sh <godot_version>
# Example: ./download_godot_templates.sh 4.3

set -e  # Exit on error

# Check if GODOT version is provided
if [ $# -ne 1 ]; then
    echo "Error: Godot version must be provided as a command-line argument."
    echo "Usage: $0 <godot_version>"
    echo "Example: $0 4.3"
    exit 1
fi

GODOT_VERSION="$1"
TAG="${GODOT_VERSION}-stable"

# Official Godot export templates download URL
# This TPZ file contains both release and debug templates for all platforms
TEMPLATES_URL="https://github.com/godotengine/godot-builds/releases/download/${TAG}/Godot_v${TAG}_export_templates.tpz"
OUTPUT_FILE="Godot_v${TAG}_export_templates.tpz"

# Godot expects templates in this directory structure
TEMPLATES_DIR="${HOME}/.local/share/godot/export_templates/${GODOT_VERSION}.stable"

echo "=========================================="
echo "Godot Export Templates Installer"
echo "=========================================="
echo "Version: ${GODOT_VERSION}"
echo "Download URL: ${TEMPLATES_URL}"
echo "Install Path: ${TEMPLATES_DIR}"
echo ""

# Download the templates
echo "[1/4] Downloading export templates..."
if wget -O "${OUTPUT_FILE}" "${TEMPLATES_URL}"; then
    echo "✓ Download completed successfully"
else
    echo "✗ Error: Failed to download templates."
    echo "  Please check the version number or network connection."
    exit 1
fi

# Create the templates directory
echo ""
echo "[2/4] Creating templates directory..."
mkdir -p "${TEMPLATES_DIR}"
echo "✓ Directory created: ${TEMPLATES_DIR}"

# Extract the templates (TPZ is just a ZIP file)
echo ""
echo "[3/4] Extracting templates..."
if unzip -q -o "${OUTPUT_FILE}" -d "${TEMPLATES_DIR}"; then
    echo "✓ Templates extracted successfully"
else
    echo "✗ Error: Failed to extract templates"
    exit 1
fi

# Move templates from the 'templates' subdirectory to the version directory
# The TPZ file extracts to a 'templates' folder, but Godot expects them directly in the version folder
if [ -d "${TEMPLATES_DIR}/templates" ]; then
    echo ""
    echo "[4/4] Installing templates..."
    mv "${TEMPLATES_DIR}/templates"/* "${TEMPLATES_DIR}/"
    rmdir "${TEMPLATES_DIR}/templates"
    echo "✓ Templates installed successfully"
fi

# Clean up the downloaded file
rm "${OUTPUT_FILE}"

echo ""
echo "=========================================="
echo "Installation Complete!"
echo "=========================================="
echo "Templates installed to: ${TEMPLATES_DIR}"
echo ""
echo "The following templates are now available:"
ls -1 "${TEMPLATES_DIR}" | grep -E '\.(zip|tpz|so|dylib|dll|wasm|exe|apk)$' || ls -1 "${TEMPLATES_DIR}"
echo ""
echo "Both release and debug templates are included."
echo "You can now export your Godot project for various platforms."
echo "=========================================="
