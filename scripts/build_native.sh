#!/usr/bin/env bash
# ============================================================================
# LunCoSim — native desktop build + package assembler
# ============================================================================
# Builds a LunCoSim desktop binary (lunica or sandbox) for the
# host platform (Linux, macOS, Windows) and assembles a self-contained
# distributable directory containing the binary, the assets/ tree, the
# relevant .cache/ subdirs (fonts, MSL, models, …), and a launcher script
# that sets LUNCOSIM_CACHE so the app finds bundled cache assets.
#
# Usage:
#     ./scripts/build_native.sh <binary> [--release] [--package] [options]
#
# Binaries:
#     lunica      — Modelica Workbench IDE (desktop GUI)
#     sandbox     — Rover Physics Sandbox (desktop GUI)
#
# Options:
#     --release          Optimized release build (default: dev)
#     --package          Create a .tar.gz (unix) or .zip (windows) archive
#     --target <triple>  Cross-compile target (default: host triple)
#     --no-cache         Skip bundling .cache/ subdirs (binary + assets only)
#     --skip-download    Skip the cache asset download step (use existing .cache/)
#     --full-cache       Bundle ALL .cache/ subdirs (default: per-binary subset)
#     --no-assets        Skip bundling the assets/ tree
#     --out <dir>        Output directory (default: dist/<binary>-<platform>-<arch>/)
#     --extra <args>     Pass extra args to cargo build
#
# Examples:
#     ./scripts/build_native.sh lunica --release --package
#     ./scripts/build_native.sh sandbox --release --package
#     ./scripts/build_native.sh lunica                    # quick dev build
#     ./scripts/build_native.sh sandbox --target aarch64-unknown-linux-gnu
#
# Platform detection is automatic. The script works on:
#   Linux   (x86_64 / aarch64)  — needs libasound2, libudev, libwayland, libxkbcommon
#   macOS   (x86_64 / arm64)    — needs nothing extra (Metal is built-in)
#   Windows (x86_64)            — needs MSVC build tools (Visual Studio)
#
# Cache assets are NOT downloaded by this script. Populate them first with:
#   cargo run -p lunco-assets -- download -p lunco-theme   # fonts
#   cargo run -p lunco-assets -- download -p lunco-modelica  # MSL (for lunica)
#   cargo run -p lunco-assets -- download -p lunco-usd && \
#     cargo run -p lunco-assets -- process -p lunco-usd     # rover model (for sandbox)
#
# The package layout:
#   dist/<binary>-<platform>-<arch>/
#     <binary>[.exe]          — the compiled binary
#     assets/                  — scene files, config, models, shaders
#     .cache/                  — fonts, MSL, models, ephemeris (what each binary needs)
#     run.sh / run.bat         — launcher that sets LUNCOSIM_CACHE and runs the binary
#     README.md                — quick-start for end users
# ============================================================================

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
    sed -n '2,56p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

# ── Map binary → cargo crate ──────────────────────────────────────────────
get_crate() {
    case "$1" in
        lunica)   echo "lunco-modelica" ;;
        sandbox)  echo "lunco-sandbox" ;;
        *) error "Unknown binary: $1"; error "Available: lunica, sandbox"; exit 1 ;;
    esac
}

# ── Detect host triple ────────────────────────────────────────────────────
detect_host_triple() {
    local ostype machine
    ostype="$(uname -s)"
    machine="$(uname -m)"
    case "$ostype" in
        Linux)
            case "$machine" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                *) error "Unsupported Linux arch: $machine"; exit 1 ;;
            esac ;;
        Darwin)
            case "$machine" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64|aarch64) echo "aarch64-apple-darwin" ;;
                *) error "Unsupported macOS arch: $machine"; exit 1 ;;
            esac ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$machine" in
                x86_64|amd64) echo "x86_64-pc-windows-msvc" ;;
                *) error "Unsupported Windows arch: $machine"; exit 1 ;;
            esac ;;
        *) error "Unsupported OS: $ostype"; exit 1 ;;
    esac
}

# ── Short platform name for dist folder naming ────────────────────────────
platform_short() {
    case "$1" in
        *linux*)   echo "linux" ;;
        *darwin*)  echo "macos" ;;
        *windows*) echo "windows" ;;
        *) echo "unknown" ;;
    esac
}

arch_short() {
    case "$1" in
        *x86_64*)  echo "x86_64" ;;
        *aarch64*) echo "aarch64" ;;
        *arm64*)   echo "aarch64" ;;
        *) echo "unknown" ;;
    esac
}

is_windows() { [[ "$1" == *windows* ]]; }

# ── Per-binary cache subdirs ──────────────────────────────────────────────
# Each binary needs a different subset of the .cache/ tree at runtime.
#   lunica:   fonts (UI fallback) + msl (Modelica Standard Library) + thermofluidstream
#   sandbox:  fonts + models (Perseverance rover glTF)
cache_subdirs_for() {
    case "$1" in
        lunica)   echo "fonts msl thermofluidstream" ;;
        sandbox)  echo "fonts models" ;;
    esac
}

# ── Resolve the cache directory ───────────────────────────────────────────
# Mirrors lunco_assets::cache_dir(): LUNCOSIM_CACHE env → workspace .cache/ →
# one level up (shared workspace cache) → OS cache dir. Returns "" if none
# found.
resolve_cache_dir() {
    if [ -n "${LUNCOSIM_CACHE:-}" ] && [ -d "$LUNCOSIM_CACHE" ]; then
        echo "$LUNCOSIM_CACHE"
        return
    fi
    for candidate in "$PROJECT_DIR/.cache" "$PROJECT_DIR/../.cache"; do
        if [ -d "$candidate" ]; then
            echo "$candidate"
            return
        fi
    done
    echo ""
}

# ── Download cache assets for a binary ────────────────────────────────────
# Runs `cargo run -p lunco-assets -- download` for each crate whose assets
# the binary needs, then `process` for crates with a process step (e.g.
# lunco-usd's glTF transform). Idempotent — re-running with a populated
# .cache/ is a no-op (the tool verifies sha256 and skips present files).
#
# Skipped when --no-cache is set, when the lunco-assets bin can't build
# (e.g. missing system deps on a fresh CI runner), or when SKIP_DOWNLOAD=1
# is exported. Downloads land in LUNCOSIM_CACHE (or the resolved cache dir).
download_cache_for() {
    local binary="$1"
    local cache_dir
    cache_dir="$(resolve_cache_dir)"
    if [ -z "$cache_dir" ]; then
        warn "No cache dir resolved — cannot download assets. Set LUNCOSIM_CACHE or run from a worktree."
        return 0
    fi
    info "Downloading cache assets for $binary → $cache_dir"

    # Each binary needs fonts (lunco-theme) + its own crate's assets.
    # sandbox also needs the processed glTF from lunco-usd.
    local crates_to_download=""
    local crates_to_process=""
    case "$binary" in
        lunica)
            crates_to_download="lunco-theme lunco-modelica"
            ;;
        sandbox)
            crates_to_download="lunco-theme lunco-usd"
            crates_to_process="lunco-usd"
            ;;
    esac

    # Export so the lunco-assets binary picks it up.
    export LUNCOSIM_CACHE="$cache_dir"

    for crate in $crates_to_download; do
        info "  downloading assets for $crate …"
        if cargo run -p lunco-assets -- download -p "$crate"; then
            success "  downloaded $crate assets"
        else
            warn "  download for $crate failed — continuing (build_native.sh will warn about missing .cache/ subdirs)"
        fi
    done
    for crate in $crates_to_process; do
        info "  processing assets for $crate …"
        if cargo run -p lunco-assets -- process -p "$crate"; then
            success "  processed $crate assets"
        else
            warn "  process for $crate failed — continuing (the raw asset may be missing or npx unavailable)"
        fi
    done
}

# ── Write the launcher script ─────────────────────────────────────────────
write_launcher_unix() {
    local dir="$1" binary="$2"
    local launcher="$dir/run.sh"
    cat > "$launcher" <<EOF
#!/usr/bin/env bash
# Launcher for $binary — sets LUNCOSIM_CACHE to the bundled .cache/ dir
# so the app finds fonts / MSL / models without a separate download step.
cd "\$(dirname "\$0")"
export LUNCOSIM_CACHE="\$PWD/.cache"
exec "./$binary" "\$@"
EOF
    chmod +x "$launcher"
}

write_launcher_windows() {
    local dir="$1" binary="$2"
    local launcher="$dir/run.bat"
    cat > "$launcher" <<EOF
@echo off
cd /d "%~dp0"
set LUNCOSIM_CACHE=%CD%\.cache
$binary.exe %*
EOF
}

# ── Write a README for the package ────────────────────────────────────────
write_readme() {
    local dir="$1" binary="$2" platform="$3" arch="$4"
    local readme="$dir/README.md"
    local run_cmd
    if is_windows "$platform"; then
        run_cmd="run.bat"
    else
        run_cmd="./run.sh"
    fi
    cat > "$readme" <<EOF
# LunCoSim — $binary ($platform-$arch)

## Quick start

Extract this archive, then run:

    $run_cmd

Or run the binary directly (from this directory):

    ./$binary$(is_windows "$platform" && echo ".exe")

## What's included

- \`$binary\` — the application binary
- \`assets/\` — scene files, config, models, shaders
- \`.cache/\` — fonts and runtime data (MSL, models, ephemeris as needed)
- \`docs/\` — architecture docs, tutorials, app guides
- \`AGENTS.md\` — AI agent guidelines for working on the codebase
- \`run.sh\` / \`run.bat\` — launcher that sets the cache path

## Documentation

See \`docs/\` for:
- \`README.md\` — reading order for newcomers
- \`crates-index.md\` — map of the ~50-crate workspace
- \`principles.md\` — non-negotiable design principles
- \`architecture/\` — numbered design docs (00s overview, 10s systems, etc.)
- \`tutorials/\` — user-facing tutorials

\`AGENTS.md\` documents the project conventions for AI agents (Bevy 0.18,
plugin layering, tunability mandate, TDD-first).

## Cache directory

The launcher sets \`LUNCOSIM_CACHE\` to the bundled \`.cache/\` directory.
If you move the binary without the cache, the app falls back to:
  - Linux:   ~/.cache/lunco/
  - macOS:   ~/Library/Caches/lunco/
  - Windows: %LOCALAPPDATA%\\lunco\\

Populate it with: \`cargo run -p lunco-assets -- download\`

## Build info

Built from LunCoSim source. See https://github.com/LunCoSim/luncosim-workspace
EOF
}

# ── Create a compressed archive ───────────────────────────────────────────
create_archive() {
    local dir="$1" platform="$2"
    local base
    base="$(basename "$dir")"
    local parent
    parent="$(dirname "$dir")"
    local archive
    if is_windows "$platform"; then
        archive="${dir}.zip"
        info "Creating .zip archive: $archive"
        if command -v 7z &>/dev/null; then
            (cd "$parent" && 7z a -tzip "$base.zip" "$base")
        elif command -v zip &>/dev/null; then
            (cd "$parent" && zip -r "$base.zip" "$base")
        else
            powershell -NoProfile -Command \
                "Compress-Archive -Path '$dir' -DestinationPath '$archive'"
        fi
    else
        archive="${dir}.tar.gz"
        info "Creating .tar.gz archive: $archive"
        tar -czf "$archive" -C "$parent" "$base"
    fi
    if [ -f "$archive" ]; then
        local size
        size=$(du -h "$archive" | cut -f1)
        success "Archive: $archive ($size)"
    else
        error "Archive creation failed"
        exit 1
    fi
}

# ── Parse arguments ───────────────────────────────────────────────────────
BINARY="${1:-}"
[ -z "$BINARY" ] && usage 2
case "$BINARY" in -h|--help) usage 0 ;; esac
shift

RELEASE=0
PACKAGE=0
TARGET=""
NO_CACHE=0
SKIP_DOWNLOAD=0
FULL_CACHE=0
NO_ASSETS=0
OUT_DIR=""
EXTRA_ARGS=()

while [ $# -gt 0 ]; do
    case "$1" in
        --release)       RELEASE=1; shift ;;
        --package)       PACKAGE=1; shift ;;
        --target)        TARGET="$2"; shift 2 ;;
        --target=*)      TARGET="${1#--target=}"; shift ;;
        --no-cache)      NO_CACHE=1; shift ;;
        --skip-download) SKIP_DOWNLOAD=1; shift ;;
        --full-cache)    FULL_CACHE=1; shift ;;
        --no-assets)     NO_ASSETS=1; shift ;;
        --out)           OUT_DIR="$2"; shift 2 ;;
        --out=*)         OUT_DIR="${1#--out=}"; shift ;;
        --extra)         EXTRA_ARGS+=("$2"); shift 2 ;;
        --extra=*)       EXTRA_ARGS+=("${1#--extra=}"); shift ;;
        -h|--help)       usage 0 ;;
        *)               error "Unknown option: $1"; usage 2 ;;
    esac
done

# Validate binary
case "$BINARY" in
    lunica|sandbox) ;;
    *) error "Unknown binary: $BINARY"; error "Available: lunica, sandbox"; exit 1 ;;
esac

CRATE="$(get_crate "$BINARY")"
HOST_TRIPLE="$(detect_host_triple)"
TRIPLE="${TARGET:-$HOST_TRIPLE}"
PLATFORM="$(platform_short "$TRIPLE")"
ARCH="$(arch_short "$TRIPLE")"

if [ -n "$TARGET" ]; then
    info "Cross-compiling: $TRIPLE (host: $HOST_TRIPLE)"
else
    info "Host build: $TRIPLE"
fi

# ── Build ─────────────────────────────────────────────────────────────────
PROFILE_ARGS=()
PROFILE_LABEL="dev"
if [ "$RELEASE" -eq 1 ]; then
    PROFILE_ARGS=(--release)
    PROFILE_LABEL="release"
else
    PROFILE_ARGS=(--profile dev)
fi

# Binary output path (cargo puts it in target/<triple-or-profile>/<bin>)
# For cross-compile: target/<triple>/release|debug/<bin>
# For host build:    target/release|debug/<bin>
if [ -n "$TARGET" ]; then
    if [ "$RELEASE" -eq 1 ]; then
        BIN_PATH="target/$TRIPLE/release/$BINARY"
    else
        BIN_PATH="target/$TRIPLE/debug/$BINARY"
    fi
    TARGET_ARGS=(--target "$TRIPLE")
else
    if [ "$RELEASE" -eq 1 ]; then
        BIN_PATH="target/release/$BINARY"
    else
        BIN_PATH="target/debug/$BINARY"
    fi
    TARGET_ARGS=()
fi

# Add .exe for Windows targets
if is_windows "$TRIPLE"; then
    BIN_PATH="${BIN_PATH}.exe"
fi

info "Building $BINARY ($CRATE) — $PROFILE_LABEL, target: $TRIPLE"
cd "$PROJECT_DIR"

cargo build "${PROFILE_ARGS[@]}" "${TARGET_ARGS[@]}" \
    --bin "$BINARY" -p "$CRATE" \
    "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}"

if [ ! -f "$PROJECT_DIR/$BIN_PATH" ]; then
    error "Build succeeded but binary not found at $BIN_PATH"
    exit 1
fi
success "Binary built: $BIN_PATH ($(du -h "$PROJECT_DIR/$BIN_PATH" | cut -f1))"

# ── Download cache assets before staging ──────────────────────────────────
# Runs `cargo run -p lunco-assets -- download` for the crates this binary
# needs (fonts, MSL, models). Skipped with --skip-download or --no-cache.
# Idempotent — re-runs verify sha256 and skip already-present files.
if [ "$NO_CACHE" -eq 0 ] && [ "$SKIP_DOWNLOAD" -eq 0 ]; then
    download_cache_for "$BINARY"
elif [ "$SKIP_DOWNLOAD" -eq 1 ]; then
    info "Skipping cache download (--skip-download)"
fi

# ── Stage the package ─────────────────────────────────────────────────────
if [ -z "$OUT_DIR" ]; then
    OUT_DIR="$PROJECT_DIR/dist/${BINARY}-${PLATFORM}-${ARCH}"
fi

info "Staging package → $OUT_DIR"
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Copy the binary
cp -f "$PROJECT_DIR/$BIN_PATH" "$OUT_DIR/"
BIN_NAME="$(basename "$BIN_PATH")"

# Strip the shipped copy (release profile already strips, but dev builds
# carry ~3+ GB of debug info). The original target/ binary keeps symbols
# for local debugging.
if [ "$RELEASE" -eq 0 ]; then
    if is_windows "$TRIPLE"; then
        : # No strip on Windows (release profile handles it; dev is dev)
    elif [[ "$TRIPLE" == *darwin* ]]; then
        strip -x "$OUT_DIR/$BIN_NAME" 2>/dev/null || true
    else
        strip "$OUT_DIR/$BIN_NAME" 2>/dev/null || true
    fi
fi
info "Binary staged: $BIN_NAME ($(du -h "$OUT_DIR/$BIN_NAME" | cut -f1))"

# Copy assets/
if [ "$NO_ASSETS" -eq 0 ] && [ -d "$PROJECT_DIR/assets" ]; then
    info "Copying assets/ → $OUT_DIR/assets/"
    rsync -a --delete "$PROJECT_DIR/assets/" "$OUT_DIR/assets/"
else
    [ "$NO_ASSETS" -eq 0 ] && warn "No assets/ directory found at $PROJECT_DIR/assets"
fi

# Copy docs/ + AGENTS.md so end users have the architecture docs, tutorials,
# and agent guidelines alongside the binary. ~1.2 MB — trivial copy.
if [ "$NO_ASSETS" -eq 0 ]; then
    if [ -d "$PROJECT_DIR/docs" ]; then
        info "Copying docs/ → $OUT_DIR/docs/"
        rsync -a --delete \
            --exclude 'numeric-experiments' \
            "$PROJECT_DIR/docs/" "$OUT_DIR/docs/"
    else
        warn "No docs/ directory found at $PROJECT_DIR/docs"
    fi
    if [ -f "$PROJECT_DIR/AGENTS.md" ]; then
        cp -f "$PROJECT_DIR/AGENTS.md" "$OUT_DIR/AGENTS.md"
        info "Copied AGENTS.md → $OUT_DIR/"
    fi
fi

# Copy .cache/ subdirs
if [ "$NO_CACHE" -eq 0 ]; then
    CACHE_SRC="$(resolve_cache_dir)"
    if [ -n "$CACHE_SRC" ] && [ -d "$CACHE_SRC" ]; then
        if [ "$FULL_CACHE" -eq 1 ]; then
            info "Bundling full .cache/ → $OUT_DIR/.cache/"
            rsync -a --delete "$CACHE_SRC/" "$OUT_DIR/.cache/"
        else
            SUBDIRS="$(cache_subdirs_for "$BINARY")"
            for subdir in $SUBDIRS; do
                if [ -d "$CACHE_SRC/$subdir" ]; then
                    mkdir -p "$OUT_DIR/.cache"
                    info "Bundling .cache/$subdir → $OUT_DIR/.cache/$subdir/ ($(du -sh "$CACHE_SRC/$subdir" | cut -f1))"
                    rsync -a "$CACHE_SRC/$subdir/" "$OUT_DIR/.cache/$subdir/"
                else
                    warn ".cache/$subdir not found at $CACHE_SRC — $BINARY may need it at runtime"
                    warn "  Populate with: cargo run -p lunco-assets -- download -p <crate>"
                fi
            done
        fi
    else
        warn "No .cache/ directory found — app will use OS cache dir at runtime"
        warn "  Populate with: cargo run -p lunco-assets -- download"
    fi
else
    info "Skipping .cache/ bundle (--no-cache)"
fi

# Write launcher script
if is_windows "$TRIPLE"; then
    write_launcher_windows "$OUT_DIR" "$BINARY"
else
    write_launcher_unix "$OUT_DIR" "$BINARY"
fi
info "Launcher: $OUT_DIR/run.$(is_windows "$TRIPLE" && echo bat || echo sh)"

# Write README
write_readme "$OUT_DIR" "$BINARY" "$TRIPLE" "$ARCH"

# Summary
TOTAL_SIZE=$(du -sh "$OUT_DIR" | cut -f1)
success "Package assembled: $OUT_DIR ($TOTAL_SIZE)"
info "Contents:"
( cd "$OUT_DIR" && ls -la ) | while read -r line; do info "  $line"; done

if [ "$RELEASE" -eq 0 ]; then
    warn "Dev build (opt-level 1). For distribution use --release."
fi

info "Run:  $OUT_DIR/run.$(is_windows "$TRIPLE" && echo bat || echo sh)"

# ── Archive ───────────────────────────────────────────────────────────────
if [ "$PACKAGE" -eq 1 ]; then
    create_archive "$OUT_DIR" "$TRIPLE"
fi

success "Done."
