#!/usr/bin/env bash
#
# run_scene_tests.sh — run every authored scene test with the production
# luncosim runner: deterministic headless Rhai tests plus the GPU-backed
# render tests declared with lunco:notHeadlessTestable.
#
# Each headless scene is an authored USD file whose attached Rhai scenario ends
# in `emit("<CHANNEL>", "PASS"|"FAIL")`. `luncosim test` runs it headless and
# deterministically (manual clock, no window, no GPU, no realtime pacing) and
# exits 0 = PASS, 1 = FAIL, 2 = no verdict. Render-only scenes run through
# `scripts/run_render_scene_tests.sh` using the same production binary in
# GPU-full offscreen mode.
#
#   ./scripts/run_scene_tests.sh              # all scenes
#   ./scripts/run_scene_tests.sh drivetrain   # only scenes matching a substring
#   ./scripts/run_scene_tests.sh --no-build   # reuse target/debug/luncosim
#   ./scripts/run_scene_tests.sh --bin /path/to/luncosim --no-build
#   ./scripts/run_scene_tests.sh --stress     # + optional diagnostic second pass
#
# Exits non-zero if ANY scene fails, produces no verdict, or hangs past
# SCENE_TIMEOUT (default 420s) IN THE GATE PASS.  SCENE_MAX_TICKS is the
# simulated-time liveness bound passed to every production scene run; it is
# deliberately independent of the wall-clock timeout because a valid tutorial
# route can need more than the binary's small interactive default.
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
# That pass exists because `scenes/tests/drivetrain_parity.usda` passes
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
BUILD=1
BIN="${LUNCOSIM_BIN:-target/debug/luncosim}"

# ── Per-scene wall-clock bound ──────────────────────────────────────────────
#
# Deliberately GENEROUS: this is a liveness backstop, not a performance budget.
# Its job is only to stop one wedged scene from taking the whole gate down with
# it; a scene slow enough to trip it is a finding either way. Override for a
# slower machine with `SCENE_TIMEOUT=900 ./scripts/run_scene_tests.sh`.
SCENE_TIMEOUT="${SCENE_TIMEOUT:-420}"
SCENE_MAX_TICKS="${SCENE_MAX_TICKS:-36000}"

# ── Arguments ───────────────────────────────────────────────────────────────
#
# The default is convenient for a clean checkout. `--no-build` is the important
# authoring path: after a Rust build has produced the production binary, edits
# to USD/Rhai rerun against that exact binary and do not compile or restart the
# core. A binary override is explicit so a caller cannot accidentally validate
# a stale executable from another worktree.
FILTER=""
while (($# > 0)); do
    case "$1" in
        --stress)
            STRESS=1
            shift
            ;;
        --no-build)
            BUILD=0
            shift
            ;;
        --build)
            BUILD=1
            shift
            ;;
        --bin)
            if (($# < 2)); then
                echo "--bin needs an executable path" >&2
                exit 2
            fi
            BIN="$2"
            shift 2
            ;;
        --bin=*)
            BIN="${1#--bin=}"
            shift
            ;;
        -h|--help)
            sed -n '2,46p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        --*)
            echo "unknown option: $1" >&2
            exit 2
            ;;
        *)
            if [[ -n "$FILTER" ]]; then
                echo "only one scene substring filter is supported" >&2
                exit 2
            fi
            FILTER="$1"
            shift
            ;;
    esac
done

# ── Scene → scenario graph preflight ────────────────────────────────────────
#
# `crates/lunco-scene-commands` owns the source-level contract. This small
# no-build mirror is deliberately kept in the runner because `--no-build` must
# catch a missing authored scenario before it launches the binary. It checks
# the actual asset edge, not merely the presence of a marker/API string.
check_scenario_graph() {
    local errors=0
    local scene ref
    local -a scenes refs

    mapfile -t scenes < <(find assets/scenes/tests -maxdepth 1 -type f -name '*.usda' -print | sort)
    for scene in "${scenes[@]}"; do
        if grep -q "lunco:notHeadlessTestable" "$scene"; then
            continue
        fi

        mapfile -t refs < <(
            sed -nE 's/.*info:sourceAsset = @lunco:\/\/(scenarios\/tests\/[^@]+\.rhai)@.*/\1/p' "$scene" \
                | sort -u
        )
        if [[ ${#refs[@]} -eq 0 ]]; then
            echo "SCENARIO GRAPH: $scene has no scenarios/tests/*.rhai sourceAsset" >&2
            errors=1
            continue
        fi

        for ref in "${refs[@]}"; do
            if [[ ! -f "assets/$ref" ]]; then
                echo "SCENARIO GRAPH: $scene references missing assets/$ref" >&2
                errors=1
            fi
        done
    done

    if ((errors)); then
        echo "SCENARIO GRAPH FAILED — repair authored scene→scenario edges before running" >&2
        return 1
    fi
}

if ! check_scenario_graph; then
    exit 2
fi

# ── The scene list ──────────────────────────────────────────────────────────
#
# Paths are relative to `assets/`, exactly as `--scene` wants them.
#
# DISCOVERED, never hand-listed: everything in `assets/scenes/tests/` is a test,
# and this gate runs all of it. A hand-written array is a place tests go to die —
# eleven scenes with real scenarios sat outside the old list, passing when run by
# hand and gating nothing.
#
# The directory IS the declaration. A scene is a test because of where it lives,
# not because its name ends in `_test`, and `lunco-assets`'s `is_test_asset` reads
# the same fact to keep them out of the UI's Scene menu.
#
# A test scene that asserts nothing is not skipped-and-forgotten either: it is
# caught by `lunco-scene-commands`'s `every_test_scene_carries_a_scenario`, which
# fails naming it. The two halves together leave nowhere for a silent test to sit.
# A scene that CANNOT return a headless verdict says so in itself, with a reason
# (`lunco:notHeadlessTestable` — the render checks, which need a GPU). Those
# scenes are not omitted: the GPU pass below discovers the same marker.
mapfile -t SCENES < <(
    grep -L "lunco:notHeadlessTestable" assets/scenes/tests/*.usda | sed 's|^assets/||' | sort
)
if [[ ${#SCENES[@]} -eq 0 ]]; then
    echo "assets/scenes/tests/ is empty — the discovery glob is wrong" >&2
    exit 2
fi
mapfile -t SKIPPED < <(
    grep -l "lunco:notHeadlessTestable" assets/scenes/tests/*.usda | sed 's|^assets/||' | sort
)
for s in "${SKIPPED[@]}"; do
    echo "==> QUEUE $(basename "$s" .usda) — GPU render assertion"
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

# ── Build once, or reuse the production binary ─────────────────────────────
#
# With BUILD=1 this is the ONLY cargo invocation in the script; the runs below
# execute the built binary directly. With BUILD=0 there is no Cargo access at
# all, which is the fast script/scene iteration path.
if ((BUILD)); then
    echo "==> building luncosim test runner (one cargo invocation, -j 4)"
    if ! RUSTC_WRAPPER=sccache cargo build -q -p lunco-luncosim --bin luncosim -j 4; then
        echo "BUILD FAILED — no scenes run" >&2
        exit 2
    fi
else
    echo "==> reusing production luncosim binary (--no-build): $BIN"
fi
if [[ ! -x "$BIN" ]]; then
    echo "production binary is missing or not executable: $BIN" >&2
    exit 2
fi

# ── Run each scene ──────────────────────────────────────────────────────────
LOG_DIR="target/scene-tests"
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

    # WALL-CLOCK bounded, because `--max-ticks` is not a bound on hanging.
    # `--max-ticks` can only fire between ticks; `scenes/tests/rover_comparison`
    # spins forever INSIDE a single physics step (a diverged body, one core at
    # 100%, no tick ever completes), so the runner never reaches its own check and
    # the gate wedges instead of reporting. A gate that can hang reports nothing at
    # all, which is strictly worse than reporting the wrong thing.
    #
    # The two outcomes stay DISTINGUISHABLE rather than collapsing into exit-2:
    # `timeout` returns 124, which maps to its own HUNG status below.
    #
    # The flags are PASSED EXPLICITLY even though they are the binary's defaults:
    # the gate's determinism must not silently change if a default ever moves.
    timeout --kill-after=10 "$SCENE_TIMEOUT" \
        "$BIN" test --scene "$scene" --max-ticks "$SCENE_MAX_TICKS" \
        --threads 1 --jitter 0 >"$log" 2>&1
    code=$?

    # The one-line summary `luncosim test` prints last; falls back to the exit code.
    summary="$(grep -E '^luncosim test (PASS|FAIL|NO-VERDICT)' "$log" | tail -1)"

    case $code in
        0) status="PASS" ;;
        1) status="FAIL" ; overall=1 ;;
        2) status="NO-VERDICT" ; overall=1 ;;
        # 124 is `timeout`'s own exit: the scene never finished. Named, not folded
        # into ERROR — a hang and a crash need different investigations.
        124) status="HUNG(${SCENE_TIMEOUT}s)" ; overall=1 ;;
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

# ── GPU render pass ─────────────────────────────────────────────────────────
# The two render-only scenes are a separate acceptance class because their
# assertions are pixels and render diagnostics, not physics telemetry. The
# helper still uses this already-built production binary and exits non-zero on
# missing assets, wrong pixels, pipeline warnings, hangs, or incomplete frames.
if [[ -z "$FILTER" || "hdri shader_fallback" == *"$FILTER"* ]]; then
    echo
    echo "==> GPU render pass (production offscreen renderer)"
    if ! LUNCOSIM_BIN="$BIN" "$REPO_ROOT/scripts/run_render_scene_tests.sh" "$FILTER"; then
        overall=1
    fi
fi

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

        timeout --kill-after=10 "$SCENE_TIMEOUT" \
            "$BIN" test --scene "$scene" --max-ticks "$SCENE_MAX_TICKS" \
            --threads "$STRESS_THREADS" \
            --jitter "$STRESS_JITTER" \
            --seed "$STRESS_SEED" >"$log" 2>&1
        code=$?

        summary="$(grep -E '^luncosim test (PASS|FAIL|NO-VERDICT)' "$log" | tail -1)"
        case $code in
            0) status="PASS" ;;
            1) status="FAIL" ;;
            2) status="NO-VERDICT" ;;
            124) status="HUNG(${SCENE_TIMEOUT}s)" ;;
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
    echo "  $BIN test --scene <SCENE> --threads $STRESS_THREADS --jitter $STRESS_JITTER --seed $STRESS_SEED"
    echo "(note: --threads $STRESS_THREADS is multi-threaded and therefore NOT bit-reproducible;"
    echo " re-run with --threads 1 --jitter $STRESS_JITTER to isolate dt-sensitivity alone.)"
fi

if [[ $overall -eq 0 ]]; then
    echo "ALL ${#names[@]} SCENE TESTS PASSED (gate pass)"
else
    echo "SOME SCENE TESTS FAILED (gate pass)"
fi
exit $overall
