#!/usr/bin/env bash
# ============================================================================
# LunCoSim — Web Deploy Script
# ============================================================================
# Pre-compresses wasm/JS/HTML with brotli + gzip, then rsyncs the
# whole `dist/<bin>/` tree to a remote server over SSH.
#
# Usage:
#     ./scripts/deploy_web.sh <user@host:/remote/path>
#     # or:
#     DEPLOY_TARGET="user@host:/var/www/lunco" ./scripts/deploy_web.sh
#     # build a different binary first:
#     BIN=sandbox_web ./scripts/deploy_web.sh user@host:/path
#
# Environment variables:
#     BIN              binary name (default: lunica_web)
#     DEPLOY_TARGET    rsync destination (overrides positional arg)
#     SSH_PORT         non-default SSH port (passed via -e "ssh -p N")
#     EXTRA_RSYNC      extra rsync args, e.g. "-n" for dry-run
#
# What gets compressed:
#     *.wasm  *.js  *.html  *.json  *.css  *.svg
#         → sibling .gz (gzip max)
#     *.zst (already zstd-compressed) — left alone, recompressing
#         actually grows them.
#
# Server side: nginx with `gzip_static on;` will serve the `.gz`
# sibling automatically when the client sends
# `Accept-Encoding: gzip`. Make sure `application/wasm` is mapped
# correctly — see the config snippet at the end of this script's
# output.
# ============================================================================

set -euo pipefail

# ── Colors ─────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# ── Config ─────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

BIN="${BIN:-lunica_web}"
TARGET="${DEPLOY_TARGET:-${1:-}}"
SSH_PORT="${SSH_PORT:-}"
EXTRA_RSYNC="${EXTRA_RSYNC:-}"
DIST_DIR="$PROJECT_DIR/dist/$BIN"

if [ -z "$TARGET" ]; then
    error "No deploy target."
    cat >&2 <<EOF

usage: $0 <user@host:/remote/path>
   or: DEPLOY_TARGET="user@host:/path" $0

example:
   ./scripts/deploy_web.sh deploy@lunco.dev:/var/www/lunco
   BIN=sandbox_web $0 deploy@lunco.dev:/var/www/rover
EOF
    exit 2
fi

if [ ! -d "$DIST_DIR" ]; then
    error "No bundle at $DIST_DIR"
    info  "Run: ./scripts/build_web.sh build $BIN  (then deploy)"
    exit 1
fi

# ── Pre-compression ────────────────────────────────────────────────────
# Walk every file we care about and produce sibling .br / .gz copies.
# Servers configured with brotli_static / gzip_static serve them at
# request time without compressing on-the-fly. Wasm typically shrinks
# ~4× under brotli; HTML/JS/JSON are similar.
#
# We *skip* files already in efficient final formats (zstd, brotli,
# gzip, png, jpg, woff2). Recompressing them is pure overhead and
# typically grows the output.

shopt -s globstar nullglob
COMPRESSIBLE_EXTS=(wasm js html json css svg ts xml txt map)
SKIP_EXTS=(zst br gz png jpg jpeg gif webp woff woff2 ttf otf eot ico)

is_skip_ext() {
    local ext=$1
    for s in "${SKIP_EXTS[@]}"; do [ "$ext" = "$s" ] && return 0; done
    return 1
}
is_compressible_ext() {
    local ext=$1
    for c in "${COMPRESSIBLE_EXTS[@]}"; do [ "$ext" = "$c" ] && return 0; done
    return 1
}

# Brotli is preferred (typically ~20% smaller than gzip-9 on wasm);
# gzip is the universal fallback for clients that don't send
# `Accept-Encoding: br` (rare in modern browsers but cheap insurance).
HAVE_BROTLI=0
if command -v brotli &> /dev/null; then
    HAVE_BROTLI=1
else
    warn "brotli not installed — skipping .br siblings (install: apt install brotli)"
fi

info "Pre-compressing files in $DIST_DIR (gzip -9$([ $HAVE_BROTLI -eq 1 ] && echo ' + brotli -q 11'))…"
total_raw=0
total_gz=0
total_br=0
file_count=0

while IFS= read -r -d '' f; do
    rel="${f#$DIST_DIR/}"
    base="${rel##*/}"
    ext="${base##*.}"
    [ "$base" = "$ext" ] && ext=""    # files with no extension
    is_skip_ext "$ext" && continue
    is_compressible_ext "$ext" || continue

    raw_size=$(stat -c '%s' "$f" 2>/dev/null || stat -f '%z' "$f")
    total_raw=$((total_raw + raw_size))
    file_count=$((file_count + 1))

    # -k keeps original, -f overwrites existing .gz, -n drops mtime
    # for reproducible output, -9 = max compression.
    gzip -kfn -9 "$f"
    gz_size=$(stat -c '%s' "$f.gz" 2>/dev/null || stat -f '%z' "$f.gz")
    total_gz=$((total_gz + gz_size))

    # -q 11 = max quality (slow but one-shot at deploy time).
    # --large_window=24 lets the encoder use a 16 MB window — meaningful
    # for the 20+ MB wasm blob. Some old decoders cap at window=22, but
    # every browser brotli decoder accepts up to 24.
    if [ $HAVE_BROTLI -eq 1 ]; then
        brotli -f -k -q 11 --large_window=24 -o "$f.br" "$f"
        br_size=$(stat -c '%s' "$f.br" 2>/dev/null || stat -f '%z' "$f.br")
        total_br=$((total_br + br_size))
    fi
done < <(find "$DIST_DIR" -type f -print0)

if [ $file_count -gt 0 ]; then
    raw_mb=$(awk "BEGIN{printf \"%.1f\", $total_raw/1048576}")
    gz_mb=$(awk  "BEGIN{printf \"%.1f\", $total_gz/1048576}")
    gz_ratio=$(awk  "BEGIN{printf \"%.1f\", $total_raw/$total_gz}")
    if [ $HAVE_BROTLI -eq 1 ]; then
        br_mb=$(awk    "BEGIN{printf \"%.1f\", $total_br/1048576}")
        br_ratio=$(awk "BEGIN{printf \"%.1f\", $total_raw/$total_br}")
        info "Compressed $file_count file(s): ${raw_mb} MB raw → ${gz_mb} MB gz (${gz_ratio}×), ${br_mb} MB br (${br_ratio}×)"
    else
        info "Compressed $file_count file(s): ${raw_mb} MB → ${gz_mb} MB (${gz_ratio}× smaller)"
    fi
else
    info "No compressible files found."
fi

# ── Rsync ──────────────────────────────────────────────────────────────
RSYNC_OPTS=(
    -avz
    --delete                  # remote becomes a mirror of dist/<bin>/
    --human-readable
    --progress
)
SSH_CMD="ssh"
[ -n "$SSH_PORT" ] && SSH_CMD="ssh -p $SSH_PORT"
RSYNC_OPTS+=(-e "$SSH_CMD")
# Allow EXTRA_RSYNC like "-n" (dry-run) without quoting issues.
read -r -a EXTRA_ARR <<< "$EXTRA_RSYNC"
[ ${#EXTRA_ARR[@]} -gt 0 ] && RSYNC_OPTS+=("${EXTRA_ARR[@]}")

# Trailing slash on source ↔ contents-of-dir, no slash on target.
SRC="$DIST_DIR/"
DST="${TARGET%/}/"

info "Uploading $SRC → $DST"
[ -n "$EXTRA_RSYNC" ] && info "Extra rsync args: $EXTRA_RSYNC"
rsync "${RSYNC_OPTS[@]}" "$SRC" "$DST"

success "Deployed $BIN to $TARGET"

# ── nginx configuration hint ──────────────────────────────────────────
cat <<'EOF'

Reminder — make sure your nginx site is configured to use the
pre-compressed `.gz` siblings (otherwise nginx compresses on-the-fly
on every request, or not at all):

    # /etc/nginx/sites-available/lunco
    server {
        listen 443 ssl http2;
        server_name lunco.example;
        root /var/www/lunco;

        # Make sure WASM has the right MIME type so the browser uses
        # streaming compile (`WebAssembly.instantiateStreaming`) — most
        # nginx installs don't ship a `application/wasm` mapping.
        types {
            application/wasm        wasm;
            application/javascript  js mjs;
        }

        # Serve pre-compressed siblings directly. brotli_static requires
        # ngx_brotli (apt install libnginx-mod-http-brotli on Debian/Ubuntu,
        # or nginx-extras). If brotli_static isn't available, drop the
        # line — gzip_static alone still works.
        brotli_static on;        # serve `<file>.br` for `Accept-Encoding: br`
        gzip_static on;          # serve `<file>.gz` for `Accept-Encoding: gzip`

        # Long cache for hashed assets (msl/sources-<sha>.tar.zst etc).
        # The wasm filename isn't hashed by default — if you want
        # immutable caching for it too, mention it and we can add a
        # post-build step to inject a content hash.
        location ~* \.(?:wasm|js|css|tar\.zst|bin\.zst)$ {
            add_header Cache-Control "public, max-age=31536000, immutable";
        }
        location = /index.html {
            add_header Cache-Control "no-cache";
        }

        # Serve the SPA's wasm-bindgen entrypoint by default.
        index index.html;
    }

After config change: `sudo nginx -t && sudo systemctl reload nginx`.

Verify the pre-compressed sibling is actually served:
    curl -I -H "Accept-Encoding: br" https://lunco.example/lunica_web_bg.wasm
    # expect:  Content-Encoding: br
    curl -I -H "Accept-Encoding: gzip" https://lunco.example/lunica_web_bg.wasm
    # expect:  Content-Encoding: gzip
EOF
