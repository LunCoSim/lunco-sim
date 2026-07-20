//! Unified asset management for LunCoSim.
//!
//! This crate is the single source of truth for:
//! - Cache directory resolution (shared across all git worktrees)
//! - Asset source registration (`cache://`, `user://`, `assets://`)
//! - Unified asset loading that works across desktop and wasm32 targets
//!
//! ## Cache Directory Strategy
//!
//! All worktrees share the same cache directory to avoid redundant downloads
//! and duplicate processed output. Resolution order:
//!
//! 1. `LUNCOSIM_CACHE` environment variable (set in `.cargo/config.toml`)
//! 2. Fallback to `.cache/` relative to the workspace root
//!
//! ```text
//! ~/.cache/luncosim/          # Shared across ALL worktrees
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
//! let dir = cache_dir();  // → ~/.cache/luncosim/ on Linux
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

pub use asset_sources::register_lunco_asset_sources;
pub use lunco_source::{
    id_to_disk_path, parse_lunco_uri, shipped_asset_root, ASSETS_DIR_NAME, LUNCO_SCHEME,
};
pub use scheme_registry::SchemeRegistry;
pub use twin_source::{parse_twin_uri, split_twin_rel, twin_uri, TwinRoots, TWIN_SCHEME};

// ============================================================================
// User Config Directory Resolution
// ============================================================================

/// Resolves the user-level config directory for LunCoSim — for
/// settings, recents, keybinds, palette history, layouts, and any
/// other **per-user persistent state** that must survive `cargo clean`
/// and is independent of any one Twin.
///
/// Resolution order:
///
/// 1. `LUNCOSIM_CONFIG` environment variable if set (testing, custom
///    installs, sandboxed CI).
/// 2. Legacy `~/.lunco/` if it already exists (backwards compat for
///    users who started before we adopted OS-conventional dirs).
/// 3. OS-conventional config dir via [`dirs::config_dir`]:
///    - Linux:   `~/.config/lunco/`
///    - macOS:   `~/Library/Application Support/lunco/`
///    - Windows: `%APPDATA%\lunco\` (i.e. `C:\Users\<user>\AppData\Roaming\lunco\`)
/// 4. `~/.lunco/` if no OS-conventional dir is available.
/// 5. `.lunco/` in the CWD as a pathological last resort.
///
/// The directory is **not created** by this function — callers that
/// write into a subdir use [`user_config_subdir`] which `create_dir_all`s.
/// Read-only callers (existence probes for migrations, etc.) get a
/// path back regardless of whether the dir exists.
///
/// Distinct from [`cache_dir`]: that returns the regenerable artifact
/// cache (MSL, textures, ephemeris). Anything safe to delete and
/// re-download belongs there. User config does not.
pub fn user_config_dir() -> PathBuf {
    if let Some(val) = std::env::var_os("LUNCOSIM_CONFIG") {
        return PathBuf::from(val);
    }
    // Legacy: if the user already has `~/.lunco/` from an earlier
    // build, keep using it so their settings/recents don't suddenly
    // vanish from under them. New installs land in the OS dir below.
    if let Some(home) = dirs::home_dir() {
        let legacy = home.join(".lunco");
        if legacy.exists() {
            return legacy;
        }
    }
    if let Some(cfg) = dirs::config_dir() {
        return cfg.join("lunco");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".lunco");
    }
    PathBuf::from(".lunco")
}

/// User-level data directory for projects, exported simulations, FMUs,
/// logs — anything the user produced and would be upset to lose.
///
/// Resolution: `LUNCOSIM_DATA` env → OS-conventional data dir
/// ([`dirs::data_dir`]) under `lunco/` → fall back to [`user_config_dir`]
/// so callers always get *some* writable location.
///
/// - Linux:   `~/.local/share/lunco/`
/// - macOS:   `~/Library/Application Support/lunco/`
/// - Windows: `%APPDATA%\lunco\`
pub fn user_data_dir() -> PathBuf {
    if let Some(val) = std::env::var_os("LUNCOSIM_DATA") {
        return PathBuf::from(val);
    }
    if let Some(d) = dirs::data_dir() {
        return d.join("lunco");
    }
    user_config_dir()
}

/// Returns a named subdirectory of [`user_config_dir`], creating it
/// (and any missing parents) on the way out.
///
/// Use this for *write* paths; for *probe* paths (existence checks,
/// migrations) call `user_config_dir().join(name)` directly so a
/// missing dir doesn't get materialised on a no-op read.
///
/// # Examples
///
/// ```no_run
/// use lunco_assets::user_config_subdir;
///
/// let recents = user_config_subdir("").join("recents.json");
/// // → ~/.lunco/recents.json (Linux/macOS), C:\Users\u\.lunco\recents.json (Windows)
/// ```
pub fn user_config_subdir(name: &str) -> PathBuf {
    let dir = if name.is_empty() {
        user_config_dir()
    } else {
        user_config_dir().join(name)
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// ============================================================================
// Cache Directory Resolution
// ============================================================================

/// Resolves the shared cache directory.
///
/// Reads `LUNCOSIM_CACHE` from the environment, falling back to `.cache/`
/// in the current working directory. All worktrees should point to the same
/// location via the env var in `.cargo/config.toml`.
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
        // 1. Runtime env var overrides everything.
        if let Some(val) = std::env::var_os("LUNCOSIM_CACHE") {
            return PathBuf::from(val);
        }
        // 2. Dev mode: walk up from this crate's manifest looking for a
        //    workspace `.cache/msl/Modelica` that's actually populated.
        //    CARGO_MANIFEST_DIR = .../modelica/crates/lunco-assets.
        //    Only succeeds inside a worktree; packaged installs skip
        //    straight to (3).
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut current = Some(manifest);
        for _ in 0..10 {
            if let Some(dir) = &current {
                let candidate = dir.join(".cache");
                let msl_modelica = candidate.join("msl").join("Modelica");
                if msl_modelica.exists()
                    && msl_modelica
                        .read_dir()
                        .map(|mut d| d.next().is_some())
                        .unwrap_or(false)
                {
                    return candidate;
                }
                current = dir.parent().map(PathBuf::from);
            }
        }
        // 3. OS-conventional cache dir for end users:
        //      Linux:   ~/.cache/lunco/
        //      macOS:   ~/Library/Caches/lunco/
        //      Windows: %LOCALAPPDATA%\lunco\
        if let Some(c) = dirs::cache_dir() {
            return c.join("lunco");
        }
        // 4. Last resort: CWD-relative.
        PathBuf::from(".cache")
    }
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
/// - `ObsoleteModelica4.mo` — deprecated classes retained for
///   backward compatibility.
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
    // Canonicalize so callers see the same absolute path regardless
    // of CWD. `LUNCOSIM_CACHE = "../.cache"` in `.cargo/config.toml`
    // is relative, and rumoca's bincode source-root cache keys on the
    // exact path it receives — a CWD-dependent relative form would
    // produce different keys per caller and force full reparses.
    std::fs::canonicalize(&root).ok().or(Some(root))
}

// ============================================================================
// Assets Directory (development source)
// ============================================================================

/// Returns the `assets/` directory relative to the current working directory.
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

/// [`assets_dir`] resolved against the process CWD — the ABSOLUTE shipped-library
/// root, and the exact path Bevy's `AssetPlugin.file_path` is configured with.
///
/// Anything reaching library bytes off the `AssetServer` must anchor here rather
/// than joining `"assets"` itself: a bare relative join silently follows the CWD
/// of whoever calls it, which is how the same reference resolved two ways.
pub fn assets_dir_abs() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(assets_dir())
}

/// On-disk root of a shipped Modelica package under `assets/models/` — the
/// MODELICAPATH entry for a top-level library name (`"LunCo"` →
/// `<assets>/models/LunCo`). `None` when it is not a structured package on this
/// filesystem, which is the normal case on wasm.
///
/// Anchored on [`assets_dir_abs`], the SAME path Bevy's `AssetPlugin.file_path`
/// serves, and that parity is the whole point. A `.mo` named by
/// `lunco:program:sourceAsset` reaches the compiler through the AssetServer, so
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
/// `cached_textures://`, `http(s)://`, …) and so must be passed through untouched
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
/// `cached_textures://…`, `http(s)://…`) is returned unchanged — a Twin shipping
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
pub fn engine_asset_local_path(reference: &str) -> Option<PathBuf> {
    let rel = engine_asset_rel(reference);
    if has_scheme(rel) {
        return None; // another scheme's root — not in the shipped library
    }
    Some(assets_dir_abs().join(rel))
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
/// Resolves to `<cache_dir>/fonts/DejaVuSans.ttf`. Populated by
/// `cargo run -p lunco-assets -- download` via the
/// `crates/lunco-theme/Assets.toml` entry.
pub fn dejavu_sans_path() -> PathBuf {
    fonts_dir().join("DejaVuSans.ttf")
}

/// Constructs an ephemeris cache file path for a given target ID.
///
/// The filename format matches the JPL Horizons CSV convention:
/// `target_{id}_{start}_{stop}.csv`
///
/// Returns a `PathBuf` within [`ephemeris_dir()`].
pub fn ephemeris_path_for_target(target_id: &str, date_range_start: &str, date_range_end: &str) -> PathBuf {
    ephemeris_dir().join(format!(
        "target_{target_id}_{date_range_start}_{date_range_end}.csv"
    ))
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
    fn user_config_dir_returns_path() {
        // Function is infallible — returns *some* path regardless of
        // platform / env. Don't assert the exact location since CI
        // may set `LUNCOSIM_CONFIG` or run with HOME unset.
        let dir = user_config_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn user_config_dir_honours_env_override() {
        let prev = std::env::var_os("LUNCOSIM_CONFIG");
        // SAFETY: tests in this module run sequentially relative to
        // each other (they don't, in fact, but the env var is unique
        // to this single test and we restore it). Fine for a single-
        // file unit test.
        std::env::set_var("LUNCOSIM_CONFIG", "/tmp/lunco-test-config");
        assert_eq!(user_config_dir(), PathBuf::from("/tmp/lunco-test-config"));
        match prev {
            Some(v) => std::env::set_var("LUNCOSIM_CONFIG", v),
            None => std::env::remove_var("LUNCOSIM_CONFIG"),
        }
    }

    #[test]
    fn user_config_subdir_creates_dir() {
        let prev = std::env::var_os("LUNCOSIM_CONFIG");
        let tmp = std::env::temp_dir().join(format!(
            "lunco-test-cfg-{}",
            std::process::id()
        ));
        std::env::set_var("LUNCOSIM_CONFIG", &tmp);
        let sub = user_config_subdir("recents");
        assert!(sub.exists());
        assert!(sub.ends_with("recents"));
        let _ = std::fs::remove_dir_all(&tmp);
        match prev {
            Some(v) => std::env::set_var("LUNCOSIM_CONFIG", v),
            None => std::env::remove_var("LUNCOSIM_CONFIG"),
        }
    }

    #[test]
    fn cache_dir_defaults_to_dot_cache() {
        // When LUNCOSIM_CACHE is not set, falls back to .cache
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

    #[test]
    fn ephemeris_path_format() {
        let path = ephemeris_path_for_target("-1024", "2026-04-02_0159", "2026-04-11_0001");
        assert!(path.ends_with("target_-1024_2026-04-02_0159_2026-04-11_0001.csv"));
    }
}
