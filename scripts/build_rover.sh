#!/bin/bash
# ============================================================================
# LunCoSim - Rover Sandbox Build Script
# ============================================================================
# Builds the rover_sandbox binary (desktop or web)
# Usage: ./scripts/build_rover.sh [desktop|web|all]
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

# Build desktop rover_sandbox
build_desktop() {
    local profile="${1:-release}"
    info "Building rover_sandbox for desktop ($profile)..."
    
    cargo build --$profile -p lunco-client --bin rover_sandbox
    
    if [ $? -eq 0 ]; then
        local bin_path="$PROJECT_DIR/target/$profile/rover_sandbox"
        success "Desktop build complete: $bin_path"
        success "Run: $bin_path"
    else
        error "Desktop build failed"
        exit 1
    fi
}

# Build web rover_sandbox_web
build_web() {
    info "Building rover_sandbox_web for WebAssembly..."
    
    # Check prerequisites
    if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
        warn "wasm32-unknown-unknown target not found. Installing..."
        rustup target add wasm32-unknown-unknown
    fi
    
    if ! command -v wasm-bindgen &> /dev/null; then
        warn "wasm-bindgen CLI not found. Installing..."
        cargo install wasm-bindgen-cli
    fi
    
    # Build WASM
    info "Compiling to WebAssembly..."
    cargo build --release --target wasm32-unknown-unknown --bin rover_sandbox_web -p lunco-client
    
    # Generate bindings
    info "Generating JavaScript bindings..."
    local web_dir="$PROJECT_DIR/crates/lunco-client/web"
    local pkg_dir="$web_dir/pkg"
    mkdir -p "$pkg_dir"
    
    wasm-bindgen "$PROJECT_DIR/target/wasm32-unknown-unknown/release/rover_sandbox_web.wasm" \
        --out-dir "$pkg_dir" \
        --target web
    
    # Copy root assets to web directory (shaders, models, etc.)
    info "Copying assets for web serving..."
    if [ -d "$PROJECT_DIR/assets" ]; then
        rsync -a --delete "$PROJECT_DIR/assets/" "$web_dir/assets/" 2>/dev/null || cp -r "$PROJECT_DIR/assets/"* "$web_dir/assets/"
    fi
    
    # Show sizes
    WASM_SIZE=$(du -h "$pkg_dir/rover_sandbox_web_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$pkg_dir/rover_sandbox_web.js" | cut -f1)
    
    success "Web build complete!"
    success "Output sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
    success "Serve with: cd $web_dir && python3 -m http.server 8081"
    success "Then open: http://localhost:8081"
}

# Serve web build
serve_web() {
    local port="${1:-8081}"
    local web_dir="$PROJECT_DIR/crates/lunco-client/web"
    
    info "Starting web server for rover_sandbox_web..."
    info "Serving from: $web_dir"
    info "URL: http://localhost:$port"
    
    cd "$web_dir"
    
    if command -v http-server &> /dev/null; then
        http-server -p "$port" -c-1 --cors
    else
        python3 -m http.server "$port"
    fi
}

# Clean build artifacts
clean() {
    info "Cleaning rover sandbox build artifacts..."
    rm -f "$PROJECT_DIR/target/release/rover_sandbox"
    rm -f "$PROJECT_DIR/target/debug/rover_sandbox"
    rm -f "$PROJECT_DIR/target/wasm32-unknown-unknown/release/rover_sandbox_web.wasm"
    rm -rf "$PROJECT_DIR/crates/lunco-client/web/pkg"
    success "Cleaned"
}

# Show help
show_help() {
    echo "LunCoSim - Rover Sandbox Build Script"
    echo ""
    echo "Usage: $0 [COMMAND] [OPTIONS]"
    echo ""
    echo "Commands:"
    echo "  desktop [debug|release]   Build for desktop (default: release)"
    echo "  web                       Build for WebAssembly"
    echo "  serve [PORT]              Start web server (default: 8081)"
    echo "  all                       Build desktop (release) + web"
    echo "  clean                     Remove build artifacts"
    echo "  help                      Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 desktop           # Build desktop release build"
    echo "  $0 desktop debug     # Build desktop debug build"
    echo "  $0 web               # Build WASM + generate bindings"
    echo "  $0 serve             # Serve web build on port 8081"
    echo "  $0 serve 8082        # Serve web build on custom port"
    echo "  $0 all               # Build both desktop and web"
    echo ""
    echo "Running the desktop build:"
    echo "  cargo run --release -p lunco-client --bin rover_sandbox"
    echo "  ./target/release/rover_sandbox"
}

# Main execution
main() {
    local command="${1:-help}"
    local arg="${2:-}"
    
    case "$command" in
        desktop)
            build_desktop "${arg:-release}"
            ;;
        web)
            build_web
            ;;
        serve)
            serve_web "${arg:-8081}"
            ;;
        all)
            info "Building all variants..."
            build_desktop "release"
            echo ""
            build_web
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
