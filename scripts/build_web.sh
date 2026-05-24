#!/bin/bash
# ============================================================================
# LunCoSim - Web Build Script
# ============================================================================
# Builds LunCoSim applications for WebAssembly and serves them locally
# Usage: ./scripts/build_web.sh [command] [binary]
#
# Commands:
#   build <binary>    Build WASM and generate bindings
#   serve <binary>    Start web server (requires built files)
#   all <binary>      Build and serve
#   clean             Remove build artifacts
#   help              Show this help message
#
# Available binaries:
#   lunica_web  - Modelica Workbench IDE
#   sandbox_web       - Simulation Sandbox
# ============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Print colored message
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Get binary config
get_binary_config() {
    local binary="$1"
    case "$binary" in
        lunica_web)
            echo "lunco-modelica"
            ;;
        sandbox_web)
            echo "lunco-client"
            ;;
        *)
            error "Unknown binary: $binary"
            error "Available binaries: lunica_web, sandbox_web"
            exit 1
            ;;
    esac
}

# Check prerequisites
check_prerequisites() {
    info "Checking prerequisites..."
    
    # Check Rust
    if ! command -v rustc &> /dev/null; then
        error "Rust is not installed. Install from https://rustup.rs/"
        exit 1
    fi
    
    # Check wasm32 target
    if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
        warn "wasm32-unknown-unknown target not found. Installing..."
        rustup target add wasm32-unknown-unknown
        success "wasm32-unknown-unknown target installed"
    else
        success "wasm32-unknown-unknown target is installed"
    fi
    
    # Check wasm-bindgen CLI
    if ! command -v wasm-bindgen &> /dev/null; then
        warn "wasm-bindgen CLI not found. Installing..."
        cargo install wasm-bindgen-cli
        success "wasm-bindgen CLI installed"
    else
        success "wasm-bindgen CLI is installed"
    fi
    
    # Check HTTP server options
    if command -v http-server &> /dev/null; then
        HTTP_SERVER_CMD="http-server"
        success "http-server (Node.js) found"
    elif command -v python3 &> /dev/null; then
        HTTP_SERVER_CMD="python3"
        success "Python3 found"
    else
        warn "No HTTP server found. Install http-server: npm install -g http-server"
        HTTP_SERVER_CMD="python3"
    fi
}

# Should we rebuild the off-thread worker bundle?
#
# The worker bin pulls all of `lunco-modelica` (lib + UI), so most
# inner-loop UI edits invalidate it. But you DO want to skip it when
# only HTML/JS/asset/build-script files changed, or — crucially —
# when nothing under `crates/` changed since the last successful
# worker build (re-running `build_web.sh` on a clean tree shouldn't
# re-link 54 MB of wasm twice).
#
# Heuristic: rebuild iff any `.rs` or `Cargo.toml` under any local
# crate is newer than the existing `lunica_worker.wasm` cargo output.
# `find -newer` is cheap (one stat per file). False positive on a
# changed third-party crate version is fine — that's a real rebuild.
# False negative on a manual `cargo clean` is mitigated by the
# `[ ! -f "$worker_wasm" ]` short-circuit below.
#
# Set `WORKER_REBUILD=force` to override (e.g. switching profiles).
should_rebuild_worker() {
    local worker_wasm="$1"
    if [ "${WORKER_REBUILD:-}" = "force" ]; then
        return 0
    fi
    if [ ! -f "$worker_wasm" ]; then
        return 0
    fi
    # First file newer than the worker wasm = needs rebuild.
    local newer
    newer=$(find "$PROJECT_DIR/crates" \
        \( -name '*.rs' -o -name 'Cargo.toml' \) \
        -newer "$worker_wasm" -print -quit 2>/dev/null)
    [ -n "$newer" ]
}

# Wrap cargo with sccache when it's installed.
#
# sccache caches per-rustc-invocation across worktrees / branches /
# `cargo clean` cycles — biggest win on the cold rebuild that follows
# a dependency-version bump. Disables Cargo incremental (sccache and
# incremental fight; sccache is the better trade-off for our flow).
maybe_sccache_env() {
    if command -v sccache &> /dev/null; then
        export RUSTC_WRAPPER="${RUSTC_WRAPPER:-sccache}"
        export CARGO_INCREMENTAL=0
        info "sccache: enabled (RUSTC_WRAPPER=$RUSTC_WRAPPER, CARGO_INCREMENTAL=0)"
    else
        info "sccache: not installed — install with \`cargo install sccache\` for cross-worktree caching"
    fi
}

# Build the WASM binary
build_wasm() {
    local binary="$1"
    local crate="$2"

    # `BUILD_PROFILE` is exported by `main` once the CLI args are
    # parsed. Defaults to web-release; `--dev` flips it to web-dev for
    # a fast inner-loop build (no fat LTO, no shrink-first codegen,
    # parallel codegen units, incremental). The `wasm-opt` post-pass
    # is skipped in dev mode too — see `generate_bindings`.
    local profile="${BUILD_PROFILE:-web-release}"
    info "Building $binary for WebAssembly..."
    info "Crate: $crate"
    info "Target: wasm32-unknown-unknown"
    info "Profile: $profile"

    maybe_sccache_env

    # We use --no-default-features to avoid pulling in the full tokio/axum stack
    # from lunco-api, which depends on mio and other networking primitives
    # that are unsupported on wasm32-unknown-unknown.
    #
    # `--cfg=web_sys_unstable_apis` is REQUIRED for wgpu's WebGPU backend on
    # wasm (web-sys's `Gpu*` bindings are gated behind that flag). Without it
    # `navigator.gpu.requestAdapter()` silently returns null and bevy_render
    # panics with "Unable to find a GPU!" — even when the browser fully
    # supports WebGPU. egui's pipeline requires WebGPU here, so this flag is
    # mandatory, not optional.
    RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis" \
        cargo build --profile "$profile" --target wasm32-unknown-unknown --bin "$binary" -p "$crate" --no-default-features

    # For lunica_web, also build the off-thread worker bundle. It runs in a
    # Web Worker so rumoca's compile (~20s) doesn't block the UI.
    if [ "$binary" = "lunica_web" ]; then
        local base_target_dir
        base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
        local worker_wasm="$base_target_dir/wasm32-unknown-unknown/$profile/lunica_worker.wasm"
        if should_rebuild_worker "$worker_wasm"; then
            info "Building companion worker bundle: lunica_worker"
            RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis" \
                cargo build --profile "$profile" --target wasm32-unknown-unknown --bin lunica_worker -p "$crate" --no-default-features
        else
            # Stamp the existing wasm so the next freshness check
            # references "now" — cheap dependency change in a sibling
            # script (build_web.sh edit, copying assets) won't
            # otherwise re-trigger this reasoning every run.
            info "Worker bundle up-to-date; skipping cargo build (set WORKER_REBUILD=force to override)"
        fi
    fi

    if [ $? -eq 0 ]; then
        success "WASM binary built successfully"
    else
        error "Build failed"
        exit 1
    fi
}

# Generate JavaScript bindings and assemble the shippable bundle.
#
# Layout (matches Rust/wasm conventions):
#   target/wasm32-unknown-unknown/release/<bin>.wasm   — cargo output
#   target/web/<bin>/                                  — wasm-bindgen output (intermediate)
#   dist/<bin>/                                        — final bundle served to browsers
#   crates/<crate>/web/index.html                      — source HTML template
generate_bindings() {
    local binary="$1"
    local crate="$2"
    local index_html="$PROJECT_DIR/crates/$crate/web/index.html"

    # Dynamically find the target directory in case it's overridden in .cargo/config.toml
    local base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
    local profile="${BUILD_PROFILE:-web-release}"
    local cargo_out_dir="$base_target_dir/wasm32-unknown-unknown/$profile"
    local bindgen_out_dir="$base_target_dir/web/$binary"
    local dist_dir="$PROJECT_DIR/dist/$binary"

    info "Generating JavaScript bindings..."
    info "wasm-bindgen out: $bindgen_out_dir"
    info "Bundle dir:       $dist_dir"

    mkdir -p "$bindgen_out_dir" "$dist_dir"

    # Prefer local wasm-bindgen if it exists
    local wasm_bindgen_cmd="wasm-bindgen"
    if [ -f "$PROJECT_DIR/.cargo-bin/bin/wasm-bindgen" ]; then
        wasm_bindgen_cmd="$PROJECT_DIR/.cargo-bin/bin/wasm-bindgen"
        info "Using local wasm-bindgen: $wasm_bindgen_cmd"
    fi

    $wasm_bindgen_cmd "$cargo_out_dir/${binary}.wasm" \
        --out-dir "$bindgen_out_dir" \
        --target web

    if [ $? -ne 0 ]; then
        error "Binding generation failed"
        exit 1
    fi
    success "JavaScript bindings generated"

    # Best-effort wasm-opt pass. Typical 15–30 % size win on a release
    # wasm, which directly cuts download + streaming-compile time on
    # the page. Skipped (with a hint) if wasm-opt isn't on PATH so the
    # build still succeeds on machines that haven't installed
    # `binaryen`.
    local wasm_in="$bindgen_out_dir/${binary}_bg.wasm"
    # Skip the size pass entirely in dev mode — wasm-opt on a 28 MB
    # debug-ish wasm is ~20–30 s, which dominates inner-loop cycle
    # time. Bigger payload is fine for `localhost`.
    if [ "${BUILD_PROFILE:-web-release}" = "web-dev" ]; then
        info "wasm-opt skipped (--dev profile)"
    elif [ -f "$wasm_in" ] && command -v wasm-opt &> /dev/null; then
        info "Running wasm-opt -Oz --converge (max-size pass)…"
        local before
        before=$(stat -c '%s' "$wasm_in" 2>/dev/null || stat -f '%z' "$wasm_in")
        # -Oz = shrink-first (typically 10–25 % smaller than -O2).
        # --converge re-runs passes until no further size win — adds a
        # minute or two to release builds but is one-shot at deploy.
        local tmp="$wasm_in.opt.tmp"
        if wasm-opt -Oz --converge --strip-debug -o "$tmp" "$wasm_in"; then
            mv "$tmp" "$wasm_in"
            local after
            after=$(stat -c '%s' "$wasm_in" 2>/dev/null || stat -f '%z' "$wasm_in")
            info "wasm-opt: $(awk "BEGIN{printf \"%.1f\", $before/1048576}") MB → $(awk "BEGIN{printf \"%.1f\", $after/1048576}") MB"
        else
            warn "wasm-opt failed; keeping original output"
            rm -f "$tmp"
        fi
    elif [ -f "$wasm_in" ]; then
        info "wasm-opt not installed — skipping size pass (install \`binaryen\` for ~20% smaller wasm)"
    fi

    # Assemble the bundle: bindings + index.html in one place.
    # Use a fresh dist dir so stale files from a previous binary version
    # don't get served accidentally.
    #
    # Stash the worker subdir before the wipe so the worker-bindgen
    # skip path can restore it without re-running bindgen / wasm-opt.
    # Without this, the wipe deletes a still-up-to-date worker bundle,
    # the freshness check below sees a missing dist file, and the
    # skip can never fire.
    local stashed_worker=""
    if [ -d "$dist_dir/worker" ]; then
        stashed_worker=$(mktemp -d)
        cp -r "$dist_dir/worker"/. "$stashed_worker/"
    fi
    rm -rf "$dist_dir"
    mkdir -p "$dist_dir"
    cp "$bindgen_out_dir"/* "$dist_dir/"
    if [ -n "$stashed_worker" ]; then
        mkdir -p "$dist_dir/worker"
        cp -r "$stashed_worker"/. "$dist_dir/worker/"
        rm -rf "$stashed_worker"
    fi
    if [ -f "$index_html" ]; then
        cp "$index_html" "$dist_dir/index.html"
        # Inject the actual uncompressed WASM size so the loading UI
        # can show accurate progress even when nginx serves a
        # pre-compressed .gz sibling (gzip_static on).
        local wasm_dist="$dist_dir/${binary}_bg.wasm"
        if [ -f "$wasm_dist" ]; then
            local wasm_bytes
            wasm_bytes=$(stat -c '%s' "$wasm_dist" 2>/dev/null || stat -f '%z' "$wasm_dist")
            sed -i "s/const __LC_WASM_SIZE__ = 0/const __LC_WASM_SIZE__ = $wasm_bytes/" "$dist_dir/index.html"
            info "Injected WASM size into index.html: $(awk "BEGIN{printf \"%.1f\", $wasm_bytes/1048576}") MB"
        fi
    else
        warn "No index.html found at $index_html — bundle will lack an entry point"
    fi

    # DejaVu Sans — wasm has no filesystem, lunco-theme fetches this
    # over HTTP at startup (see crates/lunco-theme/src/fonts.rs::
    # spawn_wasm_font_fetch). Source lives in the workspace cache
    # (populated by `cargo run -p lunco-assets -- download`); we just
    # copy it next to the wasm so it's served same-origin.
    local dejavu_src=""
    for candidate in \
        "$PROJECT_DIR/../.cache/fonts/DejaVuSans.ttf" \
        "$PROJECT_DIR/.cache/fonts/DejaVuSans.ttf"; do
        if [ -f "$candidate" ]; then dejavu_src="$candidate"; break; fi
    done
    if [ -n "$dejavu_src" ]; then
        mkdir -p "$dist_dir/fonts"
        cp "$dejavu_src" "$dist_dir/fonts/DejaVuSans.ttf"
        info "Copied DejaVu Sans → $dist_dir/fonts/"
    else
        warn "DejaVu Sans not found — math/arrow glyphs will tofu in browser. \
Run: cargo run -p lunco-assets -- download"
    fi

    # Show output size
    WASM_SIZE=$(du -h "$dist_dir/${binary}_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$dist_dir/${binary}.js" | cut -f1)
    info "Bundle sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
    info "Bundle ready: $dist_dir"

    # ── Worker bundle (lunica_web only) ─────────────────────────────────
    # Generate bindings for the off-thread Modelica worker and place its
    # output under `dist/lunica_web/worker/` so the main page can
    # `new Worker('./worker/lunica_worker.js', { type: 'module' })`. The
    # worker bundle is a SECOND wasm instance — it has its own memory and
    # state — and there is no way to share Rust globals or `Arc`s with it.
    if [ "$binary" = "lunica_web" ]; then
        local worker_bin="lunica_worker"
        local worker_bindgen_dir="$base_target_dir/web/$worker_bin"
        local worker_dist_dir="$dist_dir/worker"
        local worker_wasm_src="$cargo_out_dir/${worker_bin}.wasm"
        local worker_wasm_dist="$worker_dist_dir/${worker_bin}_bg.wasm"
        # Skip the bindgen + wasm-opt + copy work entirely if the
        # cargo output didn't move since the last dist build. Pairs
        # with the `should_rebuild_worker` cargo-build skip in
        # `build_wasm`. Set `WORKER_REBUILD=force` to override.
        if [ "${WORKER_REBUILD:-}" != "force" ] \
            && [ -f "$worker_wasm_src" ] \
            && [ -f "$worker_wasm_dist" ] \
            && [ ! "$worker_wasm_src" -nt "$worker_wasm_dist" ]; then
            local worker_size
            worker_size=$(du -h "$worker_wasm_dist" | cut -f1)
            info "Worker bundle up-to-date ($worker_size) — bindgen skipped"
            return 0
        fi
        info "Generating bindings for worker bundle: $worker_bin"
        mkdir -p "$worker_bindgen_dir" "$worker_dist_dir"
        $wasm_bindgen_cmd "$worker_wasm_src" \
            --out-dir "$worker_bindgen_dir" \
            --target web
        if [ $? -ne 0 ]; then
            error "Worker binding generation failed"
            exit 1
        fi
        # wasm-opt the worker too, same flags as the main bundle.
        local worker_wasm_in="$worker_bindgen_dir/${worker_bin}_bg.wasm"
        if [ "${BUILD_PROFILE:-web-release}" = "web-dev" ]; then
            info "Worker wasm-opt skipped (--dev profile)"
        elif [ -f "$worker_wasm_in" ] && command -v wasm-opt &> /dev/null; then
            local tmp="$worker_wasm_in.opt.tmp"
            if wasm-opt -Oz --converge --strip-debug -o "$tmp" "$worker_wasm_in"; then
                mv "$tmp" "$worker_wasm_in"
            else
                rm -f "$tmp"
            fi
        fi
        rm -rf "$worker_dist_dir"
        mkdir -p "$worker_dist_dir"
        cp "$worker_bindgen_dir"/* "$worker_dist_dir/"
        # Web Worker entry shim. wasm-bindgen --target web exports `init`
        # but doesn't run it; this tiny module imports + calls it so the
        # `#[wasm_bindgen(start)]` worker entry actually fires.
        local worker_bootstrap="$PROJECT_DIR/crates/$crate/web/worker_bootstrap.js"
        if [ -f "$worker_bootstrap" ]; then
            cp "$worker_bootstrap" "$worker_dist_dir/worker_bootstrap.js"
        else
            warn "No worker_bootstrap.js at $worker_bootstrap — worker won't init"
        fi
        local worker_size
        worker_size=$(du -h "$worker_dist_dir/${worker_bin}_bg.wasm" | cut -f1)
        info "Worker bundle: $worker_size at $worker_dist_dir"
    fi
}

# Pack MSL into a versioned, compressed bundle and place it next to the
# wasm under `dist/<bin>/msl/`. Same-origin so the runtime fetcher doesn't
# need CORS configuration. Skipped for binaries that don't ship MSL
# (sandbox_web).
build_msl_bundle() {
    local binary="$1"
    if [ "$binary" != "lunica_web" ]; then
        return 0
    fi
    local dist_dir="$PROJECT_DIR/dist/$binary"
    local msl_dir="$dist_dir/msl"

    # Skip the rumoca pre-parse + tar+zstd pass when nothing under
    # `.cache/msl/` is newer than the existing `manifest.json`. Pack
    # is content-addressed (`parsed-<sha>.bin.zst`), so a no-op rerun
    # produces byte-identical output anyway — the only thing the
    # script saves is ~2 s of parse + compress work.
    #
    # Override with `MSL_REBUILD=force` if you've changed the
    # bundler binary itself (`build_msl_assets`) or its serialisation
    # format and want a guaranteed re-pack.
    if [ "${MSL_REBUILD:-}" != "force" ] && [ -f "$msl_dir/manifest.json" ]; then
        local msl_src
        for candidate in \
            "$PROJECT_DIR/../.cache/msl" \
            "$PROJECT_DIR/.cache/msl"; do
            if [ -d "$candidate" ]; then msl_src="$candidate"; break; fi
        done
        if [ -n "$msl_src" ]; then
            local newer
            newer=$(find "$msl_src" -name '*.mo' -newer "$msl_dir/manifest.json" -print -quit 2>/dev/null)
            if [ -z "$newer" ]; then
                info "MSL bundle up-to-date ($msl_src) — skipping pack (set MSL_REBUILD=force to override)"
                return 0
            fi
        fi
    fi

    info "Packing MSL bundle for $binary..."

    # The bundler walks `lunco_assets::msl_source_root_path()` on the host,
    # which lives at <workspace>/.cache/msl/ in this repo. If MSL isn't
    # materialised, the binary will exit non-zero with a clear message and
    # we surface that as a build error so we never ship without MSL.
    rm -rf "$msl_dir"
    mkdir -p "$msl_dir"
    cargo run --release -q -p lunco-assets --bin build_msl_assets -- \
        --out "$msl_dir"

    if [ $? -ne 0 ]; then
        error "MSL bundling failed"
        exit 1
    fi
    success "MSL bundle written to $msl_dir"
}

# Serve the web application from its dist bundle.
serve_web() {
    local binary="$1"
    local crate="$2"
    local dist_dir="$PROJECT_DIR/dist/$binary"
    local port="${3:-8080}"

    if [ ! -f "$dist_dir/index.html" ]; then
        error "No bundle at $dist_dir — run '$0 build $binary' first"
        exit 1
    fi

    info "Starting web server for $binary..."
    info "Serving from: $dist_dir"
    info "URL: http://localhost:$port"

    cd "$dist_dir"

    if [ "$HTTP_SERVER_CMD" = "http-server" ]; then
        info "Using http-server (Node.js)"
        http-server -p "$port" -c-1 --cors
    else
        info "Using Python3 HTTP server"
        python3 -m http.server "$port"
    fi
}

# Clean build artifacts.
# We don't touch target/ globally — that's cargo's job (`cargo clean`). We
# only remove the web-specific intermediates and the dist bundle.
clean() {
    info "Cleaning web build artifacts..."
    local base_target_dir
    base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
    rm -rf "$base_target_dir/web"
    rm -rf "$PROJECT_DIR/dist"
    rm -f "$base_target_dir/wasm32-unknown-unknown/web-release/"*_web.wasm
    rm -f "$base_target_dir/wasm32-unknown-unknown/web-release/"*_web.d
    success "Cleaned"
}

# Show help
show_help() {
    echo "LunCoSim - Web Build Script"
    echo ""
    echo "Usage: $0 [COMMAND] [BINARY] [PORT]"
    echo ""
    echo "Commands:"
    echo "  build <binary>    Build WASM and generate bindings"
    echo "  serve <binary>    Start web server (requires built files)"
    echo "  all <binary>      Build and serve"
    echo "  clean             Remove build artifacts"
    echo "  help              Show this help message"
    echo ""
    echo "Available binaries:"
    echo "  lunica_web  - Modelica Workbench IDE (default port: 8080)"
    echo "  sandbox_web       - Rover Physics Sandbox (default port: 8081)"
    echo ""
    echo "Examples:"
    echo "  $0 build lunica_web    # Build Modelica Workbench"
    echo "  $0 serve sandbox_web         # Serve Sandbox"
    echo "  $0 all lunica_web      # Build and serve Modelica Workbench"
    echo "  $0 all sandbox_web 8082      # Build and serve on custom port"
    echo "  $0 clean                           # Clean all artifacts"
    echo ""
    echo "Prerequisites:"
    echo "  - Rust with wasm32-unknown-unknown target"
    echo "  - wasm-bindgen CLI (cargo install wasm-bindgen-cli)"
    echo "  - http-server (npm install -g http-server) OR python3"
}

# Main execution
main() {
    local command="${1:-help}"
    local binary="${2:-}"
    local port="${3:-}"

    # Pull `--dev` out of the positional args so it can appear in any
    # slot (`build lunica_web --dev`, `--dev build lunica_web`, …).
    # Sets `BUILD_PROFILE=web-dev` for `build_wasm` + `generate_bindings`,
    # which selects the fast-iteration cargo profile and skips the
    # `wasm-opt` size pass.
    export BUILD_PROFILE="web-release"
    local positional=()
    for arg in "$@"; do
        case "$arg" in
            --dev)
                export BUILD_PROFILE="web-dev"
                ;;
            *)
                positional+=("$arg")
                ;;
        esac
    done
    command="${positional[0]:-help}"
    binary="${positional[1]:-}"
    port="${positional[2]:-}"

    case "$command" in
        build)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            build_wasm "$binary" "$crate"
            generate_bindings "$binary" "$crate"
            build_msl_bundle "$binary"
            success "Build complete! Run '$0 serve $binary' to start the server"
            ;;
        serve)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            local default_port=8080
            if [ "$binary" = "sandbox_web" ]; then
                default_port=8081
            fi
            serve_web "$binary" "$crate" "${port:-$default_port}"
            ;;
        all)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            local default_port=8080
            if [ "$binary" = "sandbox_web" ]; then
                default_port=8081
            fi
            build_wasm "$binary" "$crate"
            generate_bindings "$binary" "$crate"
            build_msl_bundle "$binary"
            serve_web "$binary" "$crate" "${port:-$default_port}"
            ;;
        clean)
            clean
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            error "Unknown command: $command"
            show_help
            exit 1
            ;;
    esac
}

main "$@"
