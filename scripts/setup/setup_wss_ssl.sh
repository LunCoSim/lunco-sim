#!/bin/bash
# Setup WSS (WebSocket Secure) for LunCo Godot Project
# This script helps automate the certificate setup for WSS connections

set -e

DOMAIN="${1:-langrenus.lunco.space}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LUNCO_PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CERT_DIR="$LUNCO_PROJECT_DIR/pyscripts"

echo "🔒 WSS Setup Script for LunCo"
echo "=============================="
echo "Domain: $DOMAIN"
echo "Project: $LUNCO_PROJECT_DIR"
echo ""

# Check if certbot is installed
if ! command -v certbot &> /dev/null; then
    echo "📦 Installing certbot..."
    sudo apt-get update
    sudo apt-get install -y certbot
fi

# Check if certificate already exists
if [ -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]; then
    echo "✅ Certificate already exists for $DOMAIN"
else
    echo "🔐 Obtaining new certificate for $DOMAIN..."
    sudo certbot certonly --standalone -d "$DOMAIN"
fi

# Copy certificates to project
echo "📋 Copying certificates to $CERT_DIR and .cert/..."
mkdir -p "$LUNCO_PROJECT_DIR/.cert"
sudo cp "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" "$CERT_DIR/server.crt"
sudo cp "/etc/letsencrypt/live/$DOMAIN/privkey.pem" "$CERT_DIR/server.key"
sudo cp "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" "$LUNCO_PROJECT_DIR/.cert/fullchain.pem"
sudo cp "/etc/letsencrypt/live/$DOMAIN/privkey.pem" "$LUNCO_PROJECT_DIR/.cert/privkey.pem"

# Fix permissions
echo "🔓 Fixing permissions..."
sudo chown "$USER:$USER" "$CERT_DIR/server.crt"
sudo chown "$USER:$USER" "$CERT_DIR/server.key"
sudo chown "$USER:$USER" "$LUNCO_PROJECT_DIR/.cert/fullchain.pem"
sudo chown "$USER:$USER" "$LUNCO_PROJECT_DIR/.cert/privkey.pem"
chmod 600 "$CERT_DIR/server.key"
chmod 644 "$CERT_DIR/server.crt"
chmod 600 "$LUNCO_PROJECT_DIR/.cert/privkey.pem"
chmod 644 "$LUNCO_PROJECT_DIR/.cert/fullchain.pem"

# Verify certificates
echo "✔️  Verifying certificates..."
openssl x509 -in "$CERT_DIR/server.crt" -noout -text | grep -A 2 "Subject:"
echo ""

# Create renewal hook
echo "🔄 Setting up auto-renewal..."
RENEWAL_HOOK="/etc/letsencrypt/renewal-hooks/post/lunco-renewal.sh"

sudo tee "$RENEWAL_HOOK" > /dev/null <<EOF
#!/bin/bash
# Sync to pyscripts/
cp /etc/letsencrypt/live/$DOMAIN/fullchain.pem $CERT_DIR/server.crt
cp /etc/letsencrypt/live/$DOMAIN/privkey.pem $CERT_DIR/server.key
chown $USER:$USER $CERT_DIR/server.*
chmod 600 $CERT_DIR/server.key
chmod 644 $CERT_DIR/server.crt

# Sync to .cert/
cp /etc/letsencrypt/live/$DOMAIN/fullchain.pem $LUNCO_PROJECT_DIR/.cert/fullchain.pem
cp /etc/letsencrypt/live/$DOMAIN/privkey.pem $LUNCO_PROJECT_DIR/.cert/privkey.pem
chown $USER:$USER $LUNCO_PROJECT_DIR/.cert/*.pem
chmod 600 $LUNCO_PROJECT_DIR/.cert/privkey.pem
chmod 644 $LUNCO_PROJECT_DIR/.cert/fullchain.pem

echo "🔄 LunCo certificates renewed at \$(date)" >> /var/log/letsencrypt/lunco-renewal.log
EOF

sudo chmod +x "$RENEWAL_HOOK"

echo "✅ Renewal hook created at $RENEWAL_HOOK"
echo ""

# Test renewal
echo "🧪 Testing certificate renewal (dry-run)..."
sudo certbot renew --dry-run --quiet

echo ""
echo "✨ WSS Setup Complete!"
echo ""
echo "Next steps:"
echo "1. In your Godot code, connect with TLS enabled:"
echo "   Networking.connect_to_server(\"$DOMAIN\", 9000, true)"
echo ""
echo "2. To host a server with TLS:"
echo "   Networking.host(9000, \"user://server.crt\", \"user://server.key\")"
echo ""
echo "3. Test WSS connection:"
echo "   npm install -g wscat"
echo "   wscat -c wss://$DOMAIN:9000"
echo ""
echo "Certificate details:"
echo "  Cert: $CERT_DIR/server.crt"
echo "  Key: $CERT_DIR/server.key"
echo ""
echo "Auto-renewal: Enabled (via $RENEWAL_HOOK)"
