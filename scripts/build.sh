#!/usr/bin/env bash
# ============================================================================
# LunCoSim — unified build dispatcher
# ============================================================================
# One entry point for every shippable target, web or native.
#
# Usage:
#     ./scripts/build.sh <target> [--release] [extra cargo/build_web args]
#
# Targets:
#     luncosim          WASM client bundle  -> dist/luncosim/   (browser)
#     lunica           WASM client bundle  -> dist/lunica/    (browser IDE)
#     luncosim-server   NATIVE headless server binary -> target/<profile>/luncosim
#                      (run with `--no-ui --host`; see crates/lunco-networking/DEPLOY.md)
#     lunica-desktop   NATIVE desktop GUI  -> dist/lunica-<platform>-<arch>/
#     luncosim-desktop  NATIVE desktop GUI  -> dist/luncosim-<platform>-<arch>/
#                      (Linux, macOS, Windows; see scripts/build_native.sh)
#
# Profile:
#     default          fast dev build
#     --release        optimized build (web: fat LTO + wasm-opt; native: release)
#
# Examples:
#     ./scripts/build.sh luncosim --release          # optimized web client
#     ./scripts/build.sh luncosim-server --release   # optimized native server
#     ./scripts/build.sh luncosim-server             # quick dev server build
#     ./scripts/build.sh lunica-desktop --release --package  # native desktop
#     ./scripts/build.sh luncosim-desktop --release --package
#
# The web targets delegate to build_web.sh (the wasm pipeline); the server
# target is a plain native cargo build with the headless feature set; the
# desktop targets delegate to build_native.sh (native GUI + asset packaging).
# ============================================================================
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
    sed -n '2,33p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

TARGET="${1:-}"
[ -z "$TARGET" ] && usage 2
shift

# Pull --release out of the remaining args; pass the rest through.
RELEASE=0
PASS=()
for a in "$@"; do
    case "$a" in
        --release) RELEASE=1 ;;
        -h|--help) usage 0 ;;
        *) PASS+=("$a") ;;
    esac
done

case "$TARGET" in
    luncosim|lunica)
        # WASM client — hand off to the web pipeline (it owns wasm-bindgen,
        # asset staging, the MSL bundle, the worker, and the optional wasm-opt
        # size pass under --release).
        args=(build "$TARGET")
        [ "$RELEASE" -eq 1 ] && args+=(--release)
        args+=("${PASS[@]+"${PASS[@]}"}")
        info "web build: ./scripts/build_web.sh ${args[*]}"
        exec "$SCRIPT_DIR/build_web.sh" "${args[@]}"
        ;;

    luncosim-server)
        # NATIVE headless server: the `luncosim-server` bin from `lunco-luncosim-server`.
        # default features are dropped to omit the GUI stack, and the `server` feature
        # enables the HTTP API + networking host. No winit, no display.
        cd "$PROJECT_DIR"
        profile_args=()
        out="target/debug/luncosim-server"
        label="dev"
        rel_flag=""
        if [ "$RELEASE" -eq 1 ]; then
            profile_args=(--release)
            out="target/release/luncosim-server"
            label="release"
            rel_flag=" --release"
        fi
        info "native server build ($label)…"
        cargo build "${profile_args[@]}" \
            --bin luncosim-server -p lunco-luncosim-server \
            "${PASS[@]+"${PASS[@]}"}"
        success "server binary: $PROJECT_DIR/$out"

        # Assemble a self-contained dist/server/ — mirrors dist/<bin>/ for the
        # web targets, so the server deploy is a single-dir rsync.
        DIST="$PROJECT_DIR/dist/server"
        info "staging bundle → dist/server/"
        rm -rf "$DIST"; mkdir -p "$DIST/deploy"
        cp -f "$PROJECT_DIR/$out" "$DIST/luncosim"
        # Strip the SHIPPED copy. A dev binary carries ~3+ GB of `.debug_*`
        # (loadable code is only ~200 MB); the `[profile.release]` already sets
        # `strip = true`, so this is the dev-build equivalent. The original
        # target/ binary keeps its symbols for local debugging.
        if command -v strip >/dev/null 2>&1; then strip "$DIST/luncosim" 2>/dev/null || true; fi
        cp -a "$PROJECT_DIR/assets" "$DIST/assets"          # ~330 KB, trivial copy
        # `cp` cannot exclude, so prune afterwards: `.lunco/` (runtime overlay)
        # and `history/` (edit journal) are per-session state a dev run writes
        # into `assets/`, which is itself an open twin. Shipping them layers
        # someone's recorded session over the authored scene on every install.
        # The rule is `lunco_twin::is_runtime_state`.
        find "$DIST/assets" -type d \( -name '.lunco' -o -name 'history' \) -prune -exec rm -rf {} + 2>/dev/null || true
        cp -f "$SCRIPT_DIR/deploy/"* "$DIST/deploy/"
        cp -f "$PROJECT_DIR/crates/lunco-networking/DEPLOY.md" "$DIST/DEPLOY.md"
        success "server bundle: $DIST ($(du -sh "$DIST/luncosim" | cut -f1) binary + assets + deploy kit)"
        [ "$RELEASE" -eq 0 ] && warn "dev build (opt-level 1). For deploy use --release (faster + smaller)."
        info "run:    $out --host 5888 --api 4101   (5888/4101 are the defaults; numbers optional)"
        info "deploy: ./scripts/deploy_server.sh user@host --server <remote-path> [--web <remote-path>]"
        ;;

    -h|--help)
        usage 0 ;;
    lunica-desktop|luncosim-desktop)
        # NATIVE desktop GUI — hand off to the native build pipeline (it
        # owns platform detection, asset/cache bundling, launcher scripts,
        # and optional archive creation). Pass --release and any extra
        # args through.
        bin="${TARGET%-desktop}"
        args=("$bin")
        [ "$RELEASE" -eq 1 ] && args+=(--release)
        args+=("${PASS[@]+"${PASS[@]}"}")
        info "native desktop build: ./scripts/build_native.sh ${args[*]}"
        exec "$SCRIPT_DIR/build_native.sh" "${args[@]}"
        ;;
    *)
        error "Unknown target: $TARGET"
        error "Targets: luncosim, lunica (web)  |  luncosim-server (native server)  |  lunica-desktop, luncosim-desktop (native desktop)"
        exit 2 ;;
esac
