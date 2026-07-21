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
#   ./scripts/run_scene_tests.sh --stress     # + optional diagnostic second pass
#
# Exits non-zero if ANY scene fails or produces no verdict IN THE GATE PASS.
#
# ── The gate pass vs the --stress pass ──────────────────────────────────────
#
# The GATE runs every scene with `--threads 1 --jitter 0`: one compute thread
# and an exactly-fixed manual dt. That combination is bit-reproducible, so a
# red here is a real, re-runnable regression.
#
# `--stress` adds a SECOND, clearly separated pass over the same scenes with
# `--threads 0` (bevy's default multi-threaded pool, as the GUI runs) and
# `--jitter 0.4` (seeded pseudo-random dt, modelling realtime frame pacing).
# That pass exists because `scenes/sandbox/drivetrain_parity.usda` passes
# headless and explodes under the GUI, and those two flags are the two known
# differences. Reading the stress pass:
#
#   red only with jitter   => dt-sensitivity bug, not a threading bug
#   red only with threads  => ordering/race bug in the parallel solver path
#
# The stress pass is reported SEPARATELY and does NOT affect the exit code. It
# is diagnostic, not a gate: multi-threading is by construction not run-to-run
# reproducible, so gating on it would make the build flaky, and until we know
# what a jittered failure means it must not be able to turn CI red.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

# ── Stress-pass configuration ───────────────────────────────────────────────
STRESS=0
STRESS_THREADS=0     # 0 = leave bevy's default multi-threaded pool alone
STRESS_JITTER=0.4    # +/- 40% dt, i.e. frame times from 10 ms to 23 ms at 60 Hz
STRESS_SEED=12345    # FIXED: a stress failure must be replayable verbatim

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
    "scenes/sandbox/parts_attached.usda"
)

# Args: any `--stress` anywhere enables the diagnostic pass; the first remaining
# positional is the substring filter.
FILTER=""
for arg in "$@"; do
    case "$arg" in
        --stress) STRESS=1 ;;
        -h|--help)
            sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) [[ -z "$FILTER" ]] && FILTER="$arg" ;;
    esac
done

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

echo "==> GATE pass: --threads 1 --jitter 0 (deterministic, this is what gates)"
for scene in "${SCENES[@]}"; do
    name="$(basename "$scene" .usda)"
    log="$LOG_DIR/$name.log"
    echo "==> $name"

    # The runner self-terminates on its own `--max-ticks` bound, so there is no
    # external `timeout` here: a hang inside the sim is already an exit-2, and
    # wrapping it would only mask which of the two happened.
    #
    # The flags are PASSED EXPLICITLY even though they are the binary's defaults:
    # the gate's determinism must not silently change if a default ever moves.
    "$BIN" --scene "$scene" --threads 1 --jitter 0 >"$log" 2>&1
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

# ── OPTIONAL stress pass — DIAGNOSTIC ONLY, never gates ─────────────────────
if [[ $STRESS -eq 1 ]]; then
    echo
    echo "==> STRESS pass (DIAGNOSTIC — does NOT affect the exit code)"
    echo "    --threads $STRESS_THREADS (bevy default pool)  --jitter $STRESS_JITTER  --seed $STRESS_SEED"
    echo "    A scene GREEN in the gate and RED here is dt-sensitive and/or order-sensitive,"
    echo "    which is the class of bug that only shows up under the GUI."

    s_names=()
    s_statuses=()
    s_details=()

    for scene in "${SCENES[@]}"; do
        name="$(basename "$scene" .usda)"
        log="$LOG_DIR/$name.stress.log"
        echo "==> $name (stress)"

        "$BIN" --scene "$scene" \
            --threads "$STRESS_THREADS" \
            --jitter "$STRESS_JITTER" \
            --seed "$STRESS_SEED" >"$log" 2>&1
        code=$?

        summary="$(grep -E '^scene_test (PASS|FAIL|NO-VERDICT)' "$log" | tail -1)"
        case $code in
            0) status="PASS" ;;
            1) status="FAIL" ;;
            2) status="NO-VERDICT" ;;
            *) status="ERROR($code)" ;;
        esac

        s_names+=("$name")
        s_statuses+=("$status")
        s_details+=("${summary:-see $log}")
    done

    echo
    echo "============ stress pass (diagnostic, NOT a gate) =========="
    printf '%-28s %-12s %s\n' "SCENE" "STRESS" "DETAIL"
    for i in "${!s_names[@]}"; do
        printf '%-28s %-12s %s\n' "${s_names[$i]}" "${s_statuses[$i]}" "${s_details[$i]}"
    done
    echo "============================================================"
    echo "stress logs: $LOG_DIR/*.stress.log"
    echo "reproduce any stress failure verbatim:"
    echo "  $BIN --scene <SCENE> --threads $STRESS_THREADS --jitter $STRESS_JITTER --seed $STRESS_SEED"
    echo "(note: --threads $STRESS_THREADS is multi-threaded and therefore NOT bit-reproducible;"
    echo " re-run with --threads 1 --jitter $STRESS_JITTER to isolate dt-sensitivity alone.)"
fi

if [[ $overall -eq 0 ]]; then
    echo "ALL ${#names[@]} SCENE TESTS PASSED (gate pass)"
else
    echo "SOME SCENE TESTS FAILED (gate pass)"
fi
exit $overall
