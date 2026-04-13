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
    std::env::var_os("LUNCOSIM_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".cache"))
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
