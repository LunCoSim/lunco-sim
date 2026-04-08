#!/bin/bash
# ============================================================================
# LunCoSim Modelica Workbench - Web Build Script
# ============================================================================
# Builds the Modelica Workbench for WebAssembly and serves it locally
# Usage: ./scripts/build_web.sh [serve|build|help]
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
WEB_DIR="$PROJECT_DIR/crates/lunco-modelica/web"
PKG_DIR="$WEB_DIR/pkg"
TARGET_DIR="$PROJECT_DIR/target/wasm32-unknown-unknown/release"

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
    info "Building Modelica Workbench for WebAssembly..."
    info "Target: wasm32-unknown-unknown"
    info "Profile: release"
    
    cargo build --release --target wasm32-unknown-unknown --bin modelica_workbench_web
    
    if [ $? -eq 0 ]; then
        success "WASM binary built successfully"
    else
        error "Build failed"
        exit 1
    fi
}

# Generate JavaScript bindings
generate_bindings() {
    info "Generating JavaScript bindings..."
    
    # Create pkg directory if it doesn't exist
    mkdir -p "$PKG_DIR"
    
    wasm-bindgen "$TARGET_DIR/modelica_workbench_web.wasm" \
        --out-dir "$PKG_DIR" \
        --target web
    
    if [ $? -eq 0 ]; then
        success "JavaScript bindings generated"
    else
        error "Binding generation failed"
        exit 1
    fi
    
    # Show output size
    WASM_SIZE=$(du -h "$PKG_DIR/modelica_workbench_web_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$PKG_DIR/modelica_workbench_web.js" | cut -f1)
    info "Output sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
}

# Serve the web application
serve_web() {
    info "Starting web server..."
    info "Serving from: $WEB_DIR"
    
    cd "$WEB_DIR"
    
    if [ "$HTTP_SERVER_CMD" = "http-server" ]; then
        info "Using http-server (Node.js)"
        info "URL: http://localhost:8080"
        http-server -p 8080 -c-1 --cors
    else
        info "Using Python3 HTTP server"
        info "URL: http://localhost:8080"
        python3 -m http.server 8080
    fi
}

# Clean build artifacts
clean() {
    info "Cleaning build artifacts..."
    rm -rf "$PKG_DIR"
    rm -f "$TARGET_DIR/modelica_workbench_web.wasm"
    rm -f "$TARGET_DIR/modelica_workbench_web.d"
    success "Cleaned"
}

# Show help
show_help() {
    echo "LunCoSim Modelica Workbench - Web Build Script"
    echo ""
    echo "Usage: $0 [COMMAND]"
    echo ""
    echo "Commands:"
    echo "  build       Build WASM and generate bindings (default)"
    echo "  serve       Start web server (requires built files)"
    echo "  all         Build and serve"
    echo "  clean       Remove build artifacts"
    echo "  help        Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 build          # Build only"
    echo "  $0 serve          # Serve only"
    echo "  $0 all            # Build and serve"
    echo "  $0 clean          # Clean artifacts"
    echo ""
    echo "Prerequisites:"
    echo "  - Rust with wasm32-unknown-unknown target"
    echo "  - wasm-bindgen CLI (cargo install wasm-bindgen-cli)"
    echo "  - http-server (npm install -g http-server) OR python3"
}

# Main execution
main() {
    local command="${1:-build}"
    
    case "$command" in
        build)
            check_prerequisites
            build_wasm
            generate_bindings
            success "Build complete! Run '$0 serve' to start the server"
            ;;
        serve)
            check_prerequisites
            serve_web
            ;;
        all)
            check_prerequisites
            build_wasm
            generate_bindings
            serve_web
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
