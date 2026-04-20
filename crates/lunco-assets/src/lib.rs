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

use std::path::PathBuf;

pub mod download;
pub mod process;

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
    // 1. Runtime env var overrides everything
    if let Some(val) = std::env::var_os("LUNCOSIM_CACHE") {
        return PathBuf::from(val);
    }
    // 2. Walk up from this crate's manifest to find .cache/msl
    //    CARGO_MANIFEST_DIR = .../modelica/crates/lunco-assets
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut current = Some(manifest.clone());
    for _ in 0..10 {
        if let Some(dir) = &current {
            let candidate = dir.join(".cache");
            let msl_modelica = candidate.join("msl").join("Modelica");
            // Check it has actual content
            if msl_modelica.exists() {
                if msl_modelica.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
                    return candidate;
                }
            }
            current = dir.parent().map(PathBuf::from);
        }
    }
    // 3. Last resort: CWD-relative
    PathBuf::from(".cache")
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
/// Shorthand for `cache_subdir("textures")`. Used by texture loaders
/// and the `cached_textures://` asset source.
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
    if root.join("Modelica").exists() {
        Some(root)
    } else {
        None
    }
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
    PathBuf::from("assets")
}

/// Constructs a `cached_textures://` asset path from a filename.
///
/// This is the canonical way to reference textures that live in the cache
/// directory. On desktop, the `cached_textures://` asset source points to
/// [`textures_dir()`]; on wasm32, these are embedded at compile time.
///
/// # Example
///
/// ```
/// use lunco_assets::cached_texture_path;
/// let path = cached_texture_path("earth.png");
/// assert_eq!(path, "cached_textures://earth.png");
/// ```
pub fn cached_texture_path(filename: &str) -> String {
    format!("cached_textures://{filename}")
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
    fn cached_texture_path_format() {
        assert_eq!(cached_texture_path("earth.png"), "cached_textures://earth.png");
        assert_eq!(cached_texture_path("moon.png"), "cached_textures://moon.png");
    }

    #[test]
    fn ephemeris_path_format() {
        let path = ephemeris_path_for_target("-1024", "2026-04-02_0159", "2026-04-11_0001");
        assert!(path.ends_with("target_-1024_2026-04-02_0159_2026-04-11_0001.csv"));
    }
}
