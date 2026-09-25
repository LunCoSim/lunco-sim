#!/usr/bin/env bash
#
# Run authored editor-domain scene tests through the production windowed
# luncosim. These tests exercise document/preview/selection APIs that are
# intentionally absent from the headless and offscreen hosts.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

# Editor acceptance runs are throwaway by design. Do not load or write a
# developer's settings or Twin runtime overlay while exercising a fixture.
export LUNCOSIM_EPHEMERAL_SETTINGS=1
export LUNCOSIM_ISOLATED_RUN=1
export LUNCOSIM_CONFIG="$REPO_ROOT/target/scene-tests/config-editor-suite-$$"

BIN="${LUNCOSIM_BIN:-target/debug/luncosim}"
export LUNCOSIM_BIN="$BIN"
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
if ((EDITOR_API_PORT + ${#SCENES[@]} - 1 > 65535)); then
    echo "editor scene gate: selected scenes exceed the available API port range" >&2
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
    if LUNCOSIM_CONFIG="$LOG_DIR/config-${name}-$$-$index" \
        python3 scripts/api/run_editor_scene_test.py \
        --port "$port" \
        --timeout "$EDITOR_TIMEOUT" \
        --scene "$scene" \
        --log "$log"; then
        passed=$((passed + 1))
    else
        overall=1
    fi
done

echo "editor scene gate: ${passed}/${#SCENES[@]} passed"
exit "$overall"
