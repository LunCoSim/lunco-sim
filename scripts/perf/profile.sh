#!/usr/bin/env bash
#
# profile.sh — capture a CPU profile of a LunCoSim binary's render loop and
# flatten it to per-function self-time. The entry point of the perf subsystem.
#
# Why: our binaries run a real-time 3D scene + Avian physics + an embedded egui
# IDE on one frame loop. When FPS drops, a sampling profiler is the only
# reliable way to find the dominant cost — code reading sends you optimising a
# 0.2 ms system while a 5 ms one hides behind it (that is exactly how the
# 2026-05-29 cosim-clone regression was missed until profiled).
#
# What it does:
#   1. builds the target binary (dev = fast + full debuginfo; --release = the
#      `profiling` profile = release codegen + line tables).
#   2. runs it under `samply record` with `--no-vsync` (PresentMode::Mailbox) so
#      frame pacing can't mask CPU cost, and `--log-diag` for frame_time/FPS.
#   3. records a fixed window, then SIGINTs samply's child so samply finalises a
#      clean capture (samply only writes when its child exits; our binaries
#      ignore the API `Exit`, so we signal the child PID directly — never the
#      user's own instance on another port).
#   4. prints the wgpu adapter, frame-time, and the symbolicated hot functions
#      (steady-state only, via symbolicate_samply.py).
#
# Requires for a per-function profile: `samply` on PATH and
# `kernel.perf_event_paranoid <= 1` (one-time: echo 1 | sudo tee
# /proc/sys/kernel/perf_event_paranoid). Without it the script auto-falls back
# to a diagnostics-only run (frame time + adapter, no per-function cost).
#
# Usage:
#   scripts/perf/profile.sh [--bin NAME] [--duration N] [--warmup N]
#                           [--port P] [--scene PATH] [--release]
#                           [--no-build] [--diag-only]
#
# Captures land in scripts/perf/captures/ (git-ignored). Commit fixes and a
# one-line before/after in your PR — never the multi-MB capture.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

BIN_NAME=sandbox       # any cargo bin (sandbox, lunica, …)
DURATION=20            # seconds of profiling after warmup
WARMUP=8               # seconds to let the window open + scene load
PORT=3001              # test port (3000 is the user's own session)
SCENE=""               # empty → binary's default scene
PROFILE="dev"          # dev = fast rebuild; profiling = release codegen
BUILD=1
DIAG_ONLY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin)       BIN_NAME="$2"; shift 2 ;;
    --duration)  DURATION="$2"; shift 2 ;;
    --warmup)    WARMUP="$2"; shift 2 ;;
    --port)      PORT="$2"; shift 2 ;;
    --scene)     SCENE="$2"; shift 2 ;;
    --release)   PROFILE="profiling"; shift ;;
    --no-build)  BUILD=0; shift ;;
    --diag-only) DIAG_ONLY=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# samply needs perf_event_paranoid <= 1 for a non-root user. Detect early and
# degrade to a diagnostics-only run rather than launching the app for nothing.
PARANOID="$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 99)"
if [[ "$DIAG_ONLY" != "1" && "$PARANOID" -gt 1 ]]; then
  echo "!! perf_event_paranoid=$PARANOID (>1) — samply can't sample as non-root."
  echo "!! One-time fix:  echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid"
  echo "!! Falling back to --diag-only (frame time + adapter, no per-fn cost)."
  DIAG_ONLY=1
fi

OUT_DIR="scripts/perf/captures"
mkdir -p "$OUT_DIR"
STAMP="$(date +%Y%m%d-%H%M%S)"
CAPTURE="$OUT_DIR/$BIN_NAME-$PROFILE-$STAMP.json.gz"
APPLOG="$OUT_DIR/$BIN_NAME-$PROFILE-$STAMP.app.log"

if [[ "$PROFILE" == "profiling" ]]; then
  BIN="target/profiling/$BIN_NAME"; BUILD_ARGS=(--profile profiling)
else
  BIN="target/debug/$BIN_NAME"; BUILD_ARGS=()
fi

if [[ "$BUILD" == "1" ]]; then
  echo ">> building $BIN_NAME ($PROFILE, -j2) …"
  cargo build -j 2 --bin "$BIN_NAME" "${BUILD_ARGS[@]}"
fi

ARGS=(--api "$PORT" --no-vsync --log-diag)
[[ -n "$SCENE" ]] && ARGS+=(--scene "$SCENE")

# Keep the wgpu adapter line (bevy_render INFO) but silence the per-frame
# wgpu_hal "Suboptimal present" WARN spam (Wayland) — otherwise log formatting
# pollutes the profile and the app.log balloons to 10k+ lines.
export RUST_LOG="${RUST_LOG:-wgpu=error,wgpu_hal=error,info}"

RUN_SECS=$((WARMUP + DURATION))
echo ">> running ${RUN_SECS}s (${WARMUP}s warmup + ${DURATION}s window) — auto-stops, no manual exit"
echo ">> app log → $APPLOG"
[[ "$DIAG_ONLY" != "1" ]] && echo ">> capture → $CAPTURE"

if [[ "$DIAG_ONLY" == "1" ]]; then
  # No profiler — run the app and harvest Bevy LogDiagnostics; SIGINT stops it.
  timeout --signal=INT --kill-after=10s "${RUN_SECS}s" \
    "$BIN" "${ARGS[@]}" 2>&1 | tee "$APPLOG" || true
else
  # samply only writes the capture when its child exits, and the app never
  # exits on its own. Record in the background, then SIGINT *samply's direct
  # child* (the app) by PID — never by name, so we can't touch the user's own
  # instance on a different port.
  samply record --save-only -o "$CAPTURE" -- "$BIN" "${ARGS[@]}" > "$APPLOG" 2>&1 &
  SAMPLY_PID=$!
  sleep "$RUN_SECS"
  CHILD="$(pgrep -P "$SAMPLY_PID" | head -1)"
  [[ -n "$CHILD" ]] && kill -INT "$CHILD"
  wait "$SAMPLY_PID" 2>/dev/null || true
fi

echo
echo ">> wgpu adapter:"
grep -iE "adapterinfo|backend:" "$APPLOG" | head -3 || true
echo ">> frame-time / FPS (steady-state tail):"
grep -iE "frame_time|^.* fps " "$APPLOG" | tail -6 || true
echo

if [[ "$DIAG_ONLY" == "1" ]]; then
  echo "(diag-only — no per-function profile; lower perf_event_paranoid for that)"
else
  echo "===================== HOT FUNCTIONS (steady-state) ====================="
  # Drop the warmup window so scene-load noise doesn't dominate the ranking.
  python3 scripts/perf/symbolicate_samply.py "$CAPTURE" 40 --skip-start "$WARMUP"
fi
