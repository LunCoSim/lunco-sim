#!/usr/bin/env bash
#
# Run GPU-backed scene tests whose Rhai observer declares
# `const TEST_KIND = "render-contract"`.
#
# These tests run the production offscreen renderer and observe authored
# render diagnostics through their Rhai verdict. They deliberately do not arm
# the frame recorder: a negative shader/material fixture may have no valid
# color-phase item, and accepting an empty capture would weaken the ordinary
# graphics readiness contract.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

export LUNCOSIM_EPHEMERAL_SETTINGS=1
export LUNCOSIM_ISOLATED_RUN=1

BIN="${LUNCOSIM_BIN:-target/debug/luncosim}"
CONTRACT_TIMEOUT="${RENDER_CONTRACT_TIMEOUT:-120}"
API_PORT="${RENDER_CONTRACT_API_PORT:-4800}"
LOG_DIR="target/scene-tests"

if [[ ! -x "$BIN" ]]; then
    echo "render-contract gate: production binary is missing or not executable: $BIN" >&2
    exit 2
fi
if ! [[ "$CONTRACT_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
    echo "render-contract gate: RENDER_CONTRACT_TIMEOUT must be a positive integer" >&2
    exit 2
fi
if ! [[ "$API_PORT" =~ ^[1-9][0-9]*$ && "$API_PORT" -le 65535 ]]; then
    echo "render-contract gate: RENDER_CONTRACT_API_PORT must be a valid TCP port" >&2
    exit 2
fi

FILTER=""
EXACT=0
while (($# > 0)); do
    case "$1" in
        --exact)
            EXACT=1
            shift
            ;;
        --*)
            echo "render-contract gate: unknown option: $1" >&2
            exit 2
            ;;
        *)
            if [[ -n "$FILTER" ]]; then
                echo "render-contract gate: only one scene filter is supported" >&2
                exit 2
            fi
            FILTER="$1"
            shift
            ;;
    esac
done
if ((EXACT)) && [[ -z "$FILTER" ]]; then
    echo "render-contract gate: --exact needs a scene name or path" >&2
    exit 2
fi

LIST_OUTPUT="$($BIN test --list)" || {
    echo "render-contract gate: scene test discovery failed" >&2
    exit 2
}
SCENES=()
while IFS=$'\t' read -r kind scene; do
    [[ "$kind" == "render-contract" && -n "${scene:-}" ]] || continue
    if [[ -z "$FILTER" ]]; then
        SCENES+=("$scene")
    elif ((EXACT)); then
        [[ "$(basename "$scene" .usda)" == "$FILTER" || "$scene" == "$FILTER" ]] && SCENES+=("$scene")
    else
        [[ "$scene" == *"$FILTER"* ]] && SCENES+=("$scene")
    fi
done <<< "$LIST_OUTPUT"

if [[ ${#SCENES[@]} -eq 0 ]]; then
    echo "render-contract gate: no render-contract scene matches '${FILTER:-all}'" >&2
    exit 2
fi

mkdir -p "$LOG_DIR"
overall=0
passed=0
index=0
for scene in "${SCENES[@]}"; do
    name="$(basename "$scene" .usda)"
    log="$LOG_DIR/$name.render-contract.log"
    port=$((API_PORT + index))
    index=$((index + 1))
    echo "==> render-contract $name"

    "$BIN" --api "$port" --offscreen --render-quality high --scene "$scene" </dev/null >"$log" 2>&1 &
    pid=$!
    started_at="$(date +%s)"
    verdict=""
    while kill -0 "$pid" 2>/dev/null; do
        if grep -Fq 'TESTS_FAIL ' "$log"; then
            verdict="FAIL"
            break
        fi
        if grep -Fq 'TESTS_OK ' "$log"; then
            verdict="PASS"
            break
        fi
        now="$(date +%s)"
        if ((now - started_at >= CONTRACT_TIMEOUT)); then
            verdict="TIMEOUT"
            break
        fi
        sleep 0.25
    done

    if [[ -n "$verdict" ]]; then
        kill -TERM "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    else
        wait "$pid" 2>/dev/null || true
        if grep -Fq 'TESTS_FAIL ' "$log"; then
            verdict="FAIL"
        elif grep -Fq 'TESTS_OK ' "$log"; then
            verdict="PASS"
        else
            verdict="EXITED"
        fi
    fi

    case "$verdict" in
        PASS)
            passed=$((passed + 1))
            echo "    PASS — production render-contract verdict; log=$log"
            ;;
        FAIL)
            overall=1
            echo "    FAIL — production render-contract verdict; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
        TIMEOUT)
            overall=1
            echo "    FAIL — render-contract verdict did not arrive within ${CONTRACT_TIMEOUT}s; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
        *)
            overall=1
            echo "    FAIL — render-contract process exited without a verdict; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
    esac
done

echo "render-contract gate: ${passed}/${#SCENES[@]} passed"
exit "$overall"
