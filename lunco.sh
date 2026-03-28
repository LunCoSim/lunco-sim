#!/bin/bash

# LunCo Project Master Script
# Unified interface for setup, deployment, execution, and development

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_ROOT="$SCRIPT_DIR/scripts"

# Default Godot path
DEFAULT_GODOT="godot"

usage() {
    echo "Usage: $0 <command> <subcommand> [args]"
    echo ""
    echo "Commands:"
    echo "  setup    Install dependencies and configure environment"
    echo "    godot [version]        Install Godot engine binary"
    echo "    templates [version]    Install Godot export templates"
    echo "    addons [godot_path]    Install project-specific addons"
    echo "    wss [domain]           Setup WSS certificates"
    echo ""
    echo "  deploy   Deploy applications"
    echo "    3dsim [godot_path]     Deploy 3D Simulation"
    echo "    openmct                Deploy OpenMCT MCC"
    echo ""
    echo "  run      Run simulation and servers"
    echo "    server [godot_path]    Start headless simulation server"
    echo "    persistent             Start Godot in persistent background mode"
    echo ""
    echo "  test [godot_path]        Run Modelica tests"
    echo "  verify                   Verify telemetry and WSS setup"
    echo ""
    echo "Common arguments:"
    echo "  godot_path  Optional path to Godot binary (defaults to '$DEFAULT_GODOT')"
    echo ""
}

case "$1" in
    setup)
        case "$2" in
            godot)
                "$SCRIPTS_ROOT/setup/setup_godot_bin.sh" "${3:-4.7.dev3}"
                ;;
            templates)
                "$SCRIPTS_ROOT/setup/setup_godot_templates.sh" "${3:-4.7.dev3}"
                ;;
            addons)
                GODOT_BIN="${3:-$DEFAULT_GODOT}"
                "$SCRIPTS_ROOT/setup/setup_project_addons.sh" "$GODOT_BIN"
                ;;
            wss)
                "$SCRIPTS_ROOT/setup/setup_wss_ssl.sh" "$3"
                ;;
            *)
                usage
                exit 1
                ;;
        esac
        ;;
    deploy)
        case "$2" in
            3dsim)
                "$SCRIPTS_ROOT/deploy/deploy_luncosim_web.sh" "${3:-$DEFAULT_GODOT}"
                ;;
            openmct)
                "$SCRIPTS_ROOT/deploy/deploy_mcc_openmct.sh"
                ;;
            *)
                usage
                exit 1
                ;;
        esac
        ;;
    run)
        case "$2" in
            server)
                "$SCRIPTS_ROOT/run/start_luncosim_server.sh" "${3:-$DEFAULT_GODOT}"
                ;;
            persistent)
                "$SCRIPTS_ROOT/run/start_luncosim_daemon.sh" "${3:-$DEFAULT_GODOT}"
                ;;
            *)
                usage
                exit 1
                ;;
        esac
        ;;
    test)
        "$SCRIPTS_ROOT/dev/test_modelica_app.sh" "${2:-$DEFAULT_GODOT}"
        ;;
    verify)
        "$SCRIPTS_ROOT/dev/check_telemetry_api.sh"
        ;;
    --help|-h)
        usage
        ;;
    *)
        usage
        exit 1
        ;;
esac
