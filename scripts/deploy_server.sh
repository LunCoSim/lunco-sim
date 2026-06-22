#!/usr/bin/env bash
# ============================================================================
# LunCoSim — native SERVER deploy
# ============================================================================
# Ships the headless server binary + assets + deploy kit to a box over SSH and
# provisions it (systemd + firewall + Let's Encrypt by default), then starts it.
# Counterpart to deploy_web.sh (the CLIENT deploy). No artifact compression — a
# native binary doesn't benefit from the brotli/gzip pass the wasm bundle gets.
#
# Usage:
#     ./scripts/deploy_server.sh <user@host> [options]
#
# Options:
#     --domain <d>     TLS domain          (default: sandbox.lunco.space)
#     --release        ship target/release/sandbox  (DEFAULT)
#     --dev            ship target/debug/sandbox instead
#     --email <addr>   certbot email (recommended)
#     --self-signed    DEV: skip Let's Encrypt; server self-signs (#digest pin)
#     --web            also ship dist/sandbox/ + set up nginx on the box
#     --no-cert        skip cert provisioning (you manage certs)
#     --prefix <dir>   install root on box (default: /opt/lunco)
#     --stage <dir>    staging dir on box  (default: ~/lunco-deploy)
#     --ssh-port <n>   non-default SSH port
#     --no-provision   rsync only; print the bootstrap command to run by hand
#     --dry-run        rsync -n + don't run remote provisioning
#
# By DEFAULT the server is provisioned with a real Let's Encrypt certificate
# (needs the --domain DNS pointing at the box + port 80 reachable). Use
# --self-signed for a quick dev/localhost box.
#
# Examples:
#     ./scripts/build.sh sandbox-server --release
#     ./scripts/deploy_server.sh deploy@sandbox.lunco.space --email you@lunco.space
#     # dev box, no real cert, also serve the web client:
#     ./scripts/deploy_server.sh deploy@1.2.3.4 --self-signed --web
# ============================================================================
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() { sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

HOST="${1:-}"; [ -z "$HOST" ] && usage 2
case "$HOST" in -h|--help) usage 0 ;; esac
shift

DOMAIN="sandbox.lunco.space"; PREFIX="/opt/lunco"
STAGE="lunco-deploy"; SSH_PORT=""; EMAIL=""
SELF_SIGNED=0; WEB=0; NO_CERT=0; NO_PROVISION=0; DRY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --domain)       DOMAIN="$2"; shift 2 ;;
        # Binary profile is decided at BUILD time (build.sh sandbox-server
        # [--release] stages dist/server/); accept these for muscle-memory.
        --release|--dev) shift ;;
        --email)        EMAIL="$2"; shift 2 ;;
        --self-signed)  SELF_SIGNED=1; shift ;;
        --web)          WEB=1; shift ;;
        --no-cert)      NO_CERT=1; shift ;;
        --prefix)       PREFIX="$2"; shift 2 ;;
        --stage)        STAGE="$2"; shift 2 ;;
        --ssh-port)     SSH_PORT="$2"; shift 2 ;;
        --no-provision) NO_PROVISION=1; shift ;;
        --dry-run)      DRY=1; shift ;;
        -h|--help)      usage 0 ;;
        *) error "unknown arg: $1"; usage 2 ;;
    esac
done

# Self-contained server bundle assembled by build.sh sandbox-server.
DIST="$PROJECT_DIR/dist/server"
if [ ! -x "$DIST/sandbox" ] || [ ! -d "$DIST/assets" ] || [ ! -d "$DIST/deploy" ]; then
    error "no server bundle at $DIST (missing sandbox/assets/deploy)"
    info  "build it first:  ./scripts/build.sh sandbox-server --release"
    exit 1
fi
if [ "$WEB" -eq 1 ] && [ ! -d "$PROJECT_DIR/dist/sandbox" ]; then
    error "--web set but no dist/sandbox/ — build it: ./scripts/build.sh sandbox --release"
    exit 1
fi

SSH=(ssh); [ -n "$SSH_PORT" ] && SSH=(ssh -p "$SSH_PORT")
RSH="ssh"; [ -n "$SSH_PORT" ] && RSH="ssh -p $SSH_PORT"
RS=(rsync -a --info=progress2 -e "$RSH"); [ "$DRY" -eq 1 ] && RS+=(-n)
DELRS=(rsync -a --delete --info=progress2 -e "$RSH"); [ "$DRY" -eq 1 ] && DELRS+=(-n)

info "bundle : $DIST"
info "target : $HOST:$STAGE  → install $PREFIX"
info "cert   : $([ "$SELF_SIGNED" -eq 1 ] && echo 'self-signed (dev)' || { [ "$NO_CERT" -eq 1 ] && echo 'unmanaged (--no-cert)' || echo "Let's Encrypt ($DOMAIN)"; })"

# ── Stage artifacts on the box (no compression) ──────────────────────────
"${SSH[@]}" "$HOST" "mkdir -p '$STAGE'" 2>/dev/null || { [ "$DRY" -eq 1 ] || { error "ssh to $HOST failed"; exit 1; }; }
info "uploading server bundle (dist/server/ → $STAGE)…"
# Exclude web/ from --delete so a separately-shipped wasm bundle isn't nuked
# (and re-uploaded) on every server deploy.
"${DELRS[@]}" --exclude='/web/' "$DIST/" "$HOST:$STAGE/"
if [ "$WEB" -eq 1 ]; then
    info "uploading wasm bundle (dist/sandbox/ → $STAGE/web)…"
    "${DELRS[@]}" "$PROJECT_DIR/dist/sandbox/" "$HOST:$STAGE/web/"
fi
success "artifacts staged at $HOST:$STAGE"

# ── Provision on the box ─────────────────────────────────────────────────
BOOT_FLAGS=(--src "$STAGE" --domain "$DOMAIN" --prefix "$PREFIX")
[ "$SELF_SIGNED" -eq 1 ] && BOOT_FLAGS+=(--self-signed)
[ "$WEB" -eq 1 ]         && BOOT_FLAGS+=(--web)
[ "$NO_CERT" -eq 1 ]     && BOOT_FLAGS+=(--no-cert)
[ -n "$EMAIL" ]          && BOOT_FLAGS+=(--email "$EMAIL")
BOOT_CMD="sudo bash '$STAGE/deploy/server-bootstrap.sh' ${BOOT_FLAGS[*]}"

if [ "$DRY" -eq 1 ] || [ "$NO_PROVISION" -eq 1 ]; then
    warn "skipping remote provisioning ($([ "$DRY" -eq 1 ] && echo --dry-run || echo --no-provision))."
    info "run it on the box with:"
    echo "    $RSH $HOST \"$BOOT_CMD\""
    exit 0
fi

info "provisioning on $HOST (sudo)…"
"${SSH[@]}" -t "$HOST" "$BOOT_CMD"
success "server deployed to $HOST"
info "verify:  $RSH $HOST 'journalctl -u lunco-server -n 30 --no-pager'"
