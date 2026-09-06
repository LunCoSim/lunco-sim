#!/usr/bin/env bash
#
# Run authored editor-domain scene tests through the production windowed
# luncosim. These tests exercise document/preview/selection APIs that are
# intentionally absent from the headless and offscreen hosts.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

BIN="${LUNCOSIM_BIN:-target/debug/luncosim}"
EDITOR_TIMEOUT="${EDITOR_TIMEOUT:-120}"
EDITOR_API_PORT="${EDITOR_API_PORT:-4700}"
LOG_DIR="target/scene-tests"

if [[ ! -x "$BIN" ]]; then
    echo "editor scene gate: production binary is missing or not executable: $BIN" >&2
    exit 2
fi
if ! [[ "$EDITOR_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
    echo "editor scene gate: EDITOR_TIMEOUT must be a positive integer" >&2
    exit 2
fi
if ! [[ "$EDITOR_API_PORT" =~ ^[1-9][0-9]*$ && "$EDITOR_API_PORT" -le 65535 ]]; then
    echo "editor scene gate: EDITOR_API_PORT must be a valid TCP port" >&2
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
            echo "editor scene gate: unknown option: $1" >&2
            exit 2
            ;;
        *)
            if [[ -n "$FILTER" ]]; then
                echo "editor scene gate: only one scene filter is supported" >&2
                exit 2
            fi
            FILTER="$1"
            shift
            ;;
    esac
done
if ((EXACT)) && [[ -z "$FILTER" ]]; then
    echo "editor scene gate: --exact needs a scene name or path" >&2
    exit 2
fi

LIST_OUTPUT="$($BIN test --list)" || {
    echo "editor scene gate: scene test discovery failed" >&2
    exit 2
}
SCENES=()
while IFS=$'\t' read -r kind scene; do
    [[ "$kind" == "editor" && -n "${scene:-}" ]] || continue
    SCENES+=("$scene")
done <<< "$LIST_OUTPUT"

if [[ -n "$FILTER" ]]; then
    filtered=()
    for scene in "${SCENES[@]}"; do
        if ((EXACT)); then
            [[ "$(basename "$scene" .usda)" == "$FILTER" || "$scene" == "$FILTER" ]] && filtered+=("$scene")
        else
            [[ "$scene" == *"$FILTER"* ]] && filtered+=("$scene")
        fi
    done
    SCENES=("${filtered[@]}")
fi
if [[ ${#SCENES[@]} -eq 0 ]]; then
    echo "editor scene gate: no editor scene matches '${FILTER:-all}'" >&2
    exit 2
fi

mkdir -p "$LOG_DIR"
overall=0
passed=0
index=0
for scene in "${SCENES[@]}"; do
    name="$(basename "$scene" .usda)"
    log="$LOG_DIR/$name.editor.log"
    port=$((EDITOR_API_PORT + index))
    index=$((index + 1))
    echo "==> editor $name"

    # This is a production windowed process, not a mock editor. Closing stdin
    # is part of the process contract: a background GUI process must not let
    # the Rhai REPL consume the controlling terminal or receive SIGTTIN.
    "$BIN" --api "$port" --scene "$scene" </dev/null >"$log" 2>&1 &
    pid=$!
    started_at="$(date +%s)"
    verdict=""
    while kill -0 "$pid" 2>/dev/null; do
        if grep -Fq '[rhai] TESTS_FAIL' "$log"; then
            verdict="FAIL"
            break
        fi
        if grep -Fq '[rhai] TESTS_OK' "$log"; then
            verdict="PASS"
            break
        fi
        now="$(date +%s)"
        if ((now - started_at >= EDITOR_TIMEOUT)); then
            verdict="TIMEOUT"
            break
        fi
        sleep 0.25
    done

    if [[ -n "$verdict" ]]; then
        kill -TERM "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    else
        wait "$pid" || true
        verdict="EXITED"
    fi

    case "$verdict" in
        PASS)
            passed=$((passed + 1))
            echo "    PASS — production editor verdict; log=$log"
            ;;
        FAIL)
            overall=1
            echo "    FAIL — production editor verdict; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
        TIMEOUT)
            overall=1
            echo "    FAIL — editor verdict did not arrive within ${EDITOR_TIMEOUT}s; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
        *)
            overall=1
            echo "    FAIL — editor process exited without a verdict; log=$log"
            tail -20 "$log" | sed 's/^/    | /'
            ;;
    esac
done

echo "editor scene gate: ${passed}/${#SCENES[@]} passed"
exit "$overall"
