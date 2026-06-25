#!/bin/bash

set -e

# Check if domain argument is provided
if [ -z "$1" ]; then
    echo "Error: Domain name is required."
    echo "Usage: $0 <domain>"
    echo "Example: $0 lunica.lunco.space"
    exit 1
fi

DOMAIN="$1"
# Default to /var/www/html as it's standard on Linux web servers.
# You can change this base path if your server uses a different location (e.g. /var/www or /usr/share/nginx/html)
TARGET_DIR="/var/www/html/$DOMAIN"

echo "Starting deployment for domain: $DOMAIN"
echo "Target directory: $TARGET_DIR"

# Ensure target directory exists
if [ ! -d "$TARGET_DIR" ]; then
    echo "Creating target directory: $TARGET_DIR"
    mkdir -p "$TARGET_DIR"
fi

# Clean the target directory
echo "Cleaning the target directory..."
find "$TARGET_DIR" -mindepth 1 -delete

# Move files to target directory
echo "Moving files to $TARGET_DIR..."

SCRIPT_NAME=$(basename "$0")
MOVED_COUNT=0

# Iterate through all files and directories, including hidden ones
for file in * .*; do
    # Skip standard current/parent directory references
    if [ "$file" = "." ] || [ "$file" = ".." ]; then
        continue
    fi
    
    # Skip the deployment script itself
    if [ "$file" = "$SCRIPT_NAME" ]; then
        continue
    fi

    # Only attempt to move if the file/folder actually exists
    # (this prevents errors if * or .* expands literally when no files match)
    if [ -e "$file" ]; then
        mv "$file" "$TARGET_DIR/"
        MOVED_COUNT=$((MOVED_COUNT + 1))
    fi
done

if [ $MOVED_COUNT -eq 0 ]; then
    echo "Warning: No files were found to move!"
else
    echo "Deployment completed successfully! Moved $MOVED_COUNT items to $TARGET_DIR."
fi
