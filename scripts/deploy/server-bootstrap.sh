#!/usr/bin/env bash
# ============================================================================
# LunCo server bootstrap — runs ON the server box (as root / via sudo).
# ============================================================================
# Idempotent provisioner: installs runtime deps, the service account + layout,
# the systemd unit, firewall rules, and (by default) a Let's Encrypt cert, then
# starts the headless server. Normally invoked for you by deploy_server.sh,
# which rsyncs the artifacts to a staging dir and runs this with --src.
#
# Usage (on the box):
#     sudo bash server-bootstrap.sh --src <staging-dir> --domain <d> [options]
#
# Options:
#     --src <dir>      staging dir holding `sandbox`, `assets/`, `deploy/`,
#                      `DEPLOY.md` (rsync'd by deploy_server.sh). Required.
#     --domain <d>     TLS / vhost domain         (default: sandbox.lunco.space)
#     --prefix <dir>   install root               (default: /opt/lunco)
#     --email <addr>   certbot registration email (recommended for Let's Encrypt)
#     --self-signed    DEV: skip Let's Encrypt; the server mints its own
#                      self-signed cert + digest (browsers need the #digest pin).
#     --web            also install nginx + serve the wasm bundle from
#                      <src>/web on 443 (needs <src>/web present).
#     --no-cert        skip cert provisioning entirely (you manage certs).
# ============================================================================
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SRC=""; DOMAIN="sandbox.lunco.space"; PREFIX="/opt/lunco"; EMAIL=""
SELF_SIGNED=0; WEB=0; NO_CERT=0
while [ $# -gt 0 ]; do
    case "$1" in
        --src)         SRC="$2"; shift 2 ;;
        --domain)      DOMAIN="$2"; shift 2 ;;
        --prefix)      PREFIX="$2"; shift 2 ;;
        --email)       EMAIL="$2"; shift 2 ;;
        --self-signed) SELF_SIGNED=1; shift ;;
        --web)         WEB=1; shift ;;
        --no-cert)     NO_CERT=1; shift ;;
        *) error "unknown arg: $1"; exit 2 ;;
    esac
done

[ "$(id -u)" -eq 0 ] || { error "run as root (sudo)"; exit 1; }
[ -n "$SRC" ] && [ -d "$SRC" ] || { error "--src <staging-dir> required and must exist"; exit 1; }
[ -x "$SRC/sandbox" ] || { error "no executable 'sandbox' binary in $SRC"; exit 1; }
[ -d "$SRC/assets" ]  || { error "no 'assets/' dir in $SRC"; exit 1; }
[ -d "$SRC/deploy" ]  || { error "no 'deploy/' kit in $SRC"; exit 1; }

# ── 1. Runtime packages ──────────────────────────────────────────────────
# The headless binary links no GPU/X11/audio — only the wayland-client + udev
# libs must be loadable (libwayland-client0 is linked-but-unused under --no-ui).
PKGS=(libwayland-client0 libudev1)
[ "$SELF_SIGNED" -eq 0 ] && [ "$NO_CERT" -eq 0 ] && PKGS+=(certbot)
[ "$WEB" -eq 1 ] && PKGS+=(nginx python3-certbot-nginx)
info "apt-get install: ${PKGS[*]}"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq "${PKGS[@]}"

# ── 2. Service account + layout ──────────────────────────────────────────
if ! id lunco &>/dev/null; then
    info "creating service user 'lunco'"
    useradd --system --home "$PREFIX" --shell /usr/sbin/nologin lunco
fi
install -d -o lunco -g lunco -m 0755 "$PREFIX" "$PREFIX/.cache" "$PREFIX/deploy"
install -d -o lunco -g lunco -m 0750 "$PREFIX/certs"

# ── 3. Binary + assets + deploy kit ──────────────────────────────────────
info "installing binary + assets → $PREFIX"
install -o lunco -g lunco -m 0755 "$SRC/sandbox" "$PREFIX/sandbox"
rsync -a --delete "$SRC/assets/" "$PREFIX/assets/"
cp -f "$SRC/deploy/"* "$PREFIX/deploy/" 2>/dev/null || true
[ -f "$SRC/DEPLOY.md" ] && cp -f "$SRC/DEPLOY.md" "$PREFIX/deploy/DEPLOY.md"
chown -R lunco:lunco "$PREFIX/assets" "$PREFIX/deploy"

# ── 4. systemd unit + env ────────────────────────────────────────────────
info "installing systemd unit"
install -m 0644 "$PREFIX/deploy/lunco-server.service" /etc/systemd/system/lunco-server.service
# The shipped unit hardcodes /opt/lunco; honor a custom --prefix (no-op if default).
if [ "$PREFIX" != "/opt/lunco" ]; then
    sed -i "s#/opt/lunco#$PREFIX#g" /etc/systemd/system/lunco-server.service
fi

ENV_FILE="$PREFIX/lunco-server.env"
if [ "$SELF_SIGNED" -eq 1 ]; then
    info "DEV self-signed: writing env WITHOUT cert vars (server mints its own)"
    {
        echo "# DEV: cert env intentionally unset → server uses a self-signed cert."
        echo "# Browsers must pin its #digest (see journal: 'WebTransport cert digest')."
        echo "RUST_LOG=info"
    } > "$ENV_FILE"
else
    info "Let's Encrypt: env points at the deploy-hook-copied cert"
    {
        echo "LUNCO_TLS_CERT=$PREFIX/certs/fullchain.pem"
        echo "LUNCO_TLS_KEY=$PREFIX/certs/privkey.pem"
        echo "RUST_LOG=info"
    } > "$ENV_FILE"
fi
chown root:lunco "$ENV_FILE"; chmod 0640 "$ENV_FILE"
systemctl daemon-reload

# ── 5. Firewall ──────────────────────────────────────────────────────────
if command -v ufw &>/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
    info "opening firewall: 5888/udp$([ "$WEB" -eq 1 ] && echo ' + 80,443/tcp')"
    ufw allow 5888/udp >/dev/null || true
    if [ "$WEB" -eq 1 ]; then ufw allow 80/tcp >/dev/null || true; ufw allow 443/tcp >/dev/null || true; fi
else
    warn "ufw not active — open UDP 5888$([ "$WEB" -eq 1 ] && echo ' + TCP 80/443') in your firewall/security-group manually."
fi

# ── 6. Let's Encrypt (default) ───────────────────────────────────────────
if [ "$NO_CERT" -eq 1 ] || [ "$SELF_SIGNED" -eq 1 ]; then
    info "cert provisioning skipped ($([ "$NO_CERT" -eq 1 ] && echo --no-cert || echo --self-signed))"
else
    HOOK=/etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh
    info "installing certbot deploy hook → $HOOK"
    install -d -m 0755 /etc/letsencrypt/renewal-hooks/deploy
    install -m 0755 "$PREFIX/deploy/certbot-deploy-hook.sh" "$HOOK"

    CERTBOT_ARGS=(certonly --non-interactive --agree-tos -d "$DOMAIN" --deploy-hook "$HOOK")
    [ -n "$EMAIL" ] && CERTBOT_ARGS+=(-m "$EMAIL") || CERTBOT_ARGS+=(--register-unsafely-without-email)
    # nginx plugin if we're also serving the web bundle; else standalone (needs
    # :80 free + DNS pointing here). certbot is idempotent — a valid cert is reused.
    if [ "$WEB" -eq 1 ]; then CERTBOT_ARGS+=(--nginx); else CERTBOT_ARGS+=(--standalone); fi
    info "certbot ${CERTBOT_ARGS[*]}"
    if certbot "${CERTBOT_ARGS[@]}"; then
        # Ensure the service-local copy exists even on a "cert unchanged" run.
        RENEWED_LINEAGE="/etc/letsencrypt/live/$DOMAIN" bash "$HOOK" || true
        success "cert ready under $PREFIX/certs"
    else
        error "certbot failed — check DNS for $DOMAIN points here + port 80 is reachable."
        error "the service will PANIC on start with cert env set but no cert (fail-loud)."
    fi
fi

# ── 7. nginx vhost (optional web tier) ───────────────────────────────────
if [ "$WEB" -eq 1 ]; then
    if [ -d "$SRC/web" ]; then
        info "installing wasm bundle → $PREFIX/web/sandbox + nginx vhost"
        install -d -o lunco -g lunco "$PREFIX/web/sandbox"
        rsync -a --delete "$SRC/web/" "$PREFIX/web/sandbox/"; chown -R lunco:lunco "$PREFIX/web"
        install -m 0644 "$PREFIX/deploy/nginx-sandbox.lunco.space.conf" \
            /etc/nginx/sites-available/"$DOMAIN"
        ln -sf /etc/nginx/sites-available/"$DOMAIN" /etc/nginx/sites-enabled/"$DOMAIN"
        nginx -t && systemctl reload nginx
    else
        warn "--web set but no <src>/web bundle present — skipping nginx. Deploy the client with deploy_web.sh."
    fi
fi

# ── 8. Start ─────────────────────────────────────────────────────────────
info "enabling + starting lunco-server"
systemctl enable lunco-server >/dev/null 2>&1 || true
systemctl restart lunco-server
sleep 2
systemctl --no-pager --full status lunco-server | head -12 || true
echo
success "bootstrap done. Tail logs:  journalctl -u lunco-server -f"
info "expect:  '🔐 WebTransport using cert from …'  and  '[net] host listening on 0.0.0.0:5888'"
