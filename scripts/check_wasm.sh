#!/bin/bash
# ============================================================================
# LunCoSim — wasm build gate
# ============================================================================
# Compile every wasm32 binary the workspace ships, with the same flags the
# release pipeline uses. Runs against the workspace as a CI step (and on
# demand locally) to catch wasm-incompatible dependencies / std API usage
# at PR time — well before they show up as a silent runtime failure in
# the browser.
#
# This is the single most effective regression guard for the asset-I/O
# policy (see docs/architecture/40-asset-io.md and clippy.toml): the
# linker sees the whole dep graph, so a transitive crate that pulls in
# mio / tokio-fs / std::thread will fail to link even when clippy is
# happy with our own source.
#
# Failure modes this gate has caught historically:
#   * lunco-api dep dragging axum/tokio/mio into the wasm build
#   * lunco-modelica MSL preloader binding to std::thread
#   * UsdComposer's sublayer reads through std::fs::read_to_string
#   * crossbeam-channel unconditionally pulled into lunco-scripting
#
# Usage:
#   scripts/check_wasm.sh          # full check, prints sizes
#   scripts/check_wasm.sh --quick  # only build, skip wasm-bindgen / size dump
#
# Exit codes:
#   0  — both wasm binaries built clean
#   non-zero — one binary failed; full cargo output streamed to stderr
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

QUICK=0
if [ "${1:-}" = "--quick" ]; then QUICK=1; fi

cd "$PROJECT_DIR"

# Ensure the wasm target is present before invoking cargo — otherwise the
# error message reads "linker `rust-lld` not found" instead of the
# actually-informative "the wasm32-unknown-unknown target is not
# installed".
if ! rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    echo "Installing wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

build_one() {
    local bin="$1"
    local crate="$2"
    echo
    echo "── building $bin ($crate) ─────────────────────────"
    # --cfg=web_sys_unstable_apis is required for wgpu's WebGPU backend
    # bindings; see build_web.sh for the long-form rationale.
    RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis" \
        cargo build \
            --profile web-dev \
            --target wasm32-unknown-unknown \
            --bin "$bin" \
            -p "$crate" \
            --no-default-features
}

# Build both wasm binaries. Failure short-circuits via `set -e`.
build_one lunica        lunco-modelica
build_one sandbox       lunco-sandbox
# The companion worker bundle for lunica. Off-thread Modelica compile
# can break in different ways than the main UI bundle (different deps
# active, different cfg gates), so build it explicitly.
build_one lunica_worker lunco-modelica

echo
echo "── wasm build gate passed ──"

if [ "$QUICK" = "1" ]; then exit 0; fi

# Report sizes for context (not a gate — purely informational).
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
for bin in lunica sandbox lunica_worker; do
    wasm="$target_dir/wasm32-unknown-unknown/web-dev/$bin.wasm"
    if [ -f "$wasm" ]; then
        size=$(du -h "$wasm" | cut -f1)
        echo "  $bin.wasm: $size"
    fi
done
