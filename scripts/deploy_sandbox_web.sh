#!/usr/bin/env bash
# ============================================================================
# LunCoSim — deploy sandbox web client (wasm)
# ============================================================================
# Usage:
#     ./scripts/deploy_sandbox_web.sh <user@host[:path]> [custom_path]
#
# Environment variables:
#     SSH_PORT       non-default SSH port
#     EXTRA_RSYNC    extra rsync args, e.g. "-n" for dry-run
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${1:-}"

if [ -z "$TARGET" ]; then
    echo "Usage: $0 <user@host[:path]> [custom_path]" >&2
    exit 2
fi

exec "$SCRIPT_DIR/deploy_web.sh" "$TARGET" sandbox "${2:-}"
