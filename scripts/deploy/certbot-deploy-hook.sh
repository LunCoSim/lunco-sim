#!/usr/bin/env bash
# Certbot deploy hook for the LunCo headless server.
#
# WHY THIS EXISTS
#   1. The unprivileged `lunco` service user cannot read
#      /etc/letsencrypt/live/<domain>/privkey.pem (root:root 0600). This hook
#      copies the cert into a service-owned dir it CAN read.
#   2. lunco-server reads the PEM ONCE at startup (resolve_identity), so after a
#      renewal it keeps serving the OLD in-memory cert until restarted. This
#      hook restarts the unit so the new cert takes effect.
#
# INSTALL
#   sudo install -m 0755 scripts/deploy/certbot-deploy-hook.sh \
#        /etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh
#   (certbot runs every script in renewal-hooks/deploy/ after a successful
#    renewal, with $RENEWED_LINEAGE set to the live/<domain> dir.)
#
# It also runs on the FIRST issuance if you pass --deploy-hook to certbot, e.g.
#   sudo certbot certonly --nginx -d sandbox.lunco.space \
#        --deploy-hook /etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh
set -euo pipefail

DOMAIN="sandbox.lunco.space"
DEST="/opt/lunco/certs"
SVC_USER="lunco"
SVC_GROUP="lunco"
UNIT="lunco-server"

# $RENEWED_LINEAGE is set by certbot to the live dir of the renewed cert.
# Fall back to the well-known path so the hook also works run by hand.
SRC="${RENEWED_LINEAGE:-/etc/letsencrypt/live/${DOMAIN}}"

# Only act on OUR domain (certbot runs every deploy hook for every lineage).
if [[ "${RENEWED_LINEAGE:-}" == */live/* && "$(basename "$SRC")" != "$DOMAIN" ]]; then
    exit 0
fi

if [[ ! -r "$SRC/fullchain.pem" || ! -r "$SRC/privkey.pem" ]]; then
    echo "lunco deploy hook: cert not found under $SRC" >&2
    exit 1
fi

install -d -o "$SVC_USER" -g "$SVC_GROUP" -m 0750 "$DEST"
# 0640 + group lunco: readable by the service, not world-readable.
install -o "$SVC_USER" -g "$SVC_GROUP" -m 0644 "$SRC/fullchain.pem" "$DEST/fullchain.pem"
install -o "$SVC_USER" -g "$SVC_GROUP" -m 0640 "$SRC/privkey.pem"   "$DEST/privkey.pem"

echo "lunco deploy hook: copied $DOMAIN cert to $DEST, restarting $UNIT"
# Restart so the server re-reads the cert. Use try-restart so we don't START a
# unit the operator deliberately stopped.
systemctl try-restart "$UNIT.service" || true
