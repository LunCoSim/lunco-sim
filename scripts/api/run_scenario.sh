#!/usr/bin/env bash
# Attach or hot-reload a file-authored Rhai scenario through the live API.
#
# Usage:
#   ./scripts/api/run_scenario.sh <target-api-id> <scenario.rhai> [port] [params-json]
#
# RunScenario.source is source text, so this wrapper reads the file and creates
# the same request an editor/MCP client would send. It never builds Rust or
# launches a second simulator session.

set -euo pipefail

if (( $# < 2 || $# > 4 )); then
    echo "usage: $0 <target-api-id> <scenario.rhai> [port] [params-json]" >&2
    exit 2
fi

TARGET="$1"
SOURCE_FILE="$2"
PORT="${3:-4101}"
PARAMS="${4:-{}}"
BASE="http://127.0.0.1:${PORT}/api"

if [[ ! -f "$SOURCE_FILE" ]]; then
    echo "scenario file does not exist: $SOURCE_FILE" >&2
    exit 2
fi

if ! jq -e . >/dev/null <<<"$PARAMS"; then
    echo "params must be valid JSON: $PARAMS" >&2
    exit 2
fi

request="$({
    jq -Rs \
        --arg target "$TARGET" \
        --arg params "$PARAMS" \
        '{type:"ExecuteCommand",command:"RunScenario",params:{target:($target | tonumber),source:.,params:$params}}' \
        "$SOURCE_FILE"
})"

curl --fail-with-body --silent --show-error \
    -X POST "$BASE/commands" \
    -H 'content-type: application/json' \
    --data-binary "$request" | jq .
