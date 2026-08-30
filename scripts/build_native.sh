#!/usr/bin/env bash
# ============================================================================
# LunCoSim — native desktop build + package assembler
# ============================================================================
# Builds a LunCoSim desktop binary (lunica or luncosim) for the
# host platform (Linux, macOS, Windows) and assembles a self-contained
# distributable directory containing the binary, the assets/ tree with the
# relevant cache subdirs (fonts, MSL, models, …) packed INSIDE it, and a
# launcher script.
#
# The packed cache lives at assets/.cache/ because that is the second root the
# `lunco://` resolver reads (assets/ → assets/.cache/ → the machine-wide cache
# — see lunco_assets::cache_roots). A package therefore carries its own payload
# and resolves it with no environment variable and no network: running the
# binary directly works exactly as running the launcher does.
#
# Usage:
#     ./scripts/build_native.sh <binary> [--release] [--package|--velopack] [options]
#
# Binaries:
#     lunica      — Modelica Workbench IDE (desktop GUI)
#     luncosim    — LunCoSim desktop GUI
#
# Options:
#     --release          Optimized release build (default: dev)
#     --package          Create a .tar.gz (unix) or .zip (windows) archive
#     --velopack         Create a Velopack release from the staged directory
#     --version <semver>  SemVer2 version used by Velopack (or env override)
#     --target <triple>  Cross-compile target (default: host triple)
#     --no-cache         Skip bundling assets/.cache/ subdirs (binary + assets only)
#     --skip-download    Skip the cache asset download step (use existing global cache)
#     --no-assets        Skip bundling the assets/ tree
#     --out <dir>        Output directory (default: dist/<binary>-<platform>-<arch>/)
#     --extra <args>     Pass extra args to cargo build
#
# Examples:
#     ./scripts/build_native.sh lunica --release --package
#     ./scripts/build_native.sh luncosim --release --package
#     ./scripts/build_native.sh lunica                    # quick dev build
#     ./scripts/build_native.sh luncosim --target aarch64-unknown-linux-gnu
#
# Platform detection is automatic. The script works on:
#   Linux   (x86_64 / aarch64)  — needs libasound2, libudev, libwayland, libxkbcommon
#   macOS   (x86_64 / arm64)    — needs nothing extra (Metal is built-in)
#   Windows (x86_64)            — needs MSVC build tools (Visual Studio)
#
# The script downloads the manifest-declared package assets before staging:
#   cargo run -p lunco-assets -- download --bundle lunica-native
#
# The package layout:
#   dist/<binary>-<platform>-<arch>/
#     <binary>[.exe]          — the compiled binary
#     assets/                  — scene files, config, models, shaders
#     assets/.cache/           — fonts, MSL, models, ephemeris (what each binary needs)
#     run.sh / run.bat         — launcher
#     README.md                — quick-start for end users
# ============================================================================

set -euo pipefail

if [[ -t 1 ]]; then
    RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; BLUE=''; NC=''
fi
info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=cache_dir.sh
source "$SCRIPT_DIR/cache_dir.sh"

usage() {
    sed -n '2,56p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

# ── Map binary → cargo crate ──────────────────────────────────────────────
get_crate() {
    case "$1" in
        lunica)   echo "lunco-modelica" ;;
        luncosim) echo "lunco-luncosim" ;;
        *) error "Unknown binary: $1"; error "Available: lunica, luncosim"; exit 1 ;;
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

velopack_runtime() {
    case "$1" in
        *windows*)
            case "$1" in
                *aarch64*) echo "win-arm64" ;;
                *) echo "win-x64" ;;
            esac
            ;;
        *darwin*)
            case "$1" in
                *x86_64*) echo "osx-x64" ;;
                *) echo "osx-arm64" ;;
            esac
            ;;
        *linux*)
            case "$1" in
                *aarch64*) echo "linux-arm64" ;;
                *) echo "linux-x64" ;;
            esac
            ;;
        *) error "No Velopack runtime mapping for target '$1'"; exit 1 ;;
    esac
}

# ── Portable directory sync ───────────────────────────────────────────────
# Desktop bundles must be reproducible on Linux, macOS and Git Bash. Git
# Bash's `cp` rejects the POSIX `source/.` spelling used by the former unified
# branch (it exits with `Invalid Parameter - none` on the Windows runner), so
# retain the native rsync path and use a Bash glob fallback there instead.
#
#   sync_dir <src-with-trailing-slash> <dest-with-trailing-slash>
#   sync_dir <src-with-trailing-slash> <dest-with-trailing-slash> no-delete
#
# The trailing-slash convention matches rsync: "src/" = contents-of-src.
#
# NEVER copies per-session runtime state, whatever the source tree. `assets/` is
# itself an open twin, so an ordinary dev run writes `.lunco/runtime/<scene>.usda`
# (live spawns + moved transforms) and `history/` (the edit journal) inside the
# very tree we package. A `.gitignore` entry stops those reaching git; it does
# NOT stop them reaching a bundle, because packaging copies the WORKING TREE —
# so a nightly cut on a machine that had ever driven a rover shipped that
# session, layered over the authored scene on every install. The rule is
# `lunco_twin::is_runtime_state`; keep the two in step.
sync_dir() {
    local src="$1" dest="$2" no_delete="${3:-}"
    if command -v rsync >/dev/null 2>&1; then
        if [ "$no_delete" = "no-delete" ]; then
            rsync -a --exclude='.lunco/' --exclude='history/' "$src" "$dest"
        else
            rsync -a --delete --exclude='.lunco/' --exclude='history/' "$src" "$dest"
        fi
        return
    fi

    if [ "$no_delete" != "no-delete" ]; then
        rm -rf "${dest:?}"
    fi
    mkdir -p "$dest"

    # `dotglob` makes `"$src"*` mean the contents of src, including hidden
    # files, without the Windows-hostile `source/.` argument. Keep it scoped to
    # a subshell so callers do not inherit a changed globbing policy.
    if ! (
        shopt -s dotglob nullglob
        entries=("${src%/}"/*)
        ((${#entries[@]}))
        cp -R "${entries[@]}" "$dest"
    ); then
        echo "ERROR: failed to copy '$src' → '$dest'." >&2
        echo "       The package would ship an incomplete assets/ tree." >&2
        return 1
    fi

    # The fallback cannot exclude source paths; prune session state after its
    # successful copy, exactly matching rsync's exclusion rule.
    find "$dest" -type d \( -name '.lunco' -o -name 'history' \) -prune -exec rm -rf {} + 2>/dev/null || true
}

# ── Download cache assets for a binary ────────────────────────────────────
# Runs the manifest-owned bundle target for the binary. Idempotent — re-running
# with a populated cache is a no-op because the tool verifies each declaration.
#
# Skipped when --no-cache or --skip-download is set. Downloads land in
# LUNCOSIM_CACHE (or the resolved cache dir).
download_cache_for() {
    local binary="$1"
    local cache_dir
    cache_dir="$(resolve_cache_dir)"
    mkdir -p "$cache_dir"
    if [ -z "$cache_dir" ]; then
        warn "Unable to resolve the OS-global cache — cannot download assets. Set LUNCOSIM_CACHE."
        return 0
    fi
    info "Downloading cache assets for $binary → $cache_dir"

    # Export so the lunco-assets binary picks it up.
    export LUNCOSIM_CACHE="$cache_dir"
    local target="${binary}-native"
    info "  downloading manifest bundle '$target' …"
    if cargo run -q -p lunco-assets -- download --bundle "$target"; then
        success "  downloaded manifest bundle '$target'"
    else
        error "  download for manifest bundle '$target' failed"
        return 1
    fi
}

# ── Write the launcher script ─────────────────────────────────────────────
write_launcher_unix() {
    local dir="$1" binary="$2"
    local launcher="$dir/run.sh"
    cat > "$launcher" <<EOF
#!/usr/bin/env bash
# Launcher for $binary. It only enters the package directory: the bundled data
# lives in assets/.cache/, which the app resolves on its own, so running the
# binary directly behaves identically to running this script.
cd "\$(dirname "\$0")"
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
$binary.exe %*
EOF
}

# Stage the canonical SVG sources and the platform-native icon generated by the
# Rust build script. The SVG remains the source of truth; the native output is
# required by the actual package consumer (PE resources, Finder app bundles, or
# AppImage/AppDir). A package must fail closed if that output is missing.
stage_app_icons() {
    local dir="$1" binary="$2" platform="$3" native_icon="$4"
    [ "$binary" = "luncosim" ] || return 0
    local source="$PROJECT_DIR/assets/icons"
    if [ ! -d "$source" ]; then
        error "No luncosim icon source at $source"
        return 1
    fi
    if [ ! -f "$source/svg/lcs-night-linux.svg" ]; then
        error "No canonical Linux LunCoSim SVG at $source/svg/lcs-night-linux.svg"
        return 1
    fi
    if [ ! -f "$native_icon" ]; then
        error "No platform-native LunCoSim icon at $native_icon"
        return 1
    fi
    mkdir -p "$dir/icons/svg"
    sync_dir "$source/svg/" "$dir/icons/svg/"
    mkdir -p "$dir/icons/native"
    cp -f "$native_icon" "$dir/icons/native/"
    case "$platform" in
        *linux*)
            mkdir -p "$dir/icons/hicolor/scalable/apps"
            cp -f "$source/svg/lcs-night-linux.svg" \
                "$dir/icons/hicolor/scalable/apps/luncosim.svg"
            if [ -d "$(dirname "$native_icon")/hicolor" ]; then
                sync_dir "$(dirname "$native_icon")/hicolor/" "$dir/icons/hicolor/"
            fi
            cat > "$dir/LunCoSim.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=LunCoSim
Comment=LunCoSim lunar simulation
Exec=./run.sh
Icon=luncosim
Terminal=false
Categories=Science;Education;
EOF
            ;;
    esac
    info "LunCoSim app identity/icons staged for $platform"
}

# Turn the macOS iconset emitted by the Rust build script into the ICNS file
# that Velopack requires when it creates the .app bundle. The macOS GitHub
# runner provides iconutil for both the native arm64 and cross-built x86_64
# packages.
prepare_package_icon() {
    PACKAGE_ICON=""
    [ "$BINARY" = "luncosim" ] || return 0
    case "$PLATFORM" in
        windows)
            PACKAGE_ICON="$ICON_OUTPUT_DIR/luncosim.ico"
            ;;
        macos)
            local iconset="$ICON_OUTPUT_DIR/macos/luncosim.iconset"
            PACKAGE_ICON="$ICON_OUTPUT_DIR/macos/luncosim.icns"
            if ! command -v iconutil >/dev/null 2>&1; then
                error "macOS package icon requires iconutil"
                return 1
            fi
            iconutil -c icns "$iconset" -o "$PACKAGE_ICON"
            ;;
        linux)
            PACKAGE_ICON="$ICON_OUTPUT_DIR/linux/luncosim.png"
            ;;
        *)
            error "No native icon packaging contract for target '$TRIPLE'"
            return 1
            ;;
    esac
    if [ ! -f "$PACKAGE_ICON" ]; then
        error "Icon generation did not produce $PACKAGE_ICON"
        return 1
    fi
}

# Inspect the completed Linux AppImage, not only the pre-VPK staging
# directory. Velopack owns the AppImage root desktop entry and icon; this
# check ensures those generated files retain the fixed luncosim identity and
# AppImage discovery contract.
verify_linux_appimage() {
    local appimage_dir="$1"
    local appimage
    appimage="$(find "$appimage_dir" -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
    if [ -z "$appimage" ]; then
        error "Velopack did not create a Linux AppImage in $appimage_dir"
        return 1
    fi
    bash "$SCRIPT_DIR/verify_linux_appimage.sh" "$appimage"
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
- \`assets/.cache/\` — fonts and runtime data (MSL, models, ephemeris as needed)
- \`docs/\` — architecture docs, tutorials, app guides
- \`skills/\` — project-level agent skills (runbook workflows)
- \`AGENTS.md\` — AI agent guidelines for working on the codebase
- \`icons/\` — native artwork and Linux hicolor icons used by the package
- \`run.sh\` / \`run.bat\` — launcher that sets the cache path

## Updates

Official Velopack releases check for updates once when the GUI starts. When a
new version is available, the red status-bar notice offers **Download update**
and reports its percentage. When ready, click **Install and restart**. The same
actions are available in Settings ▸ Updates. Use the official installer
for your platform: Windows uses \`LunCoSim-Windows-x86_64-Setup.exe\`, macOS uses
the matching Apple Silicon or Intel \`.pkg\`, and Linux uses the
\`LunCoSim-Linux-x86_64.AppImage\`. On Linux, make the AppImage executable,
keep it in a writable location, and keep launching that same AppImage so Velopack
can replace it in place. On Windows and macOS, keep launching the installed
shortcut or \`LunCoSim.app\`. Source builds, \`target/debug\` binaries, and
ordinary archive packages are not update-managed.

## Documentation

See \`docs/\` for:
- \`README.md\` — reading order for newcomers
- \`crates-index.md\` — map of the ~50-crate workspace
- \`principles.md\` — non-negotiable design principles
- \`architecture/\` — numbered design docs (00s overview, 10s systems, etc.)
- \`tutorials/\` — user-facing tutorials

\`skills/\` contains project-level agent skills (runbooks for theming, UI,
API testing, etc.). See \`skills/README.md\` for the index.

\`AGENTS.md\` documents the project conventions for AI agents (Bevy 0.18,
plugin layering, tunability mandate, TDD-first).
EOF
    cat >> "$readme" <<EOF

## Cache directory

Bundled data lives in \`assets/.cache/\` and is found automatically — it is
the second place the app looks for any asset, after \`assets/\` itself. No
environment variable is involved, so running the binary directly is the same
as running the launcher.

Anything not bundled (and anything you download from Settings ▸ Downloadable
data) comes from the shared machine cache:
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
    # UTC makes archive names unambiguous and consistent between local and
    # GitHub Actions builds. CI supplies one timestamp to all matrix jobs so
    # the archive names match the nightly release tag exactly. Seconds prevent
    # repeat package runs overwriting one another while preserving the stable
    # platform/architecture prefix.
    local timestamp
    timestamp="${LUNCO_BUILD_TIMESTAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
    local archive
    if is_windows "$platform"; then
        archive="${dir}-${timestamp}.zip"
        info "Creating .zip archive: $archive"
        if command -v 7z &>/dev/null; then
            (cd "$parent" && 7z a -tzip "$base-$timestamp.zip" "$base")
        elif command -v zip &>/dev/null; then
            (cd "$parent" && zip -r "$base-$timestamp.zip" "$base")
        else
            (cd "$parent" && powershell -NoProfile -Command \
                "Compress-Archive -Force -Path '$base' -DestinationPath '$base-$timestamp.zip'")
        fi
    else
        archive="${dir}-${timestamp}.tar.gz"
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
VELOPACK=0
RELEASE_VERSION="${LUNCO_RELEASE_VERSION:-}"
TARGET=""
NO_CACHE=0
SKIP_DOWNLOAD=0
NO_ASSETS=0
OUT_DIR=""
VELOPACK_OUT=""
EXTRA_ARGS=()

while [ $# -gt 0 ]; do
    case "$1" in
        --release)       RELEASE=1; shift ;;
        --package)       PACKAGE=1; shift ;;
        --velopack)      VELOPACK=1; shift ;;
        --version)       RELEASE_VERSION="$2"; shift 2 ;;
        --version=*)     RELEASE_VERSION="${1#--version=}"; shift ;;
        --target)        TARGET="$2"; shift 2 ;;
        --target=*)      TARGET="${1#--target=}"; shift ;;
        --no-cache)      NO_CACHE=1; shift ;;
        --skip-download) SKIP_DOWNLOAD=1; shift ;;
        --no-assets)     NO_ASSETS=1; shift ;;
        --out)           OUT_DIR="$2"; shift 2 ;;
        --out=*)         OUT_DIR="${1#--out=}"; shift ;;
        --velopack-out)  VELOPACK_OUT="$2"; shift 2 ;;
        --velopack-out=*) VELOPACK_OUT="${1#--velopack-out=}"; shift ;;
        --extra)         EXTRA_ARGS+=("$2"); shift 2 ;;
        --extra=*)       EXTRA_ARGS+=("${1#--extra=}"); shift ;;
        -h|--help)       usage 0 ;;
        *)               error "Unknown option: $1"; usage 2 ;;
    esac
done

if [ "$PACKAGE" -eq 1 ] && [ "$VELOPACK" -eq 1 ]; then
    error "Choose either --package or --velopack, not both"
    exit 2
fi

# Validate binary
case "$BINARY" in
    lunica|luncosim) ;;
    *) error "Unknown binary: $BINARY"; error "Available: lunica, luncosim"; exit 1 ;;
esac

CRATE="$(get_crate "$BINARY")"
HOST_TRIPLE="$(detect_host_triple)"
TRIPLE="${TARGET:-$HOST_TRIPLE}"
PLATFORM="$(platform_short "$TRIPLE")"
ARCH="$(arch_short "$TRIPLE")"

# Keep one application identity across local windows, desktop entries, and
# Velopack packages. Runtime-specific channels and package filenames still
# separate each updater target.
VPK_RUNTIME=""
if [ "$VELOPACK" -eq 1 ]; then
    VPK_RUNTIME="$(velopack_runtime "$TRIPLE")"
fi

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

if [ -z "$OUT_DIR" ]; then
    OUT_DIR="$PROJECT_DIR/dist/${BINARY}-${PLATFORM}-${ARCH}"
fi
# This is a repository-local build output, not a managed temporary directory.
# Keeping it outside dist/ lets the package staging directory be recreated
# after Cargo has emitted the icons into it.
ICON_OUTPUT_DIR="$PROJECT_DIR/target/package-icons/${BINARY}-${PLATFORM}-${ARCH}"
mkdir -p "$ICON_OUTPUT_DIR"
# Cargo otherwise considers the build script complete even if a previous
# package invocation removed its external icon output. Change this observed
# environment value per package build so the generator runs and repopulates
# the contract's output directory.
ICON_OUTPUT_STAMP="$(date +%s)-$$"

# Add .exe for Windows targets
if is_windows "$TRIPLE"; then
    BIN_PATH="${BIN_PATH}.exe"
fi

info "Building $BINARY ($CRATE) — $PROFILE_LABEL, target: $TRIPLE"
cd "$PROJECT_DIR"

    LUNCOSIM_ICON_OUTPUT_DIR="$ICON_OUTPUT_DIR" \
    LUNCOSIM_ICON_OUTPUT_STAMP="$ICON_OUTPUT_STAMP" \
    cargo build "${PROFILE_ARGS[@]+"${PROFILE_ARGS[@]}"}" "${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}" -j 4 \
    --bin "$BINARY" -p "$CRATE" \
    "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}"

if [ ! -f "$PROJECT_DIR/$BIN_PATH" ]; then
    error "Build succeeded but binary not found at $BIN_PATH"
    exit 1
fi
success "Binary built: $BIN_PATH ($(du -h "$PROJECT_DIR/$BIN_PATH" | cut -f1))"

prepare_package_icon

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
    sync_dir "$PROJECT_DIR/assets/" "$OUT_DIR/assets/"
else
    [ "$NO_ASSETS" -eq 0 ] && warn "No assets/ directory found at $PROJECT_DIR/assets"
fi

stage_app_icons "$OUT_DIR" "$BINARY" "$TRIPLE" "$PACKAGE_ICON"

# Copy docs/, skills/, + AGENTS.md so end users have the architecture docs,
# tutorials, skills (project-level agent skills), and agent guidelines
# alongside the binary. ~1.4 MB total — trivial copy.
if [ "$NO_ASSETS" -eq 0 ]; then
    if [ -d "$PROJECT_DIR/docs" ]; then
        info "Copying docs/ → $OUT_DIR/docs/"
        sync_dir "$PROJECT_DIR/docs/" "$OUT_DIR/docs/"
    else
        warn "No docs/ directory found at $PROJECT_DIR/docs"
    fi
    if [ -d "$PROJECT_DIR/skills" ]; then
        info "Copying skills/ → $OUT_DIR/skills/"
        sync_dir "$PROJECT_DIR/skills/" "$OUT_DIR/skills/"
    else
        warn "No skills/ directory found at $PROJECT_DIR/skills"
    fi
    if [ -f "$PROJECT_DIR/AGENTS.md" ]; then
        cp -f "$PROJECT_DIR/AGENTS.md" "$OUT_DIR/AGENTS.md"
        info "Copied AGENTS.md → $OUT_DIR/"
    fi
fi

# Stage manifest-declared package artifacts into assets/.cache/ — the resolver's
# second root, so a package finds its own payload without LUNCOSIM_CACHE and
# without a launcher. The manifest decides what belongs to this binary; user
# datasets such as Earth/Moon imagery have no bundle target and cannot enter.
PACKED_CACHE="$OUT_DIR/assets/.cache"
if [ "$NO_CACHE" -eq 0 ]; then
    CACHE_SRC="$(resolve_cache_dir)"
    if [ -n "$CACHE_SRC" ] && [ -d "$CACHE_SRC" ]; then
        info "Staging manifest-declared artifacts → $PACKED_CACHE/"
        cargo run -q -p lunco-assets -- stage \
            --binary "${BINARY}-native" \
            --cache "$CACHE_SRC" \
            --destination "$PACKED_CACHE"
    else
        error "No cache directory found — required bundle artifacts cannot be staged"
        exit 1
    fi
else
    info "Skipping packed cache (--no-cache)"
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

# ── Velopack release or legacy-free direct archive ────────────────────────
if [ "$VELOPACK" -eq 1 ]; then
    if [ -z "$RELEASE_VERSION" ]; then
        error "--velopack requires --version <SemVer2> or LUNCO_RELEASE_VERSION"
        exit 2
    fi
    if ! command -v vpk >/dev/null 2>&1; then
        error "Velopack packaging requested but 'vpk' is not installed"
        error "Install the pinned Velopack CLI with: dotnet tool install -g vpk --version 1.2.110-ge826545"
        exit 1
    fi
    if [ -z "$VELOPACK_OUT" ]; then
        VELOPACK_OUT="$PROJECT_DIR/dist/velopack-${PLATFORM}-${ARCH}"
    fi
    rm -rf "$VELOPACK_OUT"
    mkdir -p "$VELOPACK_OUT"
    VPK_PACK_ID="luncosim"
    # Keep one feed per runtime so a client can never select another
    # architecture's full package. Velopack channels accept this slug format.
    VPK_CHANNEL="$VPK_RUNTIME"
    VPK_ICON_ARGS=()
    if [ -n "$PACKAGE_ICON" ]; then
        VPK_ICON_ARGS=(--icon "$PACKAGE_ICON")
    fi
    info "Creating Velopack release: version $RELEASE_VERSION, runtime $VPK_RUNTIME, channel $VPK_CHANNEL"
    vpk pack \
        --packId "$VPK_PACK_ID" \
        --packTitle LunCoSim \
        --packVersion "$RELEASE_VERSION" \
        --channel "$VPK_CHANNEL" \
        --runtime "$VPK_RUNTIME" \
        --packDir "$OUT_DIR" \
        --mainExe "$BIN_NAME" \
        "${VPK_ICON_ARGS[@]}" \
        --outputDir "$VELOPACK_OUT"
    if ! find "$VELOPACK_OUT" -maxdepth 1 -type f -name "releases.$VPK_CHANNEL.json" -print -quit | grep -q .; then
        error "Velopack did not create releases.$VPK_CHANNEL.json in $VELOPACK_OUT"
        exit 1
    fi
    if [ "$PLATFORM" = "linux" ] && [ "$BINARY" = "luncosim" ]; then
        verify_linux_appimage "$VELOPACK_OUT"
    fi
    success "Velopack release assembled: $VELOPACK_OUT"
elif [ "$PACKAGE" -eq 1 ]; then
    create_archive "$OUT_DIR" "$TRIPLE"
fi

success "Done."
