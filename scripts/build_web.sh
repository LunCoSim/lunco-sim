#!/bin/bash
# ============================================================================
# LunCoSim - Web Build Script
# ============================================================================
# Builds LunCoSim applications for WebAssembly and serves them locally
# Usage: ./scripts/build_web.sh [command] [binary]
#
# Commands:
#   build <binary>    Build WASM and generate bindings
#   serve <binary>    Start web server (requires built files)
#   all <binary>      Build and serve
#   clean             Remove build artifacts
#   help              Show this help message
#
# Profile: default is a fast dev build (no wasm-opt). Pass --release for
#   a shippable build (fat LTO + wasm-opt size pass).
#
# Available binaries:
#   lunica   - Modelica Workbench IDE
#   sandbox  - Simulation Sandbox (ground physics)
#   luncosim - Full lunar-mission simulator (celestial + orbital). No Modelica
#              worker / MSL bundle (not a Modelica IDE). Textures load over HTTP
#              (built without `celestial` embed-assets).
# ============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Print colored message
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Get binary config
get_binary_config() {
    local binary="$1"
    case "$binary" in
        lunica)
            echo "lunco-modelica"
            ;;
        sandbox)
            echo "lunco-sandbox"
            ;;
        luncosim)
            echo "luncosim"
            ;;
        *)
            error "Unknown binary: $binary"
            error "Available binaries: lunica, sandbox, luncosim"
            exit 1
            ;;
    esac
}

# The web-target name IS the cargo bin name — both `lunica` and `sandbox`
# are single cfg-gated sources that compile to desktop and wasm via
# `#[cfg(target_arch = "wasm32")]`. No `_web` aliases, no `--out-name`
# rename. Kept as a function so the build flow has one obvious seam if a
# future target ever does need a different cargo bin name.
get_cargo_bin_name() {
    echo "$1"
}

# Pack one Twin folder next to the wasm and echo its scenes.json entry.
#
#   $1 src   — path to the Twin folder (contains the scene .usda + assets)
#   $2 name  — dist folder name under assets/twins/  (URL-safe)
#   $3 scene — scene .usda filename within the twin
#   $4 dist  — dist root (…/dist/sandbox)
#
# Copies src → dist/assets/twins/<name>/ and prints a JSON object
# {"name":…,"path":"twins/<name>/<scene>"} on stdout (empty on failure) so
# stage_twins can assemble the scene list. LoadScene resolves `twins/…`
# against the same-origin `assets/` tree, so the path is asset-relative.
stage_one_twin() {
    local src="$1" name="$2" scene="$3" dist="$4"
    if [ ! -d "$src" ]; then
        warn "twin source '$src' not found — skipping (scene '$name' will be absent)" >&2
        return 0
    fi
    if [ ! -f "$src/$scene" ]; then
        warn "twin '$src' has no scene '$scene' — skipping" >&2
        return 0
    fi
    local dest="$dist/assets/twins/$name"
    mkdir -p "$dest"
    # --delete keeps re-runs clean; exclude the twin's local runtime/history
    # scratch (not needed by clients, history/ holds transient .tmp files) and
    # the terrain `.cache/` raw source DTM (the baked heightmap.tif is what the
    # client fetches — the raw NAC DTM is ~2× larger and build-only).
    rsync -a --delete \
        --exclude '.lunco/' --exclude 'history/' --exclude '*.tmp' --exclude '.cache/' \
        "$src/" "$dest/" >&2
    info "Packed twin '$name' ($(du -sh "$dest" | cut -f1)) → $dest" >&2
    printf '{"name":"%s","path":"twins/%s/%s"}' "$name" "$name" "$scene"
}

# Pack the sandbox's Twin(s) and write dist/scenes.json (read by the
# index.html autoloader). See the LC_TWIN_* env docs at the call site.
stage_twins() {
    local dist="$1"
    # Prune the whole twins root first, so a twin baked by a PREVIOUS build (e.g.
    # an earlier `LC_TWIN_SRC=…/moonbase` run) does not survive into a build that
    # no longer stages it: `scenes.json` would omit it, but its ~90 MB of files
    # would still ship in the bundle. `rsync --delete` in `stage_one_twin` only
    # keeps an individually-staged twin clean; it can't remove one that isn't
    # staged this run at all. Everything under here is re-created below.
    rm -rf "$dist/assets/twins"
    # Default: do NOT bake the moonbase — on web it ships from the server on
    # connect (see SERVER_TWIN_DELIVERY_DESIGN.md), so the deploy boots a small
    # demo scene. Set LC_TWIN_SRC=/path/to/twin to bake one in (server-host
    # build / offline testing).
    local default_src="${LC_TWIN_SRC:-}"
    local default_scene="${LC_TWIN_SCENE:-moonbase_scene.usda}"

    local entries="" default_path=""
    if [ -n "$default_src" ]; then
        local name="${LC_TWIN_NAME:-}"
        if [ -z "$name" ]; then
            name="$(basename "$default_src")"
            [ "$name" = "twin" ] && name="$(basename "$(dirname "$default_src")")"
        fi
        local entry
        entry="$(stage_one_twin "$default_src" "$name" "$default_scene" "$dist")"
        if [ -n "$entry" ]; then
            entries="$entry"
            default_path="twins/$name/$default_scene"
        fi
    fi

    # Extra (non-default) twins: LC_TWIN_EXTRA="name=scene=path;name2=scene2=path2"
    if [ -n "${LC_TWIN_EXTRA:-}" ]; then
        local IFS=';'
        for spec in $LC_TWIN_EXTRA; do
            [ -n "$spec" ] || continue
            local ename="${spec%%=*}" rest="${spec#*=}"
            local escene="${rest%%=*}" esrc="${rest#*=}"
            local entry
            entry="$(stage_one_twin "$esrc" "$ename" "$escene" "$dist")"
            [ -n "$entry" ] && entries="${entries:+$entries,}$entry"
        done
    fi

    if [ -z "$entries" ]; then
        # No twin baked in → boot the lightweight demo scene (staged under
        # assets/scenes/sandbox/). The moonbase arrives from the server on connect.
        printf '{"default":"scenes/sandbox/sandbox_scene.usda","scenes":[]}\n' > "$dist/scenes.json"
        info "Wrote scenes.json (default: lightweight demo; moonbase via server)"
        return 0
    fi
    # Default is the first staged twin unless it failed (then no autoload key).
    if [ -n "$default_path" ]; then
        printf '{"default":"%s","scenes":[%s]}\n' "$default_path" "$entries" > "$dist/scenes.json"
        info "Wrote scenes.json (default: $default_path)"
    else
        printf '{"scenes":[%s]}\n' "$entries" > "$dist/scenes.json"
        info "Wrote scenes.json (no default — first twin failed to pack)"
    fi
}

# Check prerequisites
check_prerequisites() {
    info "Checking prerequisites..."
    
    # Check Rust
    if ! command -v rustc &> /dev/null; then
        error "Rust is not installed. Install from https://rustup.rs/"
        exit 1
    fi
    
    # Check wasm32 target
    if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
        warn "wasm32-unknown-unknown target not found. Installing..."
        rustup target add wasm32-unknown-unknown
        success "wasm32-unknown-unknown target installed"
    else
        success "wasm32-unknown-unknown target is installed"
    fi
    
    # Check wasm-bindgen CLI
    if ! command -v wasm-bindgen &> /dev/null; then
        warn "wasm-bindgen CLI not found. Installing..."
        cargo install wasm-bindgen-cli
        success "wasm-bindgen CLI installed"
    else
        success "wasm-bindgen CLI is installed"
    fi
    
    # Check HTTP server options
    if command -v http-server &> /dev/null; then
        HTTP_SERVER_CMD="http-server"
        success "http-server (Node.js) found"
    elif command -v python3 &> /dev/null; then
        HTTP_SERVER_CMD="python3"
        success "Python3 found"
    else
        warn "No HTTP server found. Install http-server: npm install -g http-server"
        HTTP_SERVER_CMD="python3"
    fi
}

# Should we rebuild the off-thread worker bundle?
#
# The worker bin pulls all of `lunco-modelica` (lib + UI), so most
# inner-loop UI edits invalidate it. But you DO want to skip it when
# only HTML/JS/asset/build-script files changed, or — crucially —
# when nothing under `crates/` changed since the last successful
# worker build (re-running `build_web.sh` on a clean tree shouldn't
# re-link 54 MB of wasm twice).
#
# Heuristic: rebuild iff any `.rs` or `Cargo.toml` under any watched
# source root is newer than the existing `lunica_worker.wasm` cargo output.
# `find -newer` is cheap (one stat per file). False positive on a
# changed third-party crate version is fine — that's a real rebuild.
# False negative on a manual `cargo clean` is mitigated by the
# `[ ! -f "$worker_wasm" ]` short-circuit below.
#
# Watched roots = this repo's `crates/` PLUS every local path-dependency
# checkout referenced by a `[patch]` in the root Cargo.toml. The patch roots
# are LOAD-BEARING: rumoca is consumed via `[patch] path = "../rumoca/..."`
# and its sources compile straight into the worker. A fix there (a solver /
# flatten change) lives OUTSIDE `crates/`, so scanning only `crates/` would
# silently skip the worker rebuild and leave it running stale rumoca — which
# is exactly how a fixed `rover.radiator.sigma` lowering "came back" in the
# browser while the main bundle had the fix. Each root's `target/` is pruned
# so the scan stays a cheap source-only stat sweep.
#
# Set `WORKER_REBUILD=force` to override (e.g. switching profiles).
should_rebuild_worker() {
    local worker_wasm="$1"
    if [ "${WORKER_REBUILD:-}" = "force" ]; then
        return 0
    fi
    if [ ! -f "$worker_wasm" ]; then
        return 0
    fi

    # Cargo.lock newer than the worker wasm = some dependency moved (a git-dep
    # rev bump, a version pin, a new transitive crate). The path-root scan below
    # only sees `crates/` + `[patch] path=` checkouts, so it CANNOT see a rumoca
    # bump consumed as a git dependency (`branch=main#<rev>`, living in
    # ~/.cargo/git). That's how the worker silently shipped stale rumoca: its
    # `StoredDefinition`/`WireMessage` bincode layout diverged from the freshly
    # rebuilt main bundle, so every postMessage mis-decoded (`UUID expected 16
    # found 9`, MSL "33 docs" instead of 2670). Gating on Cargo.lock catches the
    # whole class of dep bumps the source-mtime scan can't.
    if [ -f "$PROJECT_DIR/Cargo.lock" ] && [ "$PROJECT_DIR/Cargo.lock" -nt "$worker_wasm" ]; then
        return 0
    fi

    # Collect watch roots: crates/ + each [patch] checkout root (collapse
    # `.../crates/<crate>` path entries to their checkout, dedupe).
    local -a roots=("$PROJECT_DIR/crates")
    local rel root
    while IFS= read -r rel; do
        root=$(cd "$PROJECT_DIR" && realpath "${rel%%/crates/*}" 2>/dev/null) || continue
        [ -d "$root" ] && roots+=("$root")
    done < <(grep -oE 'path = "[^"]+"' "$PROJECT_DIR/Cargo.toml" 2>/dev/null \
                 | sed -E 's/.*path = "([^"]+)".*/\1/' | sort -u)

    # First source file (target/ pruned) newer than the worker wasm wins.
    local r newer
    for r in $(printf '%s\n' "${roots[@]}" | sort -u); do
        [ -d "$r" ] || continue
        newer=$(find "$r" -type d -name target -prune -o \
            \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$worker_wasm" -print -quit 2>/dev/null)
        [ -n "$newer" ] && return 0
    done
    return 1
}

# Wrap cargo with sccache when it's installed.
#
# sccache caches per-rustc-invocation across worktrees / branches /
# `cargo clean` cycles — biggest win on the cold rebuild that follows
# a dependency-version bump. Disables Cargo incremental (sccache and
# incremental fight; sccache is the better trade-off for our flow).
maybe_sccache_env() {
    if command -v sccache &> /dev/null; then
        export RUSTC_WRAPPER="${RUSTC_WRAPPER:-sccache}"
        export CARGO_INCREMENTAL=0
        info "sccache: enabled (RUSTC_WRAPPER=$RUSTC_WRAPPER, CARGO_INCREMENTAL=0)"
    else
        info "sccache: not installed — install with \`cargo install sccache\` for cross-worktree caching"
    fi
}

# Build the WASM binary
build_wasm() {
    local binary="$1"
    local crate="$2"

    # `BUILD_PROFILE` is exported by `main` once the CLI args are
    # parsed. Defaults to the fast web-dev profile (no fat LTO, parallel
    # codegen units, incremental); `--release` flips it to web-release
    # for a shippable build. The `wasm-opt` post-pass is skipped in dev
    # mode too — see `generate_bindings`.
    local profile="${BUILD_PROFILE:-web-dev}"
    info "Building $binary for WebAssembly..."
    info "Crate: $crate"
    info "Target: wasm32-unknown-unknown"
    info "Profile: $profile"

    maybe_sccache_env

    # We use --no-default-features to avoid pulling in the full tokio/axum stack
    # from lunco-api (the native `transport-http` server lives in the crate's
    # default features and needs mio — unsupported on wasm32). `--features
    # lunco-api` then opts the API crate back in *without* transport-http; on
    # wasm32 it auto-compiles the `window.lunco_api(...)` JS bridge instead (no
    # TcpListener) via `cfg(target_arch="wasm32")`.
    #
    # `--cfg=web_sys_unstable_apis` is REQUIRED for wgpu's WebGPU backend on
    # wasm (web-sys's `Gpu*` bindings are gated behind that flag). Without it
    # `navigator.gpu.requestAdapter()` silently returns null and bevy_render
    # panics with "Unable to find a GPU!" — even when the browser fully
    # supports WebGPU. egui's pipeline requires WebGPU here, so this flag is
    # mandatory, not optional.
    local cargo_bin
    cargo_bin=$(get_cargo_bin_name "$binary")
    # `ui` is REQUIRED for the web GUI builds: since the egui-ectomy refactor it
    # is a real cargo feature (egui/winit/workbench live behind it, no longer
    # unconditional deps). `--no-default-features` strips it, so we re-add it
    # explicitly — without it the wasm build links no window/egui (lunco_workbench
    # unresolved) and degrades to a headless server in the browser. luncosim is
    # the exception (it has no `ui` feature — egui is an unconditional dep there).
    #
    # sandbox carries the optional multiplayer wire (lightyear WebTransport,
    # client-only on wasm); lunica does not. Browser join is URL-driven
    # (`?connect=host#<digest>`), see `NetworkMode::from_url`.
    local wasm_features="lunco-api,ui"
    # luncosim has no `lunco-api` cargo feature (the API is an unconditional dep,
    # JS-bridge on wasm). Build it with NO features: celestial bodies load when
    # `sandbox` is off (the default), and we deliberately skip `celestial`
    # (embed-assets) on web — baking the Earth/Moon textures via `include_bytes!`
    # bloats the wasm and needs the asset cache; the browser loads them over HTTP.
    if [ "$binary" = "luncosim" ]; then
        wasm_features=""
    elif [ "$binary" = "sandbox" ]; then
        wasm_features="lunco-api,networking,ui"
        # Opt the client-prediction diagnostics into the browser build with
        # NET_DIAG=1 (off by default — same `net-diag` cargo feature as native).
        # Output lands in the browser console as `[net-diag …]` lines; mute a
        # given run with `LUNCO_NET_DIAG=0`. See lunco-networking/src/diagnostics.rs.
        if [ "${NET_DIAG:-0}" != "0" ]; then
            wasm_features="$wasm_features,net-diag"
            info "net-diag ENABLED for this web build (jitter/velocity/correction census → browser console)"
        fi
    fi
    # `getrandom_backend="wasm_js"`: ahash (via egui → catppuccin-egui →
    # lunco-theme) pulls getrandom 0.3 and lightyear 0.27's RNG (rand 0.10 in
    # lightyear_link) pulls getrandom 0.4 — both refuse to compile for
    # wasm32-unknown-unknown unless a backend is named. This cfg is version-
    # agnostic (covers 0.3 and 0.4 alike); the matching `wasm_js` *feature* is
    # enabled per-version on getrandom in the crates' wasm deps (lunco-sandbox /
    # lunco-networking). Both cfg and feature are required.
    # `${wasm_features:+--features ...}` omits the flag entirely when empty
    # (luncosim builds with no extra features).
    RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis --cfg=getrandom_backend=\"wasm_js\"" \
        cargo build --profile "$profile" --target wasm32-unknown-unknown --bin "$cargo_bin" -p "$crate" --no-default-features ${wasm_features:+--features "$wasm_features"}

    # Off-thread Modelica worker bundle. wasm32 has no real threads, so
    # without this every rumoca compile (a few seconds for non-trivial
    # models) blocks the render loop and the page appears frozen. Both
    # binaries that embed the Modelica workbench need it — lunica is
    # the workbench, sandbox embeds it as the Design workspace.
    case "$binary" in
        lunica|sandbox)
            local base_target_dir
            base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
            local worker_wasm="$base_target_dir/wasm32-unknown-unknown/$profile/lunica_worker.wasm"
            if should_rebuild_worker "$worker_wasm"; then
                info "Building companion worker bundle: lunica_worker"
                # Worker always builds out of lunco-modelica (that's where the
                # Modelica compile + step pipeline lives) regardless of which
                # main bundle is asking for it.
                RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis --cfg=getrandom_backend=\"wasm_js\"" \
                    cargo build --profile "$profile" --target wasm32-unknown-unknown --bin lunica_worker -p lunco-modelica --no-default-features
            else
                # See should_rebuild_worker rustdoc — finding "newer" .rs
                # under crates/ forces a rebuild even when the diff was in a
                # sibling script.
                info "Worker bundle up-to-date; skipping cargo build (set WORKER_REBUILD=force to override)"
            fi
            # Off-thread DEM bake worker (`dem_worker` in `lunco-terrain-bake`).
            # Same rationale as the Modelica worker: wasm32 has no threads, so the
            # ~40 MB GeoTIFF decode + crater stamp would freeze the page. Runs the
            # SAME `bake_grid` the native async task uses; streamed twin terrains
            # (moonbase) need it. Built for lunica+sandbox alongside lunica_worker.
            local dem_worker_wasm="$base_target_dir/wasm32-unknown-unknown/$profile/dem_worker.wasm"
            if should_rebuild_worker "$dem_worker_wasm"; then
                info "Building companion worker bundle: dem_worker"
                RUSTFLAGS="${RUSTFLAGS:-} --cfg=web_sys_unstable_apis --cfg=getrandom_backend=\"wasm_js\"" \
                    cargo build --profile "$profile" --target wasm32-unknown-unknown --bin dem_worker -p lunco-terrain-bake
            else
                info "DEM worker bundle up-to-date; skipping cargo build"
            fi
            ;;
    esac

    if [ $? -eq 0 ]; then
        success "WASM binary built successfully"
    else
        error "Build failed"
        exit 1
    fi
}

# Generate JavaScript bindings and assemble the shippable bundle.
#
# Layout (matches Rust/wasm conventions):
#   target/wasm32-unknown-unknown/release/<bin>.wasm   — cargo output
#   target/web/<bin>/                                  — wasm-bindgen output (intermediate)
#   dist/<bin>/                                        — final bundle served to browsers
#   crates/<crate>/web/index.html                      — source HTML template
generate_bindings() {
    local binary="$1"
    local crate="$2"
    # ONE shared index.html template for every app — lives with the rest
    # of the web library in crates/lunco-web/web/. Per-app differences are
    # filled in below by substituting __LC_BUNDLE__ / __LC_NAME__.
    local index_html="$PROJECT_DIR/crates/lunco-web/web/index.html"

    # Dynamically find the target directory in case it's overridden in .cargo/config.toml
    local base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
    local profile="${BUILD_PROFILE:-web-dev}"
    local cargo_out_dir="$base_target_dir/wasm32-unknown-unknown/$profile"
    local bindgen_out_dir="$base_target_dir/web/$binary"
    local dist_dir="$PROJECT_DIR/dist/$binary"

    info "Generating JavaScript bindings..."
    info "wasm-bindgen out: $bindgen_out_dir"
    info "Bundle dir:       $dist_dir"

    mkdir -p "$bindgen_out_dir" "$dist_dir"

    # Prefer local wasm-bindgen if it exists
    local wasm_bindgen_cmd="wasm-bindgen"
    if [ -f "$PROJECT_DIR/.cargo-bin/bin/wasm-bindgen" ]; then
        wasm_bindgen_cmd="$PROJECT_DIR/.cargo-bin/bin/wasm-bindgen"
        info "Using local wasm-bindgen: $wasm_bindgen_cmd"
    fi

    local cargo_bin
    cargo_bin=$(get_cargo_bin_name "$binary")
    # `--out-name "$binary"` normalises wasm-bindgen output to the
    # friendly name (e.g. `sandbox.js`, `sandbox_bg.wasm`) even
    # when the cargo binary is named differently (e.g. `sandbox`).
    # Downstream code (dist copy, index.html `import init from
    # './<binary>.js'`) keeps using `$binary` throughout.
    $wasm_bindgen_cmd "$cargo_out_dir/${cargo_bin}.wasm" \
        --out-dir "$bindgen_out_dir" \
        --target web \
        --out-name "$binary"

    if [ $? -ne 0 ]; then
        error "Binding generation failed"
        exit 1
    fi
    success "JavaScript bindings generated"

    # Best-effort wasm-opt pass. Typical 15–30 % size win on a release
    # wasm, which directly cuts download + streaming-compile time on
    # the page. Skipped (with a hint) if wasm-opt isn't on PATH so the
    # build still succeeds on machines that haven't installed
    # `binaryen`.
    local wasm_in="$bindgen_out_dir/${binary}_bg.wasm"
    # Skip the size pass entirely in dev mode — wasm-opt on a 28 MB
    # debug-ish wasm is ~20–30 s, which dominates inner-loop cycle
    # time. Bigger payload is fine for `localhost`.
    if [ "${BUILD_PROFILE:-web-dev}" = "web-dev" ]; then
        info "wasm-opt skipped (dev profile)"
    elif [ -f "$wasm_in" ] && command -v wasm-opt &> /dev/null; then
        info "Running wasm-opt -Oz --converge (max-size pass)…"
        local before
        before=$(stat -c '%s' "$wasm_in" 2>/dev/null || stat -f '%z' "$wasm_in")
        # -Oz = shrink-first (typically 10–25 % smaller than -O2).
        # --converge re-runs passes until no further size win — adds a
        # minute or two to release builds but is one-shot at deploy.
        local tmp="$wasm_in.opt.tmp"
        if wasm-opt -Oz --converge --strip-debug -o "$tmp" "$wasm_in"; then
            mv "$tmp" "$wasm_in"
            local after
            after=$(stat -c '%s' "$wasm_in" 2>/dev/null || stat -f '%z' "$wasm_in")
            info "wasm-opt: $(awk "BEGIN{printf \"%.1f\", $before/1048576}") MB → $(awk "BEGIN{printf \"%.1f\", $after/1048576}") MB"
        else
            warn "wasm-opt failed; keeping original output"
            rm -f "$tmp"
        fi
    elif [ -f "$wasm_in" ]; then
        info "wasm-opt not installed — skipping size pass (install \`binaryen\` for ~20% smaller wasm)"
    fi

    # Assemble the bundle: bindings + index.html in one place.
    # Use a fresh dist dir so stale files from a previous binary version
    # don't get served accidentally.
    #
    # Stash the worker subdir before the wipe so the worker-bindgen
    # skip path can restore it without re-running bindgen / wasm-opt.
    # Without this, the wipe deletes a still-up-to-date worker bundle,
    # the freshness check below sees a missing dist file, and the
    # skip can never fire.
    local stashed_worker=""
    if [ -d "$dist_dir/worker" ]; then
        stashed_worker=$(mktemp -d)
        cp -r "$dist_dir/worker"/. "$stashed_worker/"
    fi
    rm -rf "$dist_dir"
    mkdir -p "$dist_dir"
    # Recursive: wasm-bindgen emits a `snippets/` subdir (JS interop shims that
    # the generated loader imports at runtime). A non-recursive `cp` errors on
    # it under `set -e` and aborts the build before index.html / worker / msl
    # are staged — leaving a half-populated dist that serves a blank page.
    cp -r "$bindgen_out_dir"/. "$dist_dir/"
    if [ -n "$stashed_worker" ]; then
        mkdir -p "$dist_dir/worker"
        cp -r "$stashed_worker"/. "$dist_dir/worker/"
        rm -rf "$stashed_worker"
    fi
    if [ -f "$index_html" ]; then
        cp "$index_html" "$dist_dir/index.html"
        # Fill the shared template's per-app placeholders:
        #   __LC_BUNDLE__ → cargo bin / wasm-bindgen out-name (e.g. lunica)
        #   __LC_NAME__   → display name (bundle, first letter upper-cased)
        local app_name="$(tr '[:lower:]' '[:upper:]' <<< "${binary:0:1}")${binary:1}"
        sed -i "s/__LC_BUNDLE__/$binary/g; s|__LC_NAME__|$app_name|g" "$dist_dir/index.html"
        info "Filled template: bundle=$binary, name=$app_name"
        # Inject the actual uncompressed WASM size so the loading UI
        # can show accurate progress even when nginx serves a
        # pre-compressed .gz sibling (gzip_static on).
        local wasm_dist="$dist_dir/${binary}_bg.wasm"
        if [ -f "$wasm_dist" ]; then
            local wasm_bytes
            wasm_bytes=$(stat -c '%s' "$wasm_dist" 2>/dev/null || stat -f '%z' "$wasm_dist")
            sed -i "s/const __LC_WASM_SIZE__ = 0/const __LC_WASM_SIZE__ = $wasm_bytes/" "$dist_dir/index.html"
            info "Injected WASM size into index.html: $(awk "BEGIN{printf \"%.1f\", $wasm_bytes/1048576}") MB"
        fi
    else
        warn "No index.html found at $index_html — bundle will lack an entry point"
    fi

    # Shared web boot library — the streaming loader (lunco-boot.js) +
    # its styles (lunco-boot.css), maintained once in crates/lunco-web/.
    # Every app's index.html imports `./lunco-boot.js`, so copy them next
    # to the bundle. Missing = the page can't start, so warn loudly.
    local boot_src="$PROJECT_DIR/crates/lunco-web/web"
    if [ -f "$boot_src/lunco-boot.js" ] && [ -f "$boot_src/lunco-boot.css" ]; then
        cp "$boot_src/lunco-boot.js" "$boot_src/lunco-boot.css" "$dist_dir/"
        info "Copied lunco-boot.{js,css} → $dist_dir/"
    else
        warn "Missing crates/lunco-web/web/lunco-boot.{js,css} — page won't boot"
    fi

    # DejaVu Sans — wasm has no filesystem, lunco-theme fetches this
    # over HTTP at startup (see crates/lunco-theme/src/fonts.rs::
    # spawn_wasm_font_fetch). Source lives in the workspace cache
    # (populated by `cargo run -p lunco-assets -- download`); we just
    # copy it next to the wasm so it's served same-origin.
    local dejavu_src=""
    for candidate in \
        "$PROJECT_DIR/../.cache/fonts/DejaVuSans.ttf" \
        "$PROJECT_DIR/.cache/fonts/DejaVuSans.ttf"; do
        if [ -f "$candidate" ]; then dejavu_src="$candidate"; break; fi
    done
    if [ -z "$dejavu_src" ]; then
        info "DejaVu Sans font not found. Attempting to download automatically..."
        if LUNCOSIM_CACHE="$PROJECT_DIR/.cache" cargo run -p lunco-assets -- download -p lunco-theme; then
            for candidate in \
                "$PROJECT_DIR/../.cache/fonts/DejaVuSans.ttf" \
                "$PROJECT_DIR/.cache/fonts/DejaVuSans.ttf"; do
                if [ -f "$candidate" ]; then dejavu_src="$candidate"; break; fi
            done
        fi
    fi
    if [ -n "$dejavu_src" ]; then
        mkdir -p "$dist_dir/fonts"
        cp "$dejavu_src" "$dist_dir/fonts/DejaVuSans.ttf"
        info "Copied DejaVu Sans → $dist_dir/fonts/"
    else
        warn "DejaVu Sans not found — math/arrow glyphs will tofu in browser."
    fi

    # sandbox loads scene files via the bevy AssetServer over HTTP
    # (`assets/scenes/sandbox/sandbox_scene.usda` and friends). Copy the
    # workspace `assets/` tree next to the wasm so they're same-origin.
    # lunica doesn't need this — its models live in the MSL bundle.
    if [ "$binary" = "sandbox" ] && [ -d "$PROJECT_DIR/assets" ]; then
        info "Copying assets/ → $dist_dir/assets/"
        rsync -a --delete "$PROJECT_DIR/assets/" "$dist_dir/assets/"

        # The bundle ships its own file listing. The browser has no `readdir`, so
        # the spawn/shader catalogs cannot discover `*.usda`/`*.wgsl` by walking —
        # they fetch this manifest at boot (`lunco_assets::discovery`).
        #
        # It is generated HERE, from the tree we just staged, rather than baked
        # into the wasm by a `build.rs`. A baked listing describes the bundle the
        # binary was compiled against; this one describes the bundle that actually
        # shipped. They are the same thing right up until they aren't — swap an
        # asset into a deployed `dist/` and a baked listing never sees it.
        #
        # The listing is produced by `discovery::scan_library` — the SAME scanner
        # the native runtime walks the library with — so packaging cannot disagree
        # with the runtime about what counts as an asset. This used to be an inline
        # `os.walk`, a second implementation of that rule in another language, and
        # it had already drifted: it descended hidden directories, so it listed the
        # `.lunco/runtime/*.usda` private layers of any Twin staged above.
        info "Writing $dist_dir/assets/manifest.json"
        cargo run --release -q -p lunco-assets --bin build_asset_manifest -- \
            "$dist_dir/assets" || {
            # Without the manifest the browser cannot enumerate anything: the spawn
            # palette and the shader catalog come up empty. Fail the build rather
            # than ship a bundle whose assets are unreachable.
            error "failed to write assets/manifest.json — the web build would have an empty asset catalog"
            exit 1
        }
    fi

    # Include the deployment script in the dist bundle
    if [ -f "$PROJECT_DIR/scripts/copy_to_html_folder.sh" ]; then
        info "Copying copy_to_html_folder.sh → $dist_dir/"
        cp "$PROJECT_DIR/scripts/copy_to_html_folder.sh" "$dist_dir/"
    fi

    # luncosim renders Earth/Moon as celestial bodies; their PROCESSED textures
    # (`cached_textures://earth.png|moon.png`) load over HTTP same-origin —
    # `cache_dir()` resolves to ".cache" on wasm, so the bevy HTTP reader fetches
    # `<origin>/.cache/textures/<tex>`. Stage them next to the wasm (same idea as
    # the DejaVu font above). Populate the cache first with:
    #   cargo run -p lunco-assets -- download && cargo run -p lunco-assets -- process
    if [ "$binary" = "luncosim" ]; then
        for tex in earth.png moon.png; do
            local tex_src=""
            for candidate in \
                "$PROJECT_DIR/../.cache/textures/$tex" \
                "$PROJECT_DIR/.cache/textures/$tex"; do
                if [ -f "$candidate" ]; then tex_src="$candidate"; break; fi
            done
            if [ -n "$tex_src" ]; then
                mkdir -p "$dist_dir/.cache/textures"
                cp "$tex_src" "$dist_dir/.cache/textures/$tex"
                info "Copied $tex → $dist_dir/.cache/textures/"
            else
                warn "celestial texture $tex not found — that body renders untextured in \
the browser. Run: cargo run -p lunco-assets -- download && cargo run -p lunco-assets -- process"
            fi
        done
    fi

    # sandbox references glTF models via `lunco-lib://models/<name>.glb`, which
    # resolves to `<origin>/.cache/models/<name>.glb` on wasm (cache_dir() = ".cache").
    # Stage the PROCESSED models (e.g. NASA Perseverance) next to the wasm — same
    # idea as the luncosim textures above. Populate the cache first with:
    #   cargo run -p lunco-assets --bin lunco-assets -- download -a perseverance \
    #     && cargo run -p lunco-assets --bin lunco-assets -- process -p lunco-usd
    if [ "$binary" = "sandbox" ]; then
        local models_src=""
        for candidate in \
            "$PROJECT_DIR/../.cache/models" \
            "$PROJECT_DIR/.cache/models"; do
            if [ -d "$candidate" ]; then models_src="$candidate"; break; fi
        done
        if [ -n "$models_src" ]; then
            mkdir -p "$dist_dir/.cache/models"
            # Only the PROCESSED glbs the scene references (skip the raw *_source.glb).
            for glb in "$models_src"/*.glb; do
                [ -f "$glb" ] || continue
                case "$(basename "$glb")" in
                    *_source.glb) continue ;;
                esac
                cp "$glb" "$dist_dir/.cache/models/"
                info "Copied $(basename "$glb") → $dist_dir/.cache/models/"
            done
        else
            warn "no .cache/models — glTF models (Perseverance rover) will 404 in the browser. \
Run: cargo run -p lunco-assets --bin lunco-assets -- download -a perseverance && … -- process -p lunco-usd"
        fi
    fi

    # Pack the Twin(s) the sandbox should offer, and write scenes.json so the
    # index.html autoloader opens the default one on boot. Dynamic, not
    # compiled in — override the source or add scenes without rebuilding:
    #   LC_TWIN_SRC=/path/to/twin        default twin folder to pack
    #                                    (optional; default = none → lightweight
    #                                     demo, moonbase arrives via server)
    #   LC_TWIN_NAME=moonbase            dist name under assets/twins/
    #   LC_TWIN_SCENE=moonbase_scene.usda   scene file within the twin
    #   LC_TWIN_EXTRA="lab=lab.usda=/path/lab;…"  extra non-default scenes
    # Or edit dist/sandbox/scenes.json after the build.
    if [ "$binary" = "sandbox" ]; then
        stage_twins "$dist_dir"
    fi

    # Show output size
    WASM_SIZE=$(du -h "$dist_dir/${binary}_bg.wasm" | cut -f1)
    JS_SIZE=$(du -h "$dist_dir/${binary}.js" | cut -f1)
    info "Bundle sizes: WASM=${WASM_SIZE}, JS=${JS_SIZE}"
    info "Bundle ready: $dist_dir"

    # ── Worker bundle (lunica + sandbox) ──────────────────────
    # Generate bindings for the off-thread Modelica worker and place its
    # output under `dist/<bin>/worker/` so the main page can
    # `new Worker('./worker/worker_bootstrap.js', { type: 'module' })`. The
    # worker bundle is a SECOND wasm instance — it has its own memory and
    # state — and there is no way to share Rust globals or `Arc`s with it.
    case "$binary" in
        lunica|sandbox) staged_worker=1 ;;
        *) staged_worker=0 ;;
    esac
    if [ "$staged_worker" = "1" ]; then
        local worker_bin="lunica_worker"
        local worker_bindgen_dir="$base_target_dir/web/$worker_bin"
        local worker_dist_dir="$dist_dir/worker"
        local worker_wasm_src="$cargo_out_dir/${worker_bin}.wasm"
        # The dist worker wasm is content-hashed (`${worker_bin}_bg-<sha>.wasm`),
        # so discover it by glob rather than a fixed name.
        local worker_wasm_dist
        worker_wasm_dist=$(ls "$worker_dist_dir/${worker_bin}_bg"*.wasm 2>/dev/null | head -1)
        # Skip the bindgen + wasm-opt + copy work entirely if the
        # cargo output didn't move since the last dist build. Pairs
        # with the `should_rebuild_worker` cargo-build skip in
        # `build_wasm`. Set `WORKER_REBUILD=force` to override.
        #
        # The mtime test alone is NOT enough: `hash_worker_wasm` rewrites
        # the dist wasm (new mtime) on EVERY build, including this skip
        # path, so the dist file is routinely newer than the cargo output
        # even after a real recompile — the comparison then lies and the
        # stale hashed wasm survives. A git-dep rev bump (rumoca) recompiles
        # the worker but the bindgen-skip would keep shipping the OLD bundle
        # to dist → main/worker rumoca layout skew → bincode decode errors,
        # "33 docs" truncation. So ALSO refuse to skip when Cargo.lock moved
        # since the dist worker was built — same gate as should_rebuild_worker.
        local lock_moved=0
        if [ -f "$PROJECT_DIR/Cargo.lock" ] && [ -n "$worker_wasm_dist" ] \
            && [ "$PROJECT_DIR/Cargo.lock" -nt "$worker_wasm_dist" ]; then
            lock_moved=1
        fi
        # The src-wasm-vs-dist mtime test misses source-only edits: when the
        # cargo build above is itself skip-gated (or incremental leaves the
        # output mtime untouched), `worker_wasm_src` is NOT newer than the dist
        # wasm even though a `.rs` under crates/ changed — and the worker ships
        # STALE (observed: a run-loop fix in `experiments_runner.rs` never
        # reached the worker, so Fast Run kept emitting the old fixed sample
        # count). Reuse the source-aware `should_rebuild_worker` scan against
        # the DIST wasm so any newer source forces a re-bindgen.
        local worker_src_changed=0
        if should_rebuild_worker "$worker_wasm_dist"; then
            worker_src_changed=1
        fi
        # Authoritative staleness signal: does the dist worker already bake the
        # exact wire id of the freshly-staged MAIN bundle? The mtime/Cargo.lock
        # heuristics above race against content-hash rewrites (hash_worker_wasm
        # touches the dist mtime every build) and incremental builds that leave
        # the cargo output mtime untouched — the whole class of "stale worker
        # silently shipped" bugs (build_web history: Cargo.lock gate, source-edit
        # gate, …). The baked `LUNCO_WIRE_BUILD_ID` is the ground truth the
        # runtime boot handshake checks, so gate the skip on it directly: only
        # skip when the shipped worker provably matches main. If the id can't be
        # read (format drift) `worker_has_wire` stays 0 and we rebuild — safe.
        local main_wasm_dist="$dist_dir/${binary}_bg.wasm"
        local expected_wire_id="" worker_has_wire=0
        expected_wire_id=$(strings "$main_wasm_dist" 2>/dev/null \
            | grep -oE 'LUNCO_WIRE:[0-9a-f]{16}' | head -1 | sed 's/LUNCO_WIRE://')
        if [ -n "$expected_wire_id" ] && [ -n "$worker_wasm_dist" ] \
            && strings "$worker_wasm_dist" 2>/dev/null | grep -q "$expected_wire_id"; then
            worker_has_wire=1
        fi
        if [ "${WORKER_REBUILD:-}" != "force" ] \
            && [ "$lock_moved" != "1" ] \
            && [ "$worker_src_changed" != "1" ] \
            && [ "$worker_has_wire" = "1" ] \
            && [ -f "$worker_wasm_src" ] \
            && [ -n "$worker_wasm_dist" ] \
            && [ ! "$worker_wasm_src" -nt "$worker_wasm_dist" ]; then
            local worker_size
            worker_size=$(du -h "$worker_wasm_dist" | cut -f1)
            info "Worker bundle up-to-date ($worker_size, wire $expected_wire_id) — bindgen skipped"
            # Still ensure the dist wasm is content-hashed (idempotent) — a
            # bare-named worker left by an older build would otherwise persist.
            hash_worker_wasm "$worker_dist_dir" "$worker_bin"
            return 0
        fi
        info "Generating bindings for worker bundle: $worker_bin"
        mkdir -p "$worker_bindgen_dir" "$worker_dist_dir"
        $wasm_bindgen_cmd "$worker_wasm_src" \
            --out-dir "$worker_bindgen_dir" \
            --target web
        if [ $? -ne 0 ]; then
            error "Worker binding generation failed"
            exit 1
        fi
        # wasm-opt the worker too, same flags as the main bundle.
        local worker_wasm_in="$worker_bindgen_dir/${worker_bin}_bg.wasm"
        if [ "${BUILD_PROFILE:-web-dev}" = "web-dev" ]; then
            info "Worker wasm-opt skipped (dev profile)"
        elif [ -f "$worker_wasm_in" ] && command -v wasm-opt &> /dev/null; then
            local tmp="$worker_wasm_in.opt.tmp"
            if wasm-opt -Oz --converge --strip-debug -o "$tmp" "$worker_wasm_in"; then
                mv "$tmp" "$worker_wasm_in"
            else
                rm -f "$tmp"
            fi
        fi
        rm -rf "$worker_dist_dir"
        mkdir -p "$worker_dist_dir"
        # Recursive (see the main-bundle copy): wasm-bindgen emits a `snippets/`
        # subdir; a non-recursive cp aborts the build under `set -e`.
        cp -r "$worker_bindgen_dir"/. "$worker_dist_dir/"
        # Content-hash the multi-MB `_bg.wasm` (the aggressively-cached artifact)
        # and repoint the generated loader. The `.js` shims keep stable names so
        # the Rust `install_worker("./worker/worker_bootstrap.js")` URL is
        # unaffected, and their content changes per build so a conditional GET
        # refreshes them.
        hash_worker_wasm "$worker_dist_dir" "$worker_bin"
        # Web Worker entry shim. wasm-bindgen --target web exports `init`
        # but doesn't run it; this tiny module imports + calls it so the
        # `#[wasm_bindgen(start)]` worker entry actually fires.
        # The worker bootstrap shim always lives next to the worker
        # source (in `lunco-modelica`), regardless of which main bundle
        # is consuming it. Same file for lunica and sandbox.
        local worker_bootstrap="$PROJECT_DIR/crates/lunco-modelica/web/worker_bootstrap.js"
        if [ -f "$worker_bootstrap" ]; then
            cp "$worker_bootstrap" "$worker_dist_dir/worker_bootstrap.js"
        else
            warn "No worker_bootstrap.js at $worker_bootstrap — worker won't init"
        fi
        local worker_size
        worker_size=$(du -h "$worker_dist_dir/${worker_bin}_bg"*.wasm 2>/dev/null | cut -f1 | head -1)
        info "Worker bundle: $worker_size at $worker_dist_dir"
        # Hard lockstep guard: the worker just staged MUST bake the same wire id
        # as the main bundle, or the runtime handshake rejects it and Modelica
        # compile/run silently breaks. Fail the build here instead of shipping a
        # broken pair to `serve`/deploy.
        assert_wire_lockstep \
            "$dist_dir/${binary}_bg.wasm" \
            "$(ls "$worker_dist_dir/${worker_bin}_bg"*.wasm 2>/dev/null | head -1)" \
            "$binary"
    fi

    # ── DEM bake worker bundle (lunica + sandbox) ─────────────
    # A SECOND companion worker (independent of the Modelica one): the off-thread
    # DEM bake. Staged under `dist/<bin>/dem-worker/` so the page can
    # `new Worker('./dem-worker/dem_worker_bootstrap.js', { type: 'module' })`.
    # Its protocol is self-contained (bytes in → heightfield out) and built from
    # the same tree, so it needs no wire-id lockstep guard. Absent → the terrain
    # code falls back to the inline (main-thread) bake automatically.
    if [ "$staged_worker" = "1" ]; then
        local dw_bin="dem_worker"
        local dw_bindgen_dir="$base_target_dir/web/$dw_bin"
        local dw_dist_dir="$dist_dir/dem-worker"
        local dw_wasm_src="$cargo_out_dir/${dw_bin}.wasm"
        if [ -f "$dw_wasm_src" ]; then
            info "Generating bindings for worker bundle: $dw_bin"
            mkdir -p "$dw_bindgen_dir"
            $wasm_bindgen_cmd "$dw_wasm_src" --out-dir "$dw_bindgen_dir" --target web
            if [ $? -ne 0 ]; then
                error "DEM worker binding generation failed"
                exit 1
            fi
            local dw_wasm_in="$dw_bindgen_dir/${dw_bin}_bg.wasm"
            if [ "${BUILD_PROFILE:-web-dev}" != "web-dev" ] && [ -f "$dw_wasm_in" ] \
                && command -v wasm-opt &> /dev/null; then
                local tmp="$dw_wasm_in.opt.tmp"
                if wasm-opt -Oz --converge --strip-debug -o "$tmp" "$dw_wasm_in"; then
                    mv "$tmp" "$dw_wasm_in"
                else
                    rm -f "$tmp"
                fi
            fi
            rm -rf "$dw_dist_dir"
            mkdir -p "$dw_dist_dir"
            cp -r "$dw_bindgen_dir"/. "$dw_dist_dir/"
            local dw_bootstrap="$PROJECT_DIR/crates/lunco-terrain-bake/web/dem_worker_bootstrap.js"
            if [ -f "$dw_bootstrap" ]; then
                cp "$dw_bootstrap" "$dw_dist_dir/dem_worker_bootstrap.js"
            else
                warn "No dem_worker_bootstrap.js at $dw_bootstrap — DEM worker won't init"
            fi
            local dw_size
            dw_size=$(du -h "$dw_dist_dir/${dw_bin}_bg.wasm" 2>/dev/null | cut -f1)
            info "DEM worker bundle: $dw_size at $dw_dist_dir"
        else
            warn "dem_worker wasm not found at $dw_wasm_src — DEM offload disabled (inline fallback)"
        fi
    fi
}

# Pack MSL into a versioned, compressed bundle and place it next to the
# wasm under `dist/<bin>/msl/`. Same-origin so the runtime fetcher doesn't
# need CORS configuration. Both wasm bundles ship MSL — lunica because
# the workbench *is* the MSL editor, sandbox because its Design
# workspace embeds the same Modelica panels and they'd be empty without
# the standard library.
# Content-hash the worker wasm in dist so a rebuilt worker is never served
# stale from the browser cache. Idempotent: hashes a bare `${bin}_bg.wasm`
# (rename + repoint the generated loader) and no-ops once already hashed. Runs
# on BOTH the rebuild and up-to-date paths so the dist is always hashed.
# Hard guard against shipping a stale worker. The off-thread worker and the
# main bundle both bake `LUNCO_WIRE_BUILD_ID` (a hash of Cargo.lock + this
# crate's src/); if they disagree, the boot handshake in worker_transport
# reports "STALE WORKER" and every bincode message mis-decodes — Modelica
# compile/run is broken. The staleness heuristics in generate_bindings are
# best-effort; this is the backstop that makes a mismatched pair impossible to
# ship: it reads the id baked into each wasm and exits non-zero on mismatch.
# Degrades to a warning (never a false failure) when the id can't be extracted.
assert_wire_lockstep() {
    local main_wasm="$1" worker_wasm="$2" bin="${3:-lunica}"
    local id
    id=$(strings "$main_wasm" 2>/dev/null \
        | grep -oE 'LUNCO_WIRE:[0-9a-f]{16}' | head -1 | sed 's/LUNCO_WIRE://')
    if [ -z "$id" ]; then
        warn "wire-lockstep guard: could not read main wire id from $(basename "$main_wasm" 2>/dev/null) — skipping check"
        return 0
    fi
    if [ -z "$worker_wasm" ] || ! strings "$worker_wasm" 2>/dev/null | grep -q "$id"; then
        error "WIRE LOCKSTEP FAILED: shipped worker does not bake main wire id $id."
        error "  A stale worker would ship → runtime 'STALE WORKER', Modelica compile/run BROKEN."
        error "  Fix: WORKER_REBUILD=force ./scripts/build_web.sh build $bin"
        exit 1
    fi
    success "Wire lockstep OK — main + worker both bake $id"
}

hash_worker_wasm() {
    local dir="$1" bin="$2"
    local bare="$dir/${bin}_bg.wasm"
    # Already hashed (or no wasm) → nothing to do.
    [ -f "$bare" ] || return 0
    local js="$dir/${bin}.js"
    if [ ! -f "$js" ]; then
        warn "Worker hash skipped (no loader $js)"
        return 0
    fi
    local wsha hashed
    wsha=$(sha256sum "$bare" | cut -c1-16)
    hashed="${bin}_bg-${wsha}.wasm"
    mv "$bare" "$dir/$hashed"
    # wasm-bindgen --target web references the wasm by this literal filename
    # inside the loader; repoint every occurrence.
    sed -i "s/${bin}_bg\\.wasm/${bin}_bg-${wsha}.wasm/g" "$js"
    # Drop stale bare-named precompressed siblings; they'd never match the
    # hashed request and would otherwise linger.
    rm -f "$dir/${bin}_bg.wasm.br" "$dir/${bin}_bg.wasm.gz"
    info "Worker wasm content-hashed → $hashed"
}

# Regenerate `msl_index.json` (the palette's component metadata: icons, ports,
# params) so it covers the third-party libs we're about to bundle. The indexer
# auto-discovers extras under `cache_dir()` and writes the index next to the MSL
# source tree, where `build_msl_assets` then packs it.
#
# Slow (~30 s: it full-parses MSL + extras), so it only runs when extras are
# actually requested (`MSL_EXTRA_LIBS`) or explicitly forced (`MSL_REINDEX=force`).
# Default builds skip it and pack whatever index is already on disk.
build_msl_index() {
    local binary="$1"
    case "$binary" in
        lunica|sandbox) ;;
        *) return 0 ;;
    esac
    if [ -z "${MSL_EXTRA_LIBS:-}" ] && [ "${MSL_REINDEX:-}" != "force" ]; then
        return 0
    fi
    info "Reindexing MSL + extras → msl_index.json (set MSL_REINDEX=force to always run)..."
    cargo run --release -q -p lunco-modelica --bin msl_indexer -- -v
    if [ $? -ne 0 ]; then
        error "MSL indexing failed"
        exit 1
    fi
    success "MSL index regenerated (covers discovered third-party libs)"
}

build_msl_bundle() {
    local binary="$1"
    case "$binary" in
        lunica|sandbox) ;;
        *) return 0 ;;
    esac
    # Refresh the palette index first so bundled extras carry icons/ports.
    build_msl_index "$binary"
    local dist_dir="$PROJECT_DIR/dist/$binary"
    local msl_dir="$dist_dir/msl"

    # Skip the rumoca pre-parse + tar+zstd pass when nothing under
    # `.cache/msl/` is newer than the existing `manifest.json`. Pack
    # is content-addressed (`parsed-<sha>.bin.zst`), so a no-op rerun
    # produces byte-identical output anyway — the only thing the
    # script saves is ~2 s of parse + compress work.
    #
    # Override with `MSL_REBUILD=force` for a guaranteed re-pack.
    #
    # Bust the skip on three inputs, not just `.cache/msl` mtimes:
    #   1. `.cache/msl/*.mo` newer than the manifest — the sources changed.
    #   2. The bundler's OWN source newer than the manifest — its packing
    #      logic changed (e.g. a new `--exclude` filter or serialisation
    #      format). Tracking this makes the old `MSL_REBUILD=force`-after-a-
    #      bundler-edit ritual automatic; a stale bundle is silent corruption.
    #   3. `MSL_EXTRA_LIBS`/`MSL_EXCLUDE_LIBS` requested — the set of packed
    #      libraries is config-driven and not captured by mtimes, so always
    #      repack to honour the current config.
    if [ -z "${MSL_EXTRA_LIBS:-}" ] && [ -z "${MSL_EXCLUDE_LIBS:-}" ] \
        && [ "${MSL_REBUILD:-}" != "force" ] && [ -f "$msl_dir/manifest.json" ]; then
        local msl_src
        for candidate in \
            "$PROJECT_DIR/../.cache/msl" \
            "$PROJECT_DIR/.cache/msl"; do
            if [ -d "$candidate" ]; then msl_src="$candidate"; break; fi
        done
        if [ -n "$msl_src" ]; then
            local newer newer_bundler
            newer=$(find "$msl_src" -name '*.mo' -newer "$msl_dir/manifest.json" -print -quit 2>/dev/null)
            newer_bundler=$(find "$PROJECT_DIR/crates/lunco-assets/src" -name '*.rs' \
                -newer "$msl_dir/manifest.json" -print -quit 2>/dev/null)
            if [ -z "$newer" ] && [ -z "$newer_bundler" ]; then
                info "MSL bundle up-to-date ($msl_src) — skipping pack (set MSL_REBUILD=force to override)"
                return 0
            fi
        fi
    fi

    info "Packing MSL bundle for $binary..."

    # The bundler walks `lunco_assets::msl_source_root_path()` on the host,
    # which lives at <workspace>/.cache/msl/ in this repo. If MSL isn't
    # materialised, the binary will exit non-zero with a clear message and
    # we surface that as a build error so we never ship without MSL.
    # Third-party libraries ship in the SAME bundle as MSL (one combined tar +
    # parsed set; each root keeps its own top-level package namespace). Opt in
    # via `MSL_EXTRA_LIBS`:
    #   MSL_EXTRA_LIBS=discover        → bundle every lib found under cache_dir()
    #   MSL_EXTRA_LIBS=/path/a:/path/b → bundle these explicit roots
    # Default (unset) → MSL only, reproducible across machines.
    local extra_args=()
    if [ -n "${MSL_EXTRA_LIBS:-}" ]; then
        if [ "$MSL_EXTRA_LIBS" = "discover" ]; then
            extra_args+=(--discover-extras)
            info "Bundling discovered third-party libraries from cache_dir()"
        else
            local IFS=':'
            for root in $MSL_EXTRA_LIBS; do
                [ -n "$root" ] && extra_args+=(--extra-root "$root")
            done
            info "Bundling extra library roots: $MSL_EXTRA_LIBS"
        fi
    fi

    # The Modelica Association ships its own regression/conversion test suites
    # (ModelicaTest, ModelicaTestConversion4, ModelicaTestOverdetermined) INSIDE
    # the MSL source tree. They are not part of the library you import, and
    # Dymola/OMEdit don't load them by default — so we keep them out of the web
    # bundle. Override the list (colon-separated; `*` = prefix match) via
    # `MSL_EXCLUDE_LIBS`, or set it empty to ship everything.
    local exclude_libs="${MSL_EXCLUDE_LIBS-ModelicaTest*}"
    if [ -n "$exclude_libs" ]; then
        local IFS=':'
        for name in $exclude_libs; do
            [ -n "$name" ] && extra_args+=(--exclude "$name")
        done
        info "Excluding top-level packages: $exclude_libs"
    fi

    rm -rf "$msl_dir"
    mkdir -p "$msl_dir"
    cargo run --release -q -p lunco-assets --bin build_msl_assets -- \
        --out "$msl_dir" "${extra_args[@]}"

    if [ $? -ne 0 ]; then
        error "MSL bundling failed"
        exit 1
    fi
    success "MSL bundle written to $msl_dir"
}

# Serve the web application from its dist bundle.
serve_web() {
    local binary="$1"
    local crate="$2"
    local dist_dir="$PROJECT_DIR/dist/$binary"
    local port="${3:-8080}"

    if [ ! -f "$dist_dir/index.html" ]; then
        error "No bundle at $dist_dir — run '$0 build $binary' first"
        exit 1
    fi

    info "Starting web server for $binary..."
    info "Serving from: $dist_dir"
    info "URL: http://localhost:$port"

    cd "$dist_dir"

    if [ "$HTTP_SERVER_CMD" = "http-server" ]; then
        info "Using http-server (Node.js)"
        http-server -p "$port" -c-1 --cors
    else
        info "Using Python3 HTTP server"
        python3 -m http.server "$port"
    fi
}

# Clean build artifacts.
# We don't touch target/ globally — that's cargo's job (`cargo clean`). We
# only remove the web-specific intermediates and the dist bundle.
clean() {
    info "Cleaning web build artifacts..."
    local base_target_dir
    base_target_dir=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
    rm -rf "$base_target_dir/web"
    rm -rf "$PROJECT_DIR/dist"
    # Drop cargo wasm outputs from both profiles (lunica + sandbox + worker).
    for profile in web-dev web-release; do
        rm -f "$base_target_dir/wasm32-unknown-unknown/$profile/"{lunica,sandbox,lunica_worker}.wasm
        rm -f "$base_target_dir/wasm32-unknown-unknown/$profile/"{lunica,sandbox,lunica_worker}.d
    done
    success "Cleaned"
}

# Show help
show_help() {
    echo "LunCoSim - Web Build Script"
    echo ""
    echo "Usage: $0 [COMMAND] [BINARY] [PORT] [OPTIONS]"
    echo ""
    echo "Commands:"
    echo "  build <binary>    Build WASM and generate bindings"
    echo "  serve <binary>    Start web server (requires built files)"
    echo "  all <binary>      Build and serve"
    echo "  clean             Remove build artifacts"
    echo "  help              Show this help message"
    echo ""
    echo "Profile (default: fast dev build, no wasm-opt):"
    echo "  --release              Shippable build (fat LTO + wasm-opt size pass)"
    echo ""
    echo "Twin packing (sandbox only — CLI flags override LC_TWIN_* env vars):"
    echo "  --twin-src <path>      Twin folder to pack (default: ~/Documents/lunco/moonbase/twin)"
    echo "                         Pass empty string ('') to skip twin packing entirely"
    echo "  --twin-name <name>     Dist name under assets/twins/ (default: derived from folder)"
    echo "  --twin-scene <file>    Scene .usda file inside the twin (default: moonbase_scene.usda)"
    echo "  --twin-extra <specs>   Extra non-default twins: 'name=scene=/path;name2=scene2=/path2'"
    echo ""
    echo "Available binaries:"
    echo "  lunica       - Modelica Workbench IDE (default port: 8080)"
    echo "  sandbox      - Rover Physics Sandbox (default port: 8081)"
    echo ""
    echo "Examples:"
    echo "  $0 build lunica                              # Fast dev build"
    echo "  $0 build lunica --release                   # Shippable optimized build"
    echo "  $0 all lunica                               # Build (dev) and serve"
    echo "  $0 all sandbox 8082                         # Build and serve on custom port"
    echo "  $0 build sandbox --twin-src ~/twins/mb      # Custom moonbase twin path"
    echo "  $0 build sandbox --twin-src '' # No twin"
    echo "  $0 clean                                    # Clean all artifacts"
    echo ""
    echo "Prerequisites:"
    echo "  - Rust with wasm32-unknown-unknown target"
    echo "  - wasm-bindgen CLI (cargo install wasm-bindgen-cli)"
    echo "  - http-server (npm install -g http-server) OR python3"
}

# Main execution
main() {
    # ── Argument parsing ─────────────────────────────────────────────────────
    # Supported flags (may appear in any position):
    #   --release               use the web-release Cargo profile
    #   --twin-src  <path>      override LC_TWIN_SRC  ('' = skip packing)
    #   --twin-name <name>      override LC_TWIN_NAME
    #   --twin-scene <file>     override LC_TWIN_SCENE
    #   --twin-extra <specs>    override LC_TWIN_EXTRA
    # CLI flags win over pre-exported env vars; env vars win over defaults.
    # The three positional slots are: COMMAND  BINARY  PORT.
    export BUILD_PROFILE="web-dev"
    local positional=()
    local twin_src_flag="__unset__"
    local twin_name_flag="__unset__"
    local twin_scene_flag="__unset__"
    local twin_extra_flag="__unset__"

    local i=0 args=("$@")
    while [ $i -lt ${#args[@]} ]; do
        local arg="${args[$i]}"
        case "$arg" in
            --release)
                export BUILD_PROFILE="web-release"
                ;;
            --twin-src)
                i=$(( i + 1 )); twin_src_flag="${args[$i]:-}"
                ;;
            --twin-src=*)
                twin_src_flag="${arg#--twin-src=}"
                ;;
            --twin-name)
                i=$(( i + 1 )); twin_name_flag="${args[$i]:-}"
                ;;
            --twin-name=*)
                twin_name_flag="${arg#--twin-name=}"
                ;;
            --twin-scene)
                i=$(( i + 1 )); twin_scene_flag="${args[$i]:-}"
                ;;
            --twin-scene=*)
                twin_scene_flag="${arg#--twin-scene=}"
                ;;
            --twin-extra)
                i=$(( i + 1 )); twin_extra_flag="${args[$i]:-}"
                ;;
            --twin-extra=*)
                twin_extra_flag="${arg#--twin-extra=}"
                ;;
            *)
                positional+=("$arg")
                ;;
        esac
        i=$(( i + 1 ))
    done

    # Apply CLI twin overrides → env vars consumed by stage_twins().
    # Only override when the flag was actually supplied (distinguished from
    # the sentinel "__unset__" so that passing --twin-src '' correctly
    # sets LC_TWIN_SRC to the empty string, disabling twin packing).
    [ "$twin_src_flag"   != "__unset__" ] && export LC_TWIN_SRC="$twin_src_flag"
    [ "$twin_name_flag"  != "__unset__" ] && export LC_TWIN_NAME="$twin_name_flag"
    [ "$twin_scene_flag" != "__unset__" ] && export LC_TWIN_SCENE="$twin_scene_flag"
    [ "$twin_extra_flag" != "__unset__" ] && export LC_TWIN_EXTRA="$twin_extra_flag"

    local command="${positional[0]:-help}"
    local binary="${positional[1]:-}"
    local port="${positional[2]:-}"

    case "$command" in
        build)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            build_wasm "$binary" "$crate"
            generate_bindings "$binary" "$crate"
            build_msl_bundle "$binary"
            success "Build complete! Run '$0 serve $binary' to start the server"
            ;;
        serve)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            local default_port=8080
            if [ "$binary" = "sandbox" ]; then
                default_port=8081
            fi
            serve_web "$binary" "$crate" "${port:-$default_port}"
            ;;
        all)
            if [ -z "$binary" ]; then
                error "Binary name required"
                show_help
                exit 1
            fi
            check_prerequisites
            local crate=$(get_binary_config "$binary")
            local default_port=8080
            if [ "$binary" = "sandbox" ]; then
                default_port=8081
            fi
            build_wasm "$binary" "$crate"
            generate_bindings "$binary" "$crate"
            build_msl_bundle "$binary"
            serve_web "$binary" "$crate" "${port:-$default_port}"
            ;;
        clean)
            clean
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            error "Unknown command: $command"
            show_help
            exit 1
            ;;
    esac
}

main "$@"
