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
#   modelica_workbench_web  - Modelica Workbench IDE
#   rover_sandbox_web       - Rover Physics Sandbox
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
        modelica_workbench_web)
            echo "lunco-modelica"
            ;;
        rover_sandbox_web)
            echo "lunco-client"
            ;;
        *)
            error "Unknown binary: $binary"
            error "Available binaries: modelica_workbench_web, rover_sandbox_web"
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

# Build the WASM binary
build_wasm() {
    local binary="$1"
    local crate="$2"
    
    info "Building $binary for WebAssembly..."
    info "Crate: $crate"
    info "Target: wasm32-unknown-unknown"
    info "Profile: release"
    
    # We use --no-default-features to avoid pulling in the full tokio/axum stack
    # from lunco-api, which depends on mio and other networking primitives
    # that are unsupported on wasm32-unknown-unknown.
    cargo build --release --target wasm32-unknown-unknown --bin "$binary" -p "$crate" --no-default-features
    
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
    local cargo_out_dir="$base_target_dir/wasm32-unknown-unknown/release"
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
    if [ -f "$wasm_in" ] && command -v wasm-opt &> /dev/null; then
        info "Running wasm-opt -O2 (best-effort size + speed pass)…"
        local before
        before=$(stat -c '%s' "$wasm_in" 2>/dev/null || stat -f '%z' "$wasm_in")
        # -O2 keeps compile time reasonable while still doing useful
        # work; -Oz/-Os go further but are noticeably slower per build.
        local tmp="$wasm_in.opt.tmp"
        if wasm-opt -O2 --strip-debug -o "$tmp" "$wasm_in"; then
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
    rm -rf "$dist_dir"
    mkdir -p "$dist_dir"
    cp "$bindgen_out_dir"/* "$dist_dir/"
    if [ -f "$index_html" ]; then
        cp "$index_html" "$dist_dir/index.html"
    else
        warn "No index.html found at $index_html — bundle will lack an entry point"
    fi

    # Show output size
    WASM_SIZE=$(du -h "$dist_dir/${binary}_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$dist_dir/${binary}.js" | cut -f1)
    info "Bundle sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
    info "Bundle ready: $dist_dir"
}

# Pack MSL into a versioned, compressed bundle and place it next to the
# wasm under `dist/<bin>/msl/`. Same-origin so the runtime fetcher doesn't
# need CORS configuration. Skipped for binaries that don't ship MSL
# (rover_sandbox_web).
build_msl_bundle() {
    local binary="$1"
    if [ "$binary" != "modelica_workbench_web" ]; then
        return 0
    fi
    local dist_dir="$PROJECT_DIR/dist/$binary"
    local msl_dir="$dist_dir/msl"

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
    rm -f "$base_target_dir/wasm32-unknown-unknown/release/"*_web.wasm
    rm -f "$base_target_dir/wasm32-unknown-unknown/release/"*_web.d
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
    echo "  modelica_workbench_web  - Modelica Workbench IDE (default port: 8080)"
    echo "  rover_sandbox_web       - Rover Physics Sandbox (default port: 8081)"
    echo ""
    echo "Examples:"
    echo "  $0 build modelica_workbench_web    # Build Modelica Workbench"
    echo "  $0 serve rover_sandbox_web         # Serve Rover Sandbox"
    echo "  $0 all modelica_workbench_web      # Build and serve Modelica Workbench"
    echo "  $0 all rover_sandbox_web 8082      # Build and serve on custom port"
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
            if [ "$binary" = "rover_sandbox_web" ]; then
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
            if [ "$binary" = "rover_sandbox_web" ]; then
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
