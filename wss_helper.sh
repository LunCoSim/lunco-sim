#!/bin/bash
# Complete WSS Deployment Helper
# This script helps with all aspects of WSS setup and maintenance

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOMAIN="${1:-langrenus.lunco.space}"
PROJECT_DIR="$SCRIPT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_header() {
    echo -e "${BLUE}════════════════════════════════════════${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}════════════════════════════════════════${NC}"
}

print_success() {
    echo -e "${GREEN}✅  $1${NC}"
}

print_error() {
    echo -e "${RED}❌  $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

# Command selection
if [ $# -eq 0 ]; then
    print_header "WSS Deployment Helper"
    echo ""
    echo "Usage: $0 <command> [domain]"
    echo ""
    echo "Commands:"
    echo "  setup              Setup WSS certificates and project"
    echo "  status             Check certificate status"
    echo "  renew              Manually renew certificates"
    echo "  test               Test WSS connection"
    echo "  list-certs         List all certificates"
    echo "  verify             Verify certificate configuration"
    echo "  clean              Clean up old certificates (BE CAREFUL)"
    echo ""
    echo "Examples:"
    echo "  $0 setup langrenus.lunco.space"
    echo "  $0 status langrenus.lunco.space"
    echo "  $0 test langrenus.lunco.space"
    echo ""
    exit 0
fi

COMMAND="$1"
DOMAIN="${2:-$DOMAIN}"

case "$COMMAND" in
    setup)
        print_header "WSS Setup for $DOMAIN"
        
        # Check certbot
        if ! command -v certbot &> /dev/null; then
            print_warning "Certbot not found, installing..."
            sudo apt-get update
            sudo apt-get install -y certbot
            print_success "Certbot installed"
        else
            print_success "Certbot found"
        fi
        
        # Get or refresh certificate
        if [ -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]; then
            print_info "Certificate already exists for $DOMAIN"
            read -p "Renew certificate? (y/n) " -n 1 -r
            echo
            if [[ $REPLY =~ ^[Yy]$ ]]; then
                sudo certbot renew --force-renewal -d "$DOMAIN"
            fi
        else
            print_info "Obtaining new certificate for $DOMAIN..."
            sudo certbot certonly --standalone -d "$DOMAIN"
        fi
        
        # Copy certificates
        CERT_DIR="$PROJECT_DIR/pyscripts"
        mkdir -p "$CERT_DIR"
        
        print_info "Copying certificates to $CERT_DIR..."
        sudo cp "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" "$CERT_DIR/server.crt"
        sudo cp "/etc/letsencrypt/live/$DOMAIN/privkey.pem" "$CERT_DIR/server.key"
        
        # Fix permissions
        sudo chown "$USER:$USER" "$CERT_DIR/server.crt"
        sudo chown "$USER:$USER" "$CERT_DIR/server.key"
        chmod 644 "$CERT_DIR/server.crt"
        chmod 600 "$CERT_DIR/server.key"
        
        print_success "Certificates copied and permissions fixed"
        
        # Verify
        if [ -f "$CERT_DIR/server.crt" ] && [ -f "$CERT_DIR/server.key" ]; then
            print_success "Certificate files verified"
        else
            print_error "Certificate files not found!"
            exit 1
        fi
        
        # Setup auto-renewal
        RENEWAL_HOOK="/etc/letsencrypt/renewal-hooks/post/lunco-renewal.sh"
        print_info "Setting up auto-renewal..."
        
        sudo tee "$RENEWAL_HOOK" > /dev/null <<EOF
#!/bin/bash
cp /etc/letsencrypt/live/$DOMAIN/fullchain.pem $CERT_DIR/server.crt
cp /etc/letsencrypt/live/$DOMAIN/privkey.pem $CERT_DIR/server.key
chown $USER:$USER $CERT_DIR/server.*
chmod 644 $CERT_DIR/server.crt
chmod 600 $CERT_DIR/server.key
echo "✅ LunCo certificates renewed: \$(date)" >> /var/log/letsencrypt/lunco-renewal.log
EOF
        
        sudo chmod +x "$RENEWAL_HOOK"
        print_success "Auto-renewal configured"
        
        # Test renewal
        print_info "Testing renewal (dry-run)..."
        sudo certbot renew --dry-run --quiet
        print_success "Renewal test passed"
        
        print_header "✅ WSS Setup Complete!"
        echo ""
        print_success "Certificate: $CERT_DIR/server.crt"
        print_success "Private Key: $CERT_DIR/server.key"
        echo ""
        echo "Next steps:"
        echo "1. Update Godot code to use tls=true"
        echo "2. Run: $0 test $DOMAIN"
        echo "3. Deploy and test from HTTPS page"
        ;;
        
    status)
        print_header "Certificate Status for $DOMAIN"
        
        if sudo certbot certificates 2>/dev/null | grep -q "$DOMAIN"; then
            sudo certbot certificates | grep -A 5 "$DOMAIN"
            
            print_info "Certificate details:"
            openssl x509 -in "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" -noout -dates
            
            EXPIRY=$(openssl x509 -enddate -noout -in "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" | cut -d= -f2)
            print_success "Certificate expires: $EXPIRY"
        else
            print_error "Certificate not found for $DOMAIN"
            exit 1
        fi
        ;;
        
    renew)
        print_header "Renewing Certificate for $DOMAIN"
        
        sudo certbot renew --force-renewal -d "$DOMAIN"
        
        # Copy renewed certificates
        CERT_DIR="$PROJECT_DIR/pyscripts"
        sudo cp "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" "$CERT_DIR/server.crt"
        sudo cp "/etc/letsencrypt/live/$DOMAIN/privkey.pem" "$CERT_DIR/server.key"
        sudo chown "$USER:$USER" "$CERT_DIR/server."*
        
        print_success "Certificate renewed and copied"
        echo "Restart your Godot server to use the new certificate"
        ;;
        
    test)
        print_header "Testing WSS Connection to $DOMAIN"
        
        # Check if wscat is installed
        if ! command -v wscat &> /dev/null; then
            print_warning "wscat not found, attempting to install..."
            if command -v npm &> /dev/null; then
                npm install -g wscat
                print_success "wscat installed"
            else
                print_error "npm not found. Install Node.js and npm first."
                exit 1
            fi
        fi
        
        print_info "Attempting to connect to wss://$DOMAIN:9000"
        echo ""
        print_info "If connection succeeds, type a test message and press Enter"
        print_info "Press Ctrl+C to exit"
        echo ""
        
        # Try connection with timeout
        timeout 5 wscat -c "wss://$DOMAIN:9000" 2>&1 || true
        
        if [ $? -eq 0 ]; then
            print_success "Connection successful!"
        else
            print_warning "Connection attempt completed or timed out"
            echo ""
            print_info "Troubleshooting:"
            echo "1. Is the server running on port 9000?"
            echo "2. Is the domain correct: $DOMAIN?"
            echo "3. Check firewall: sudo ufw allow 9000"
            echo "4. Check if server is actually running with certificates"
        fi
        ;;
        
    list-certs)
        print_header "List All Certificates"
        sudo certbot certificates
        ;;
        
    verify)
        print_header "Verify WSS Configuration"
        
        echo ""
        print_info "1. Checking Godot code..."
        if grep -r "tls.*true" "$PROJECT_DIR/core" 2>/dev/null | grep -q "connect_to_server"; then
            print_success "Found tls=true in Godot code"
        else
            print_warning "No tls=true found - ensure you update your game code"
        fi
        
        echo ""
        print_info "2. Checking certificate files..."
        CERT_DIR="$PROJECT_DIR/pyscripts"
        
        if [ -f "$CERT_DIR/server.crt" ]; then
            print_success "Found: $CERT_DIR/server.crt"
        else
            print_error "Missing: $CERT_DIR/server.crt"
        fi
        
        if [ -f "$CERT_DIR/server.key" ]; then
            print_success "Found: $CERT_DIR/server.key"
        else
            print_error "Missing: $CERT_DIR/server.key"
        fi
        
        echo ""
        print_info "3. Checking certificate validity..."
        if [ -f "$CERT_DIR/server.crt" ]; then
            openssl x509 -in "$CERT_DIR/server.crt" -noout -text | grep -A 1 "Subject:" || true
            
            EXPIRY=$(openssl x509 -enddate -noout -in "$CERT_DIR/server.crt" | cut -d= -f2)
            print_success "Certificate expires: $EXPIRY"
        fi
        
        echo ""
        print_info "4. Checking permissions..."
        ls -l "$CERT_DIR/server."* 2>/dev/null || print_warning "Certificate files not found"
        
        echo ""
        print_header "Verification Complete"
        ;;
        
    clean)
        print_header "⚠️  Certificate Cleanup"
        echo ""
        print_warning "This will remove old certificates. Make sure you have backups!"
        echo ""
        read -p "Are you sure? Type 'yes' to confirm: " confirm
        
        if [ "$confirm" = "yes" ]; then
            print_info "Removing certificate for $DOMAIN..."
            sudo certbot delete --cert-name "$DOMAIN"
            print_success "Certificate removed"
        else
            print_info "Cleanup cancelled"
        fi
        ;;
        
    *)
        print_error "Unknown command: $COMMAND"
        echo "Run without arguments for usage help"
        exit 1
        ;;
esac
