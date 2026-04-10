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
    
    cargo build --release --target wasm32-unknown-unknown --bin "$binary" -p "$crate"
    
    if [ $? -eq 0 ]; then
        success "WASM binary built successfully"
    else
        error "Build failed"
        exit 1
    fi
}

# Generate JavaScript bindings
generate_bindings() {
    local binary="$1"
    local crate="$2"
    local web_dir="$PROJECT_DIR/crates/$crate/web"
    local pkg_dir="$web_dir/pkg"
    local target_dir="$PROJECT_DIR/target/wasm32-unknown-unknown/release"
    
    info "Generating JavaScript bindings..."
    
    # Create pkg directory if it doesn't exist
    mkdir -p "$pkg_dir"
    
    wasm-bindgen "$target_dir/${binary}.wasm" \
        --out-dir "$pkg_dir" \
        --target web
    
    if [ $? -eq 0 ]; then
        success "JavaScript bindings generated"
    else
        error "Binding generation failed"
        exit 1
    fi
    
    # Show output size
    WASM_SIZE=$(du -h "$pkg_dir/${binary}_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$pkg_dir/${binary}.js" | cut -f1)
    info "Output sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
}

# Serve the web application
serve_web() {
    local binary="$1"
    local crate="$2"
    local web_dir="$PROJECT_DIR/crates/$crate/web"
    local port="${3:-8080}"
    
    info "Starting web server for $binary..."
    info "Serving from: $web_dir"
    info "URL: http://localhost:$port"
    
    cd "$web_dir"
    
    if [ "$HTTP_SERVER_CMD" = "http-server" ]; then
        info "Using http-server (Node.js)"
        http-server -p "$port" -c-1 --cors
    else
        info "Using Python3 HTTP server"
        python3 -m http.server "$port"
    fi
}

# Clean build artifacts
clean() {
    info "Cleaning build artifacts..."
    rm -rf "$PROJECT_DIR/crates/lunco-modelica/web/pkg"
    rm -rf "$PROJECT_DIR/crates/lunco-client/web/pkg"
    rm -f "$PROJECT_DIR/target/wasm32-unknown-unknown/release/"*_web.wasm
    rm -f "$PROJECT_DIR/target/wasm32-unknown-unknown/release/"*_web.d
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
