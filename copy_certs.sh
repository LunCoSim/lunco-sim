#!/bin/bash
# Copy Let's Encrypt SSL certificates to project .cert directory
# This ensures proper permissions for Godot server to access certificates

set -e

# Configuration - modify these paths if your certificates are in different locations
DOMAIN="langrenus.lunco.space"
CERT_DIR=".cert"
LETSENCRYPT_DIR="/etc/letsencrypt/live/${DOMAIN}"

# Alternative: Direct paths (uncomment and modify if you have certificates in different locations)
# NOTE: If you uncomment these, comment out the auto-detection section below
# FULLCHAIN_SRC="/etc/letsencrypt/live/langrenus.lunco.space/fullchain.pem"
# PRIVKEY_SRC="/etc/letsencrypt/live/langrenus.lunco.space/privkey.pem"

echo "🔒 Setting up SSL certificates for LunCo server"
echo "=============================================="
echo "Domain: $DOMAIN"
echo "Source: Auto-detect Let's Encrypt or manual paths"
echo "Destination: $CERT_DIR"
echo ""

# Check for certificates in different locations
CERT_FOUND=false

# First try: Standard Let's Encrypt location
if [[ -f "$LETSENCRYPT_DIR/fullchain.pem" && -f "$LETSENCRYPT_DIR/privkey.pem" ]]; then
    echo "✅ Found certificates in standard Let's Encrypt location"
    FULLCHAIN_SRC="$LETSENCRYPT_DIR/fullchain.pem"
    PRIVKEY_SRC="$LETSENCRYPT_DIR/privkey.pem"
    CERT_FOUND=true
fi

# Alternative locations (uncomment the variables at the top if needed)
if [[ -n "${FULLCHAIN_SRC:-}" && -n "${PRIVKEY_SRC:-}" ]]; then
    if [[ -f "$FULLCHAIN_SRC" && -f "$PRIVKEY_SRC" ]]; then
        echo "✅ Using manually specified certificate paths"
        CERT_FOUND=true
    fi
fi

# Manual placement: Check if user already placed files in .cert directory
if [[ ! $CERT_FOUND && -f ".cert/fullchain.pem" && -f ".cert/privkey.pem" ]]; then
    echo "✅ Found certificates already in .cert directory"
    echo "Using existing certificates..."
    CERT_FOUND=true
fi

if [[ ! $CERT_FOUND ]]; then
    cat << 'EOF'
❌ No certificates found!

You can place certificates manually:
  mkdir -p .cert
  cp /path/to/your/fullchain.pem .cert/
  cp /path/to/your/privkey.pem .cert/
  sudo chown $USER:$USER .cert/*.pem
  chmod 644 .cert/fullchain.pem
  chmod 600 .cert/privkey.pem

Or obtain new certificates:
  sudo certbot certonly --standalone -d langrenus.lunco.space

Or check existing certificates:
  sudo certbot certificates

EOF
    exit 1
fi

# Create certificate directory
echo "📁 Creating certificate directory..."
mkdir -p "$CERT_DIR"

# Copy certificates with proper permissions
echo "📋 Copying certificates..."

# Only copy if source files exist and are different from destination
if [[ "$FULLCHAIN_SRC" != "$CERT_DIR/fullchain.pem" ]]; then
    echo "Copying from: $FULLCHAIN_SRC"
    if sudo cp "$FULLCHAIN_SRC" "$CERT_DIR/"; then
        echo "✅ Copied fullchain.pem"
    else
        echo "❌ Failed to copy fullchain.pem"
        exit 1
    fi
else
    echo "✅ fullchain.pem already in place"
fi

if [[ "$PRIVKEY_SRC" != "$CERT_DIR/privkey.pem" ]]; then
    if sudo cp "$PRIVKEY_SRC" "$CERT_DIR/"; then
        echo "✅ Copied privkey.pem"
    else
        echo "❌ Failed to copy privkey.pem"
        exit 1
    fi
else
    echo "✅ privkey.pem already in place"
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
