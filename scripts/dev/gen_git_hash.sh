#!/bin/bash
# Generate a git_hash.txt file containing the current short commit hash
# Used for build versioning and identification

echo "Generating git_hash.txt..."
git rev-parse --short HEAD > git_hash.txt
echo "git_hash.txt generated: $(cat git_hash.txt)"
