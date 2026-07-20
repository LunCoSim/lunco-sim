#!/usr/bin/env bash
#
# run_scene_tests.sh — build the headless scene-test runner ONCE, then run every
# scene test and report a summary table.
#
# Each scene is an authored USD file whose attached rhai scenario ends in
# `emit("<CHANNEL>", "PASS"|"FAIL")`. `scene_test` runs it headless and
# deterministically (manual clock, no window, no GPU, no realtime pacing) and
# exits 0 = PASS, 1 = FAIL, 2 = no verdict. This script aggregates those.
#
#   ./scripts/run_scene_tests.sh              # all scenes
#   ./scripts/run_scene_tests.sh drivetrain   # only scenes matching a substring
#
# Exits non-zero if ANY scene fails or produces no verdict.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

# ── The scene list ──────────────────────────────────────────────────────────
#
# Paths are relative to `assets/`, exactly as `--scene` wants them.
#
# TODO: these live under `scenes/sandbox/` for historical reasons — they were
# written as things to open in the GUI sandbox. They are TESTS, and they should
# move to `assets/scenes/tests/`. Once they do, replace this array with a glob
# over `assets/scenes/tests/*.usda` so a new test scene is picked up by existing
# and needs no edit here.
SCENES=(
    "scenes/sandbox/drivetrain_parity.usda"
    "scenes/sandbox/ackermann_parity.usda"
    "scenes/sandbox/six_independent_parity.usda"
)

# Optional substring filter from $1.
FILTER="${1:-}"
if [[ -n "$FILTER" ]]; then
    filtered=()
    for s in "${SCENES[@]}"; do
        [[ "$s" == *"$FILTER"* ]] && filtered+=("$s")
    done
    SCENES=("${filtered[@]}")
    if [[ ${#SCENES[@]} -eq 0 ]]; then
        echo "no scene matches filter '$FILTER'" >&2
        exit 2
    fi
fi

# ── Build ONCE ──────────────────────────────────────────────────────────────
#
# `-j 2` is a machine constraint (see feedback_cargo_resource_use), and this is
# the ONLY cargo invocation in the script — the runs below execute the built
# binary directly. Two concurrent cargo processes would contend for the same
# target-dir lock and serialise anyway, so the scene runs are sequential too.
BIN="target/debug/scene_test"
echo "==> building scene_test (one cargo invocation, -j 2)"
if ! cargo build -q -p lunco-sandbox --bin scene_test -j 2; then
    echo "BUILD FAILED — no scenes run" >&2
    exit 2
fi
if [[ ! -x "$BIN" ]]; then
    echo "build reported success but $BIN is missing" >&2
    exit 2
fi

# ── Run each scene ──────────────────────────────────────────────────────────
LOG_DIR="${TMPDIR:-/tmp}/lunco-scene-tests"
mkdir -p "$LOG_DIR"

names=()
statuses=()
details=()
overall=0

for scene in "${SCENES[@]}"; do
    name="$(basename "$scene" .usda)"
    log="$LOG_DIR/$name.log"
    echo "==> $name"

    # The runner self-terminates on its own `--max-ticks` bound, so there is no
    # external `timeout` here: a hang inside the sim is already an exit-2, and
    # wrapping it would only mask which of the two happened.
    "$BIN" --scene "$scene" >"$log" 2>&1
    code=$?

    # The one-line summary `scene_test` prints last; falls back to the exit code.
    summary="$(grep -E '^scene_test (PASS|FAIL|NO-VERDICT)' "$log" | tail -1)"

    case $code in
        0) status="PASS" ;;
        1) status="FAIL" ; overall=1 ;;
        2) status="NO-VERDICT" ; overall=1 ;;
        *) status="ERROR($code)" ; overall=1 ;;
    esac

    names+=("$name")
    statuses+=("$status")
    details+=("${summary:-see $log}")

    if [[ "$status" != "PASS" ]]; then
        echo "    $status — last 20 log lines:"
        tail -20 "$log" | sed 's/^/    | /'
    fi
done

# ── Summary table ───────────────────────────────────────────────────────────
echo
echo "==================== scene test summary ===================="
printf '%-28s %-12s %s\n' "SCENE" "RESULT" "DETAIL"
for i in "${!names[@]}"; do
    printf '%-28s %-12s %s\n' "${names[$i]}" "${statuses[$i]}" "${details[$i]}"
done
echo "============================================================"
echo "logs: $LOG_DIR"

if [[ $overall -eq 0 ]]; then
    echo "ALL ${#names[@]} SCENE TESTS PASSED"
else
    echo "SOME SCENE TESTS FAILED"
fi
exit $overall
