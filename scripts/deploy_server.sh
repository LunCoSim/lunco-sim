#!/usr/bin/env bash
# ============================================================================
# LunCoSim — sandbox deploy (server binary + wasm client)
# ============================================================================
# Just rsyncs the artifacts to the paths YOU give. No default path, no service
# restart, no provisioning. Like the lunica web deploy: copy to a path, done.
#
# Usage:
#     ./scripts/deploy_server.sh <user@host> --server <path> [--web <path>] [opts]
#
# Paths (at least --server required, no defaults):
#     --server <path>  remote dir for the native binary + assets (dist/server/)
#     --web <path>     remote dir for the wasm client bundle      (dist/sandbox/)
#
# Options:
#     --ssh-port <n>   non-default SSH port
#     --dry-run        rsync -n; don't write anything
#
# Examples:
#     ./scripts/build.sh sandbox-server --release
#     ./scripts/build_web.sh build sandbox --release
#
#     # server only:
#     ./scripts/deploy_server.sh deploy@host --server ~/sandbox-server
#
#     # server + web in one go:
#     ./scripts/deploy_server.sh deploy@host \
#         --server ~/sandbox-server --web /var/www/html/sandbox.lunco.space
# ============================================================================
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() { sed -n '2,29p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

HOST="${1:-}"; [ -z "$HOST" ] && usage 2
case "$HOST" in -h|--help) usage 0 ;; esac
shift

SERVER_PATH=""; WEB_PATH=""; SSH_PORT=""; DRY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --server)   SERVER_PATH="$2"; shift 2 ;;
        --web)      WEB_PATH="$2"; shift 2 ;;
        --ssh-port) SSH_PORT="$2"; shift 2 ;;
        --dry-run)  DRY=1; shift ;;
        -h|--help)  usage 0 ;;
        *) error "unknown arg: $1"; usage 2 ;;
    esac
done

[ -z "$SERVER_PATH" ] && { error "no --server <path> given (required, no default)"; usage 2; }

# ── Validate artifacts ────────────────────────────────────────────────────
SERVER_DIST="$PROJECT_DIR/dist/server"
WEB_DIST="$PROJECT_DIR/dist/sandbox"

if [ ! -x "$SERVER_DIST/sandbox" ] || [ ! -d "$SERVER_DIST/assets" ]; then
    error "no server bundle at $SERVER_DIST (missing sandbox binary or assets/)"
    info  "build it first:  ./scripts/build.sh sandbox-server --release"
    exit 1
fi
if [ -n "$WEB_PATH" ] && [ ! -d "$WEB_DIST" ]; then
    error "no wasm bundle at $WEB_DIST"
    info  "build it first:  ./scripts/build_web.sh build sandbox --release"
    exit 1
fi

# ── rsync ─────────────────────────────────────────────────────────────────
RSH="ssh${SSH_PORT:+ -p $SSH_PORT}"
rs_d() { rsync -a --delete --info=progress2 -e "$RSH" ${DRY:+-n} "$@"; }
remote() { ssh ${SSH_PORT:+-p "$SSH_PORT"} "$HOST" "$@"; }

info "host    : $HOST"
info "server  : $SERVER_DIST/sandbox  →  $HOST:$SERVER_PATH  ($(du -sh "$SERVER_DIST/sandbox" | cut -f1))"
[ -n "$WEB_PATH" ] && info "web     : $WEB_DIST/  →  $HOST:$WEB_PATH"
[ "$DRY" -eq 1 ] && warn "DRY RUN — no files will be written"

# Server binary + assets. --exclude='/web/' so a co-located web dir isn't wiped.
info "uploading server binary + assets → $HOST:$SERVER_PATH …"
remote "mkdir -p '$SERVER_PATH'"
rs_d --exclude='/web/' "$SERVER_DIST/" "$HOST:$SERVER_PATH/"

# Optional wasm client bundle.
if [ -n "$WEB_PATH" ]; then
    info "uploading wasm client bundle → $HOST:$WEB_PATH …"
    remote "mkdir -p '$WEB_PATH'"
    rs_d "$WEB_DIST/" "$HOST:$WEB_PATH/"
fi

success "artifacts uploaded"
info "nothing was restarted. run the binary however you run it on the box."
