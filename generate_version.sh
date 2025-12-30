#!/bin/bash

# Generate git_hash.txt with the current short commit hash
echo "Generating git_hash.txt..."
git rev-parse --short HEAD > git_hash.txt
echo "git_hash.txt generated: $(cat git_hash.txt)"
