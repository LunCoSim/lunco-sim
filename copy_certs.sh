#!/bin/bash
# Copy Let's Encrypt SSL certificates to project .cert directory
# This ensures proper permissions for Godot server to access certificates

set -e

# Configuration
DOMAIN="langrenus.lunco.space"
CERT_DIR=".cert"
LETSENCRYPT_DIR="/etc/letsencrypt/live/${DOMAIN}"

echo "🔒 Setting up SSL certificates for LunCo server"
echo "=============================================="
echo "Domain: $DOMAIN"
echo "Source: $LETSENCRYPT_DIR"
echo "Destination: $CERT_DIR"
echo ""

# Check if Let's Encrypt certificates exist
if [[ ! -f "$LETSENCRYPT_DIR/fullchain.pem" || ! -f "$LETSENCRYPT_DIR/privkey.pem" ]]; then
    echo "❌ Let's Encrypt certificates not found!"
    echo "Expected at: $LETSENCRYPT_DIR"
    echo ""
    echo "Make sure certificates exist. You can check with:"
    echo "  sudo certbot certificates"
    echo ""
    echo "Or obtain new certificates:"
    echo "  sudo certbot certonly --standalone -d $DOMAIN"
    exit 1
fi

# Create certificate directory
echo "📁 Creating certificate directory..."
mkdir -p "$CERT_DIR"

# Copy certificates with proper permissions
echo "📋 Copying certificates..."

# Full certificate chain
if sudo cp "$LETSENCRYPT_DIR/fullchain.pem" "$CERT_DIR/"; then
    echo "✅ Copied fullchain.pem"
else
    echo "❌ Failed to copy fullchain.pem"
    exit 1
fi

# Private key
if sudo cp "$LETSENCRYPT_DIR/privkey.pem" "$CERT_DIR/"; then
    echo "✅ Copied privkey.pem"
else
    echo "❌ Failed to copy privkey.pem"
    exit 1
fi

# Set proper ownership and permissions
echo "🔐 Setting permissions..."
sudo chown "$USER:$USER" "$CERT_DIR"/*.pem
chmod 644 "$CERT_DIR/fullchain.pem"  # Certificate: readable by all
chmod 600 "$CERT_DIR/privkey.pem"    # Private key: owner only

# Verify files
echo "🔍 Verifying certificates..."
if [[ -f "$CERT_DIR/fullchain.pem" && -f "$CERT_DIR/privkey.pem" ]]; then
    echo "✅ Certificate files verified"
else
    echo "❌ Certificate files missing!"
    exit 1
fi

# Show file details
echo ""
echo "📊 Certificate details:"
ls -la "$CERT_DIR"
echo ""

# Show certificate info
echo "🔏 Certificate information:"
openssl x509 -in "$CERT_DIR/fullchain.pem" -noout -subject -dates
echo ""

echo "✨ SSL setup complete!"
echo ""
echo "You can now run the secure server with:"
echo "  ./godot42b1 --headless --server --certificate .cert/fullchain.pem --key .cert/privkey.pem ./main.tscn"
echo ""
echo "Or use the run_secure_server.sh script if you create one."
echo ""
echo "📅 Certificate will auto-renew via Let's Encrypt"
echo "   Run this script again after renewal: ./setup_ssl_certs.sh"
