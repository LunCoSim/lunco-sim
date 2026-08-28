//! Unified asset management for LunCoSim.
//!
//! This crate is the single source of truth for:
//! - Cache directory resolution (shared across all git worktrees)
//! - Asset source registration (`lunco://` and `twin://`, alongside Bevy's
//!   default asset source)
//! - Unified asset loading that works across desktop and wasm32 targets
//!
//! ## Cache Directory Strategy
//!
//! All worktrees share the same machine-global cache directory to avoid
//! redundant downloads and duplicate processed output. Resolution order:
//!
//! 1. `LUNCOSIM_CACHE` environment variable (explicit override for CI/custom installs)
//! 2. OS-conventional cache directory
//!
//! If neither an explicit override nor an OS cache directory is available,
//! cache resolution fails visibly; it never silently creates a worktree-local
//! cache.
//!
//! ```text
//! ~/.cache/lunco/             # Shared across ALL worktrees and Twins
//! ├── textures/               # Large binaries (earth.jpg, moon.png)
//! ├── ephemeris/              # JPL Horizons CSVs
//! ├── remote/                 # HTTP-downloaded assets
//! └── processed/              # AssetProcessor output
//! ```
//!
//! ## Usage
//!
//! ```rust
//! use lunco_assets::cache_dir;
//!
//! let dir = cache_dir();  // → ~/.cache/lunco/ on Linux
//! ```

// This crate owns the on-disk asset cache layout, so it legitimately
// uses raw `std::fs` / `std::thread` / `Instant`. The workspace lint
// (`disallowed_methods = "deny"` in the root `Cargo.toml`, symbols
// enumerated in `clippy.toml`) bans those for *domain* crates because
// they break wasm32; lunco-assets is on the documented allow-list.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

pub mod asset_path;
pub mod asset_read;
pub mod asset_sources;
pub mod closure;
pub mod datasets;
pub mod discovery;
pub mod download;
pub mod font;
/// `lunco://` asset source — the engine asset *library*. Resolves `assets/`
/// first, then the download cache, so a logical `lunco://` address covers both
/// git-tracked content and externally-fetched binaries without any authored
/// file naming the cache. See `docs/architecture/56-asset-resolution-and-cache.md`.
pub mod lunco_source;
pub mod missions;
pub mod models;
pub mod msl;
/// PDS3 `.IMG` raster decode (attached or detached label) — lets the `dem`/
/// `map` pipelines ingest non-GeoTIFF DEM and ortho products. Native-only:
/// ingest is an offline build step, never a wasm/page path.
#[cfg(not(target_arch = "wasm32"))]
pub mod pds_img;
pub mod process;
/// Scheme → local filesystem root, as an open registry — the read-side mirror of
/// [`register_lunco_asset_sources`].
pub mod scheme_registry;
pub mod script_source;
pub mod scripting;
pub mod tutorials;
pub mod twin_source;
/// Generic browser fetch + Cache-Storage + tar.zst-unpack primitives shared by
/// every bundle distributor (MSL, twin bundles). Web-only — native downloads go
/// through [`download`].
#[cfg(target_arch = "wasm32")]
pub mod web_fetch;

pub use asset_sources::{register_lunco_asset_sources, TwinAssetMounted, TwinRootsPlugin};
#[cfg(not(target_arch = "wasm32"))]
pub use closure::{transitive_file_closure, transitive_file_closure_with};
#[cfg(not(target_arch = "wasm32"))]
pub use lunco_source::{
    existing_path_within_root, read_asset_bytes, read_asset_bytes_with_twin_root,
    read_asset_file_bytes, read_asset_file_string,
};
pub use lunco_source::{
    id_to_disk_path, parse_lunco_uri, shipped_asset_root, ASSETS_DIR_NAME, LUNCO_SCHEME,
};
pub use scheme_registry::{SchemeRegistry, SchemeRegistryError};
pub use twin_source::{
    parse_twin_uri, split_twin_rel, twin_uri, TwinRoots, TwinRootsError, TWIN_SCHEME,
};

// ============================================================================
// Cache Directory Resolution
// ============================================================================

/// Resolves the shared cache directory — ONE location for every worktree
/// and every Twin, holding regenerable artifacts (MSL, textures, ephemeris,
/// engine downloads, and Twin declarations marked `shared = true`). Twin
/// declarations with the default ownership write beside the Twin; all Twin
/// readers may still reuse this global pool.
///
/// Resolution:
///
/// 1. `LUNCOSIM_CACHE` env override (CI, moving GBs to another disk).
/// 2. OS-conventional cache dir:
///    Linux:   `~/.cache/lunco/`
///    macOS:   `~/Library/Caches/lunco/`
///    Windows: `%LOCALAPPDATA%\lunco\`
///
/// If no OS cache directory is resolvable, this function panics with an
/// actionable message. There is deliberately NO per-worktree cache: an earlier
/// dev-mode walk preferred a workspace-local `.cache/` when populated, which silently
/// made each worktree an island re-downloading the same assets. One shared
/// pool, keyed by content (sha256 in manifests, bake keys on outputs), is
/// both cheaper and correct.
///
/// This is the primary way to get the cache path — used by texture processors,
/// ephemeris downloaders, modelica compilers, and any system that reads/writes
/// cached assets.
///
/// # Example
///
/// ```
/// use lunco_assets::cache_dir;
/// let textures = cache_dir().join("textures");
/// ```
pub fn cache_dir() -> PathBuf {
    // wasm32-unknown-unknown has no filesystem — `Path::exists` /
    // `read_dir` panic with "no filesystem on this platform". Return
    // a stable nominal path; callers that try to read/write it will
    // get a clean Err instead of crashing the page.
    #[cfg(target_arch = "wasm32")]
    {
        return PathBuf::from(".cache");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(val) = std::env::var_os("LUNCOSIM_CACHE") {
            return PathBuf::from(val);
        }
        dirs::cache_dir()
            .map(|base| base.join("lunco"))
            .unwrap_or_else(|| {
                panic!("unable to resolve an OS cache directory; set LUNCOSIM_CACHE explicitly")
            })
    }
}

/// The cache that travels WITH the shipped library: `<assets>/.cache`.
///
/// A packaged build is a folder — binary, `assets/`, and the large binaries it
/// needs. Those binaries are cache artifacts (downloaded, processed,
/// git-ignored), so they cannot live in `assets/` proper; but a distribution
/// that only knew the machine-global [`cache_dir`] would arrive empty on a
/// machine that had never run the downloader, and would depend on a launcher
/// script exporting `LUNCOSIM_CACHE` to find its own payload.
///
/// So the library gets a cache of its own, right beside it, read BEFORE the
/// global one. That is the same rule a Twin already follows
/// ([`twin_cache_dir`]): the unit you ship may carry its default-owned data
/// beside itself, while the global pool remains a reusable read source and the
/// explicit owner for `shared = true` declarations.
///
/// Read-only in practice: downloads still land in [`cache_dir`], because a
/// packaged `assets/` may sit on a read-only mount and because one machine
/// should not re-fetch the same product per installed copy.
pub fn packed_cache_dir() -> PathBuf {
    assets_dir_abs().join(".cache")
}

/// The directory holding the engine's dataset manifests: `assets/manifests`.
///
/// Manifests are DATA, not code. They live beside the assets they declare, ship
/// with the package (`assets/` is staged), and are read at runtime — so adding a
/// dataset, correcting a URL or bumping a version is an edit to a `.toml`, not a
/// rebuild. A Twin's `Assets.toml` already worked this way; this makes the
/// engine's own declarations the same kind of thing rather than a compiled-in
/// special case.
///
/// One file per **group**, named for it: `celestial.toml` declares the
/// `celestial` group. The file stem is what the UI shows as the owning library,
/// so a new group is a new file and nothing else.
pub fn manifests_dir() -> PathBuf {
    assets_dir_abs().join("manifests")
}

/// Every engine manifest as `(group, path)`, sorted by group.
///
/// Sorted because it drives both a UI listing and the CLI's iteration order, and
/// `read_dir` order is whatever the filesystem feels like — which would make the
/// download list reshuffle between machines for no reason.
#[cfg(not(target_arch = "wasm32"))]
pub fn engine_manifests() -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    let entries = std::fs::read_dir(manifests_dir())?;
    let mut out: Vec<(String, PathBuf)> = entries
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .filter_map(|p| {
            let group = p.file_stem()?.to_str()?.to_string();
            Some((group, p))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The text of one engine manifest by group name, or `None` when there is no
/// such group. For a consumer that wants its OWN declarations and nothing else
/// (the MSL installer reading `[msl]`), rather than the whole set.
#[cfg(not(target_arch = "wasm32"))]
pub fn engine_manifest_text(group: &str) -> Option<String> {
    std::fs::read_to_string(manifests_dir().join(format!("{group}.toml"))).ok()
}

/// Every cache root a library-relative reference is looked up in, in order:
/// the packed cache beside `assets/`, then the shared machine-wide pool.
///
/// The ONE place that order is decided. The `lunco://` asset source, the
/// synchronous resolver ([`engine_asset_local_path`]) and any tool probing for
/// bytes all ask here, so a file found by the loader is a file found by the
/// validator.
pub fn cache_roots() -> Vec<PathBuf> {
    let mut roots = vec![packed_cache_dir()];
    let shared = cache_dir();
    if !roots.contains(&shared) {
        roots.push(shared);
    }
    roots
}

/// Where a **Twin's** downloaded assets live: `<twin_root>/.cache`.
///
/// Two cache locations, one ownership rule: an asset declared by the ENGINE
/// lands in the shared [`cache_dir`]; a Twin declaration defaults to the
/// Twin-local cache, while `shared = true` explicitly selects the global cache.
/// Twin reads search both locations, so a shared product is reusable without
/// putting a machine-local cache path into authored USD.
///
/// This is the ONE place the layout is decided; the downloader, the
/// `twin://` reader and the dataset registry all ask here.
pub fn twin_cache_dir(twin_root: &std::path::Path) -> PathBuf {
    twin_root.join(".cache")
}

/// Cross-platform temp directory for short-lived scratch files (panic
/// logs, intermediate transcode output, extraction staging).
///
/// Resolution: `LUNCOSIM_TEMP` env override → OS temp dir
/// ([`std::env::temp_dir`]) under a `lunco/` subdir so our scratch
/// files don't litter the shared root. Never hardcode `/tmp`: that
/// path doesn't exist on Windows.
///
/// - Linux:   `$TMPDIR`/`/tmp` → `…/lunco/`
/// - macOS:   `$TMPDIR` → `…/lunco/`
/// - Windows: `%TEMP%` → `…\lunco\`
///
/// The directory is created (best-effort) before returning, so callers
/// can `join(name)` and write immediately.
pub fn temp_dir() -> PathBuf {
    // wasm32 has no filesystem; return a nominal path and let any write
    // surface a clean Err rather than panicking in `temp_dir()`.
    #[cfg(target_arch = "wasm32")]
    {
        return PathBuf::from(".tmp");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let base = std::env::var_os("LUNCOSIM_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("lunco"));
        let _ = std::fs::create_dir_all(&base);
        base
    }
}

/// Returns the subdirectory within the cache for a specific asset category.
///
/// Creates the directory if it doesn't exist.
///
/// # Categories
/// - `textures` — Generated or downloaded textures (Earth, Moon, terrain maps)
/// - `ephemeris` — JPL Horizons CSV ephemeris data
/// - `remote` — HTTP-downloaded assets with integrity hashes
/// - `processed` — Preprocessed asset output (optimized USD, compressed textures)
/// - `modelica` — Modelica compilation output (`.cache/modelica/`)
/// - `msl` — Modelica Standard Library cache (`.cache/msl/`)
pub fn cache_subdir(name: &str) -> PathBuf {
    let dir = cache_dir().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Returns the `textures` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("textures")`. This is the cache half of
/// `lunco://textures/…`, which the `lunco://` source reaches via its cache
/// fallback — authored content names that address, never this directory.
pub fn textures_dir() -> PathBuf {
    cache_subdir("textures")
}

/// Returns the `ephemeris` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("ephemeris")`. Used by JPL Horizons
/// download systems and ephemeris lookup.
pub fn ephemeris_dir() -> PathBuf {
    cache_subdir("ephemeris")
}

/// Returns the `remote` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("remote")`. Reserved for HTTP-downloaded
/// assets that should persist across runs.
pub fn remote_dir() -> PathBuf {
    cache_subdir("remote")
}

/// Returns the `processed` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("processed")`. Reserved for preprocessed
/// asset output (e.g., optimized USD files, compressed textures).
pub fn processed_dir() -> PathBuf {
    cache_subdir("processed")
}

/// Returns the `modelica` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("modelica")`. Used by Modelica compilation
/// output for individual entities.
pub fn modelica_dir() -> PathBuf {
    cache_subdir("modelica")
}

/// Returns the `msl` subdirectory within the cache.
///
/// Shorthand for `cache_subdir("msl")`. Used for Modelica Standard Library
/// caching in the library browser.
pub fn msl_dir() -> PathBuf {
    cache_subdir("msl")
}

/// Returns the on-disk filesystem path that should be registered
/// as a rumoca source root for Modelica Standard Library access,
/// **if and only if it's materialised as real files on this target**.
///
/// Narrower than [`msl_dir`] — that returns the cache subdir even
/// when empty. This returns `None` when the MSL tree isn't
/// present (first run before index build, or `wasm32` where MSL
/// is served via HTTP fetch rather than the filesystem).
///
/// # What's at this path
///
/// This is `<cache>/msl/` itself — **not** `<cache>/msl/Modelica/`.
/// The difference matters because MSL ships several top-level
/// entities as siblings of the `Modelica/` directory:
///
/// - `Modelica/` — the core library (≈ 2400 classes).
/// - `Complex.mo` — the top-level `operator record Complex` used by
///   `ComplexBlocks`, `ComplexMath`, `Magnetic.FundamentalWave`, etc.
///   User models that reference `Complex` (or transitively via MSL
///   types) will fail to resolve unless this file is in scope.
/// - `ModelicaServices/` — vendor-specific animation / file-IO /
///   event-logger services MSL calls into.
/// - `ObsoleteModelica4.mo` — the standard MSL file containing its obsolete
///   class definitions; it remains part of the source root because MSL models
///   may reference those definitions.
///
/// Pointing rumoca at `<cache>/msl/` picks up all of the above at
/// the correct namespace rooting.
///
/// This is the single chokepoint that integrations like rumoca's
/// compile session use to register MSL as a source root. When we
/// move to the async `AssetSource` abstraction for full web
/// support, this function will return `None` on `wasm32` and the
/// compile path will instead populate rumoca's source set by
/// streaming bytes through the asset source. Native unchanged.
pub fn msl_source_root_path() -> Option<PathBuf> {
    let root = msl_dir();
    // Use the presence of `Modelica/` as the marker that the tree
    // is materialised. `Complex.mo` alone isn't a strong enough
    // signal — it's a small top-level file and might predate a
    // botched Modelica tree delete.
    if !root.join("Modelica").exists() {
        return None;
    }
    // Canonicalize so callers see the same absolute path regardless of CWD.
    // Rumoca's bincode source-root cache keys on the exact path it receives,
    // so a CWD-dependent relative form would produce different keys per caller
    // and force full reparses.
    std::fs::canonicalize(&root).ok().or(Some(root))
}

// ============================================================================
// Assets Directory (development source)
// ============================================================================

/// Returns the development `assets/` directory relative to the current working
/// directory.
///
/// This is the development source directory for USD scenes, Modelica models,
/// mission JSONs, and shaders. At runtime, the working directory is typically
/// the crate root or workspace root.
///
/// For tests and examples that need a stable path regardless of CWD, prefer
/// passing the asset root explicitly rather than relying on this function.
pub fn assets_dir() -> PathBuf {
    PathBuf::from(lunco_source::ASSETS_DIR_NAME)
}

/// Resolves the shipped-library root used by Bevy's `AssetPlugin`.
///
/// Packaged native binaries carry `assets/` beside the executable. Use that
/// location when present so launching a Windows `.exe` from Explorer or a
/// shortcut does not make asset lookup depend on the process working directory.
/// Development/test binaries do not have a sibling `assets/` directory, so they
/// retain the workspace-CWD behaviour.
///
/// Anything reaching library bytes off the `AssetServer` must anchor here rather
/// than joining `"assets"` itself: a bare relative join silently follows the CWD
/// of whoever calls it, which is how the same reference resolved two ways.
pub fn assets_dir_abs() -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let packaged = parent.join(assets_dir());
            if packaged.is_dir() {
                return packaged;
            }
        }
    }

    std::env::current_dir()
        .unwrap_or_default()
        .join(assets_dir())
}

/// On-disk root of a shipped Modelica package under `assets/models/` — the
/// MODELICAPATH entry for a top-level library name (`"LunCo"` →
/// `<assets>/models/LunCo`). `None` when it is not a structured package on this
/// filesystem, which is the normal case on wasm.
///
/// Anchored on [`assets_dir_abs`], the SAME path Bevy's `AssetPlugin.file_path`
/// serves, and that parity is the whole point. A `.mo` named by
/// `info:sourceAsset` reaches the compiler through the AssetServer, so
/// it is read live from disk; loading the library it belongs to out of the
/// build-time `include_dir!` copy instead would compile an edited member as its
/// last-built self until someone ran `cargo build`. Same tree both ways, or the
/// two disagree silently.
///
/// `package.mo` is the existence marker because that is what makes a directory a
/// STRUCTURED entity in Modelica's file-system mapping, rather than merely a
/// folder that happens to hold `.mo` files.
#[cfg(not(target_arch = "wasm32"))]
pub fn models_package_root_path(package: &str) -> Option<PathBuf> {
    let root = assets_dir_abs().join("models").join(package);
    if !root.join("package.mo").is_file() {
        return None;
    }
    // Canonicalize for the same reason `msl_source_root_path` does: rumoca keys
    // its source-root cache on the exact path it is handed, so a CWD-dependent
    // form would produce a different key per caller and force full reparses.
    std::fs::canonicalize(&root).ok().or(Some(root))
}

/// wasm has no filesystem to put a library on, so there is no MODELICAPATH entry
/// and callers fall back to the embedded copy from [`models::package_files`].
#[cfg(target_arch = "wasm32")]
pub fn models_package_root_path(_package: &str) -> Option<PathBuf> {
    None
}

/// Cache `scenarios/` directory — where a downloaded scenario's files are
/// materialised, one subdirectory per scenario id. A downloaded scenario is
/// mounted as an ordinary Twin root over that subdirectory, which is why it
/// needs no URI scheme of its own.
///
/// The `<cache>/scenarios/…` layout is a **private** detail of that staging:
/// three crates previously rebuilt the join by hand, so a client probing for
/// cached bytes could look somewhere the writer had never written.
pub fn scenarios_dir() -> PathBuf {
    cache_subdir("scenarios")
}

/// Resolve a built-in engine-library asset reference to a load path that is
/// **independent of the active document's root**.
///
/// A bare, scheme-less path like `shaders/wheel.wgsl` (as USD authors it in
/// `info:wgsl:sourceAsset`, or as engine code hard-codes it) otherwise loads
/// against Bevy's *default* source — which, once an external Twin is open, is the
/// wrong root: the shipped shader isn't co-located with a user's scene, so the
/// load misses (a ShaderMaterial with an unresolved shader renders as a black
/// hole). Routing it through the `lunco://` source — registered by
/// [`register_lunco_asset_sources`] onto the shipped `assets/` library — makes a
/// built-in asset resolve to the engine library from anywhere, exactly like the
/// scene author writing `@lunco://vessels/rovers/skid_rover.usda@`.
///
/// Whether a reference already names its own asset source (`lunco://`, `twin://`,
/// `http(s)://`, or another registered scheme) and so must be passed through untouched
/// rather than re-anchored against a root.
///
/// The predicate is one line, but it is the same *decision* everywhere it is
/// made — "is this already addressable?" — so it is named once here instead of
/// open-coded as `contains("://")` in every crate that loads an asset.
pub fn has_scheme(reference: impl AsRef<str>) -> bool {
    asset_path::split_scheme(reference.as_ref()).is_some()
}

/// The addressable `lunco://` form of an engine-library reference.
///
/// A reference that ALREADY carries a scheme (`lunco://…`, `twin://…`,
/// `http(s)://…`, or another registered scheme) is returned unchanged — a Twin shipping
/// its OWN shader (`twin://name/shaders/custom.wgsl`) must keep resolving against
/// the Twin, and an already-`lunco://` path must not be double-prefixed.
///
/// A leading `assets/` is stripped first: that directory is the *root* the
/// `lunco://` source is mounted on, so `assets/foo.rhai` and `foo.rhai` name one
/// file. Callers used to strip it themselves next to this call, which is the same
/// knowledge in two places — and the literal `"assets"` belongs to
/// [`ASSETS_DIR_NAME`], not to a caller.
pub fn engine_asset_uri(reference: &str) -> String {
    if has_scheme(reference) {
        return reference.to_string();
    }
    let rel = reference
        .strip_prefix(ASSETS_DIR_NAME)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(reference);
    asset_path::uri(LUNCO_SCHEME, rel)
}

/// The engine-library-relative form of a reference — the path UNDER `assets/`,
/// with the `lunco://` scheme (if any) stripped. Bare and `lunco://` references
/// both collapse to the same relative path (`shaders/wheel.wgsl`); a reference
/// carrying ANOTHER scheme (`twin://`, `http…`) is returned untouched, since it
/// does not live in the shipped library. The inverse of [`engine_asset_uri`] for
/// the `lunco://` case — use it before string-matching or comparing a reference
/// so an authored `@lunco://…@` and a bare `@…@` behave identically.
pub fn engine_asset_rel(reference: &str) -> &str {
    parse_lunco_uri(reference).unwrap_or(reference)
}

/// The local filesystem path a reference resolves to *within the shipped
/// `assets/` library*, or `None` when it lives under a different scheme's root
/// (`twin://`, `http…`) and therefore has no engine-library path.
///
/// This is the read-side companion of [`engine_asset_uri`]: it mirrors the
/// `lunco://` → `<cwd>/assets` mapping that [`register_lunco_asset_sources`]
/// installs, so code that must inspect an asset WITHOUT the `AssetServer` (e.g.
/// the shader `@fragment` pre-validator) resolves a reference exactly as the
/// loader will — whether it was authored bare (`shaders/wheel.wgsl`) or schemed
/// (`lunco://shaders/wheel.wgsl`).
///
/// It probes the SAME chain the source reads — `assets/`, then the packed
/// cache, then the shared pool ([`cache_roots`]) — and returns the first root
/// that actually holds the file. When none does, it returns the `assets/` path
/// anyway, so an error message names where the file was EXPECTED rather than
/// where it was last looked for.
pub fn engine_asset_local_path(reference: &str) -> Option<PathBuf> {
    let rel = engine_asset_rel(reference);
    if has_scheme(rel) {
        return None; // another scheme's root — not in the shipped library
    }
    if !asset_path::is_safe_relative_path(rel) {
        return None;
    }
    let authored = assets_dir_abs().join(rel);
    if authored.exists() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            return match existing_path_within_root(&assets_dir_abs(), Path::new(rel)) {
                Ok(path) => path.filter(|path| path.is_file()),
                Err(error) => {
                    bevy::log::warn!(
                        "[lunco-assets] cannot resolve authored asset `{rel}`: {error}"
                    );
                    None
                }
            };
        }
        #[cfg(target_arch = "wasm32")]
        return Some(authored);
    }
    for root in cache_roots() {
        let candidate = root.join(rel);
        if candidate.exists() {
            #[cfg(not(target_arch = "wasm32"))]
            {
                return match existing_path_within_root(&root, Path::new(rel)) {
                    Ok(path) => path.filter(|path| path.is_file()),
                    Err(error) => {
                        bevy::log::warn!(
                            "[lunco-assets] cannot resolve cached asset `{rel}` under {}: {error}",
                            root.display()
                        );
                        None
                    }
                };
            }
            #[cfg(target_arch = "wasm32")]
            return Some(candidate);
        }
    }
    Some(authored)
}

/// The local filesystem path ANY reference resolves to, whichever root owns it —
/// a `twin://<name>/<rel>` against the open Twin's root, anything else against
/// the shipped engine library. `None` when the Twin is not open or the reference
/// belongs to a scheme with no local path (`http…`).
///
/// This is the single read-side resolution entry point for code that must reach
/// bytes WITHOUT the `AssetServer` (scenario sync, shader pre-validation, file
/// dialogs). Callers previously re-implemented the `twin://` split-and-join next
/// to a hardcoded `"assets"` literal, which drifted from the readers this crate
/// registers — same URI, two different answers depending on who asked.
/// The library-relative form of an ABSOLUTE filesystem path that lives under the
/// shipped `assets/` root, or `None` when it lives elsewhere. The inverse of
/// [`engine_asset_local_path`].
///
/// Callers hand an absolute path to the `AssetServer`, which prepends its own
/// configured root to every load string — so a path under the library has to be
/// reduced to its relative form or the load resolves to `<assets>/<assets>/…`.
pub fn library_rel(path: &Path) -> Option<String> {
    path.strip_prefix(assets_dir_abs())
        .ok()
        .map(asset_path::slashed)
}

/// Cache `fonts/` directory — where `lunco-assets -- download`
/// materialises font files declared in per-crate `Assets.toml`. Lives
/// under [`cache_dir`] because these are downloaded artifacts, not
/// authored source. Shared across all worktrees (same as textures,
/// ephemeris) — one `cargo run -p lunco-assets -- download` populates
/// every git worktree at once.
pub fn fonts_dir() -> PathBuf {
    cache_subdir("fonts")
}

/// Full path to the **DejaVu Sans** TTF — the workspace's
/// proportional-text fallback. Picked over Noto because Noto's base
/// Sans and Symbols 2 *together* still leave gaps in the
/// Mathematical Operators block (U+2200-22FF), while DejaVu Sans
/// covers arrows + math operators + misc technical (U+2190-2311)
/// contiguously in one file. Matches the Godot/Blender choice.
///
/// Resolved through the SAME canonical search as every other shipped asset —
/// [`engine_asset_local_path`] / [`cache_roots`]: authored `assets/fonts/` →
/// packed `assets/.cache/fonts/` → shared machine cache `<cache_dir>/fonts/`.
/// There is no font-specific path logic: the font is just an engine asset named
/// `fonts/DejaVuSans.ttf`, so it finds a PACKAGED build's bundled copy (which
/// ships in `assets/.cache/`) as readily as a `download`-populated shared cache.
///
/// Previously this returned `fonts_dir()` (the shared cache) alone, so a fresh
/// packaged install reported the font missing until the machine-global cache was
/// populated — even though the bundle carried it. `engine_asset_local_path`
/// yields the last authored candidate when nothing exists, so a genuinely absent
/// font still returns a sensible path for the "not found" warning.
///
/// Populated by `cargo run -p lunco-assets -- download` via the
/// `crates/lunco-theme/Assets.toml` entry.
pub fn dejavu_sans_path() -> PathBuf {
    // The multi-root search probes the filesystem (`Path::exists`), which panics
    // on wasm's no-filesystem target; there the font arrives via `fetch`, not this
    // path, so keep the nominal shared-cache path for wasm callers.
    #[cfg(not(target_arch = "wasm32"))]
    {
        engine_asset_local_path("fonts/DejaVuSans.ttf")
            .unwrap_or_else(|| fonts_dir().join("DejaVuSans.ttf"))
    }
    #[cfg(target_arch = "wasm32")]
    {
        fonts_dir().join("DejaVuSans.ttf")
    }
}

/// Constructs a Modelica compilation output path for a given entity.
///
/// Returns a `PathBuf` within [`modelica_dir()`].
/// Each entity gets its own subdirectory for generated FMUs, compiled output, etc.
pub fn modelica_entity_dir(entity_name: &str) -> PathBuf {
    modelica_dir().join(entity_name)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_defaults_to_os_global_cache() {
        // When LUNCOSIM_CACHE is not set, use the OS-global cache.
        // (In CI this test may run with the env var set, so we only test the function exists)
        let dir = cache_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn cache_subdir_creates_directory() {
        let test_subdir = cache_dir().join("test_subdir");
        let _ = std::fs::create_dir_all(&test_subdir);
        assert!(test_subdir.exists());
        let _ = std::fs::remove_dir_all(&test_subdir);
    }

    /// The two caches, and the rule that keeps a twin portable.
    #[test]
    fn a_twins_cache_sits_beside_it_not_in_the_shared_one() {
        let twin = std::path::Path::new("/tmp/some_twin");
        assert_eq!(twin_cache_dir(twin), twin.join(".cache"));
        assert!(!twin_cache_dir(twin).starts_with(cache_dir()));
    }

    #[test]
    fn engine_local_paths_reject_root_escape() {
        for reference in [
            "../outside.usda",
            "terrain/../../outside.usda",
            "lunco://../outside.usda",
            "lunco://terrain/../../outside.usda",
        ] {
            assert_eq!(
                engine_asset_local_path(reference),
                None,
                "unsafe engine reference must be rejected: {reference}"
            );
        }
    }
}
