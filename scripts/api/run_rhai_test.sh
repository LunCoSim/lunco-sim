#!/usr/bin/env bash
# Run a standalone Rhai test against an already-running production luncosim.
#
# This is the no-rebuild/no-restart path for tests that need the live world
# bridge. The test source is sent through RunRhai, so it observes the same
# commands and queries as an authored scenario. The process must already be
# running with an explicit --api port and, for USD tests, the required scene.
#
# Usage:
#   ./scripts/api/run_rhai_test.sh <port> <test.rhai> [probe-prim]
#
# Exit codes: 0 = TESTS_OK, 1 = TESTS_FAIL/no verdict, 2 = request/setup error.

set -euo pipefail

if (( $# < 2 || $# > 3 )); then
    echo "usage: $0 <port> <test.rhai> [probe-prim]" >&2
    exit 2
fi

PORT="$1"
TEST="$2"
PROBE="${3:-}"

if [[ ! -f "$TEST" ]]; then
    echo "Rhai test does not exist: $TEST" >&2
    exit 2
fi

LIB_SOURCE=""
while IFS= read -r -d '' library; do
    LIB_SOURCE+="$(<"$library")"
    LIB_SOURCE+=$'\n'
done < <(find assets/scripting/tests/lib -maxdepth 1 -type f -name '*.rhai' -print0 | sort -z)

PRELUDE="$LIB_SOURCE"
if [[ -n "$PROBE" ]]; then
    PROBE_LITERAL="$(jq -Rn --arg probe "$PROBE" '$probe')"
    PRELUDE+=$'\nconst PROBE = '
    PRELUDE+="$PROBE_LITERAL;"
fi

set +e
CODE="$PRELUDE"
CODE+=$'\n'
CODE+="$(<"$TEST")"
OUTPUT="$(target/debug/luncosim rhai --api "$PORT" --stdout -e "$CODE")"
CODE=$?
set -e

if [[ -n "$OUTPUT" ]]; then
    printf '%s\n' "$OUTPUT"
fi

if ((CODE != 0)); then
    exit "$CODE"
fi

LAST="$(printf '%s\n' "$OUTPUT" | tail -n 1)"
case "$LAST" in
    TESTS_OK\ *) exit 0 ;;
    TESTS_FAIL\ *) exit 1 ;;
    *)
        echo "Rhai test produced no terminal TESTS_OK/TESTS_FAIL verdict" >&2
        exit 1
        ;;
esac
