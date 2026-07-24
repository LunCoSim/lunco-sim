//! Asset download and version verification.
//!
//! Each crate can declare its own `Assets.toml` mirroring the `Cargo.toml` pattern.
//! This module reads those files, downloads the assets, and verifies integrity.
//!
//! ## Assets.toml Format
//!
//! ```toml
//! [msl]
//! name = "Modelica Standard Library"
//! version = "4.1.0"
//! url = "https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v4.1.0.tar.gz"
//! dest = "msl"
//! # sha256 = ""  # fill after first download
//! ```
//!
//! ## Versioning Strategies
//!
//! | Asset | Strategy | Example |
//! |-------|----------|---------|
//! | Libraries (MSL) | `version` (semver) | `"4.1.0"` → `msl/4.1.0/` |
//! | Textures | `sha256` (content hash) | `"abc123..."` |
//! | Ephemeris | date in filename | `target_-1024_2026-04-02.csv` |

use crate::{cache_dir, process::ProcessConfig};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A single asset entry from `Assets.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntry {
    /// Human-readable name.
    pub name: String,
    /// Semantic version (for libraries). Changes trigger re-download.
    pub version: Option<String>,
    /// URL to download from.
    pub url: String,
    /// Destination path — **optional**. Omit it for plain source downloads:
    /// the file then lands in the OWNER's source pool,
    /// `<owner-cache>/sources/<sha256(url)[..16]>/<basename>` — the shared
    /// cache for a crate manifest, `<twin>/.cache` for a Twin's (see
    /// [`source_pool_path`], and `shared` below to opt into the global pool).
    ///
    /// Author `dest` only when the file must live at a specific path:
    /// relative to the owner's cache root (safety-checked for twins).
    ///
    /// For tarballs without `extract`: the archive is extracted into
    /// this directory.
    ///
    /// For tarballs WITH `extract`: only the named file inside the
    /// archive is copied to this path (`dest` becomes the final
    /// output file, not a directory).
    ///
    /// For single-file downloads: the bytes are written directly here.
    #[serde(default)]
    pub dest: Option<String>,
    /// Optional archive-internal path of the file to pull out of a
    /// tarball, relative to the tarball root after the usual
    /// "first-directory" prefix is stripped. When set, only this one
    /// file is copied to `dest` and the rest of the archive is
    /// discarded — handy for fonts / shader collections where the
    /// upstream ships many files but we only need one.
    ///
    /// Example: `extract = "ttf/DejaVuSans.ttf"` picks only
    /// `DejaVuSans.ttf` out of a full dejavu-fonts release tarball.
    #[serde(default)]
    pub extract: Option<String>,
    /// Put this download in the **shared** source pool
    /// (`<cache>/sources/<url-hash>/<file>`) instead of the owner's own cache.
    ///
    /// Default `false`: a Twin's downloads live in that Twin's `.cache`, so the
    /// Twin is self-contained — copy the folder and its data comes along,
    /// delete it and nothing is orphaned. Set `shared = true` for a large
    /// upstream product that several Twins legitimately reuse (a multi-GB DTM
    /// mosaic), trading self-containment for a single copy on disk.
    ///
    /// Ignored for engine-scoped entries: their owner's cache IS the shared
    /// cache, so the two resolve to the same place.
    #[serde(default)]
    pub shared: bool,
    /// Expected SHA-256 hex digest. Empty string means "compute and suggest".
    pub sha256: Option<String>,
    /// Optional post-processing step (resize, convert).
    #[serde(default)]
    pub process: Option<ProcessConfig>,
    /// Every other key in the entry's table, kept verbatim.
    ///
    /// A dataset's DOMAIN metadata belongs with the declaration that produced
    /// it — the Horizons query's `CENTER` describes those very bytes, and a
    /// second file repeating it is a second thing to get wrong. But this crate
    /// must not learn what a NAIF id is, so domain keys ride in a sub-table
    /// (`[artemis2_vectors.ephemeris]`) that transport carries and never
    /// interprets. The owning crate reads it back with
    /// [`domain`](AssetEntry::domain).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl AssetEntry {
    /// Deserialize this entry's `[<key>]` domain sub-table, if present.
    ///
    /// `None` when the entry declares no such sub-table; `Err` when it does but
    /// the shape is wrong — a typo'd declaration must be loud, not ignored.
    pub fn domain<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Option<Result<T, toml::de::Error>> {
        let raw = self.extra.get(key)?.clone();
        Some(raw.try_into())
    }
}

/// Where a manifest entry's downloaded file lives on disk — the ONE
/// resolver both the download and the process steps use, so they can never
/// disagree.
///
/// - `shared = true` → the global pool, whoever declared it.
/// - Authored `dest` → `<owner cache>/<dest>` (the shared cache for a crate
///   manifest, `<twin>/.cache` for a Twin's).
/// - No `dest` → the owner's source pool, keyed by URL hash.
pub fn entry_dest_path(entry: &AssetEntry, dest_root: Option<&Path>) -> PathBuf {
    // Opt-in: this product is big and reused, put it in the one global pool.
    if entry.shared {
        return shared_source_path(&entry.url);
    }
    // Otherwise everything resolves against the OWNER's cache — the shared
    // cache for an engine manifest, `<twin>/.cache` for a Twin's.
    let root = dest_root.map(Path::to_path_buf).unwrap_or_else(cache_dir);
    match entry.dest.as_deref() {
        Some(d) => root.join(d),
        None => source_pool_path(&root, &entry.url),
    }
}

/// The shared source pool path for a URL:
/// `<cache>/sources/<sha256(url)[..16]>/<basename>`.
///
/// Keyed by URL hash (not just basename) so two products that happen to
/// share a filename never collide; the basename is kept alongside so the
/// pool stays human-readable. Integrity is the manifest's `sha256` — the
/// pool only decides WHERE bytes live, never whether to trust them.
pub fn shared_source_path(url: &str) -> PathBuf {
    source_pool_path(&cache_dir(), url)
}

/// A URL's slot in the source pool UNDER `root`: `<root>/sources/<hash16>/<basename>`.
///
/// One layout, two roots: the shared cache holds the pool for engine assets and
/// for entries that opted into `shared = true`; a Twin's own `.cache` holds the
/// pool for everything that Twin declares. Keying by URL hash (not basename)
/// means two products that share a filename never collide; the basename is kept
/// alongside so the pool stays readable.
pub fn source_pool_path(root: &Path, url: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Basename: last path segment, query-string stripped; anything unsafe
    // (empty, traversal, absolute) falls back to a neutral name — the hash
    // dir already guarantees uniqueness.
    let base = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .filter(|s| !s.is_empty() && is_safe_rel_dest(s))
        .unwrap_or("download.bin");
    root.join("sources").join(&hash[..16]).join(base)
}

/// Parsed `Assets.toml` from a crate.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetManifest {
    #[serde(flatten)]
    pub assets: BTreeMap<String, AssetEntry>,
}

/// Parse an `Assets.toml` blob from a string. Used by callers that have the
/// manifest text embedded via `include_str!` (packaged binaries can't read the
/// workspace source tree at runtime).
///
/// This is the `FromStr` TRAIT rather than an inherent `from_str`: the
/// signature was already exactly the trait's, so an inherent method of that
/// name shadowed `std::str::FromStr::from_str` at every call site and a reader
/// could not tell which one they were getting. Implementing the trait removes
/// the ambiguity and makes `text.parse::<AssetManifest>()` work for free.
impl std::str::FromStr for AssetManifest {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

impl AssetManifest {
    /// Reads and parses a manifest FILE — `assets/manifests/<group>.toml` for
    /// the engine, `<twin>/Assets.toml` for a Twin.
    pub fn from_file(path: &Path) -> Result<Self, std::io::Error> {
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No manifest at {}", path.display()),
            ));
        }
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Reads and parses `Assets.toml` from a **Twin folder**. Twins keep their
    /// manifest at the root of the folder they travel as; only the ENGINE's
    /// declarations moved into `assets/manifests/`.
    pub fn from_crate_dir(crate_dir: &Path) -> Result<Self, std::io::Error> {
        let path = crate_dir.join("Assets.toml");
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No Assets.toml found in {}", crate_dir.display()),
            ));
        }
        let content = std::fs::read_to_string(&path)?;
        toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

/// Downloads an asset from the manifest entry. Equivalent to
/// [`download_asset_with_control`] with no progress callback and no
/// cancellation flag — keeps existing CLI/test call sites unchanged.
///
/// `dest_root` overrides the cache as the base `entry.dest` is resolved
/// against: `None` → shared cache root (the original behaviour); `Some(dir)`
/// → `dir.join(entry.dest)`, which is how a Twin's `Assets.toml` downloads
/// *into* the Twin folder (the CLI's `--twin <DIR>` flag).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_asset(
    entry: &AssetEntry,
    key: &str,
    dest_root: Option<&Path>,
) -> Result<(), DownloadError> {
    download_asset_with_control(entry, key, DownloadControl::default(), dest_root)
}

/// Downloads an asset from the manifest entry with caller-supplied
/// progress reporting and cooperative cancellation.
///
/// 1. Checks if already installed (version + path exist).
/// 2. Streams bytes from the URL, calling `control.progress` per chunk
///    and aborting if `control.cancel` flips to `true`.
/// 3. Verifies or computes SHA-256.
/// 4. Extracts (if tarball) or writes (if single file).
/// 5. Prints the computed SHA-256 for the user to fill in.
///
/// `dest_root` selects the base `entry.dest` resolves against. `None` keeps
/// the historical behaviour (shared cache root via [`crate::cache_dir`]);
/// `Some(dir)` downloads into that folder — used by the Twin download path
/// (`--twin <DIR>`) so a Twin's `Assets.toml` materialises files inside the
/// Twin, where its `demSource` / USD `references` expect to find them.
/// When a `dest_root` is supplied, `entry.dest` is validated to be a
/// strictly relative path with no `..` segments (see [`is_safe_rel_dest`])
/// so a manifest can never escape the Twin root.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_asset_with_control(
    entry: &AssetEntry,
    key: &str,
    mut control: DownloadControl<'_>,
    dest_root: Option<&Path>,
) -> Result<(), DownloadError> {
    // Twin-relative downloads must not let a manifest's `dest` walk outside
    // the Twin root. Cache-relative downloads are plain relative paths.
    if let (Some(_root), Some(d)) = (dest_root, entry.dest.as_deref()) {
        if !is_safe_rel_dest(d) {
            return Err(DownloadError::ManifestFailed(format!(
                "asset `{key}` has an unsafe `dest` for a twin download: {d:?} \
                 (must be relative, no `..`, no absolute, no backslash)"
            )));
        }
    }
    let dest = entry_dest_path(entry, dest_root);

    // Cache-hit check #1 — versioned install (used by libraries like
    // the MSL tarball where `version = "4.1.0"` pins an upstream
    // release). Matches on `.version` marker sibling.
    if dest.exists() {
        if let Some(ref ver) = entry.version {
            let version_file = dest.parent().unwrap_or(&dest).join(".version");
            if version_file.exists() {
                let installed_ver = std::fs::read_to_string(&version_file).unwrap_or_default();
                if installed_ver.trim() == ver.trim() {
                    println!(
                        "  ✓ {} v{} already installed at {}",
                        key,
                        ver,
                        dest.display()
                    );
                    return Ok(());
                }
            }
        }
    }

    // Cache-hit check #2 — sha256 match. When the manifest pins a
    // content hash, trust the existing file if its hash matches. This
    // is what prevents the NASA textures (no `version`, just a
    // `sha256`) from re-downloading tens of megabytes on every run
    // after they've been pinned. Only runs for single-file entries:
    // computing the hash of a `copy_dir_all`-style directory tree
    // would be surprisingly subtle (order sensitivity, hidden files)
    // and isn't worth the complexity here — tarball entries still
    // need the `version` path for cache-hit.
    if dest.is_file() {
        if let Some(ref expected) = entry.sha256 {
            if !expected.is_empty() {
                use sha2::{Digest, Sha256};
                if let Ok(bytes) = std::fs::read(&dest) {
                    let hash: String = Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    if hash == *expected {
                        println!(
                            "  ✓ {} already installed at {} (sha256 match)",
                            key,
                            dest.display()
                        );
                        return Ok(());
                    }
                }
            } else if dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                println!(
                    "  ✓ {} already exists at {} (file exists)",
                    key,
                    dest.display()
                );
                return Ok(());
            }
        } else if dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            println!(
                "  ✓ {} already exists at {} (file exists)",
                key,
                dest.display()
            );
            return Ok(());
        }
    }

    println!("  ↓ downloading {} ({})...", entry.name, entry.url);

    // Cancel probe — caller may have flipped the flag before we even
    // hit the network.
    let cancelled = || {
        control
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    if cancelled() {
        return Err(DownloadError::Cancelled);
    }

    // Download in chunks so progress can tick and cancellation is
    // responsive (within one chunk's read latency).
    let response = ureq::get(&entry.url)
        .call()
        .map_err(|e| DownloadError::DownloadFailed(entry.url.clone(), e.to_string()))?;
    let total: u64 = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    let mut bytes: Vec<u8> = if total > 0 {
        Vec::with_capacity(total as usize)
    } else {
        Vec::new()
    };
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let n = reader
            .read(&mut chunk)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(cb) = control.progress.as_mut() {
            cb(bytes.len() as u64, total);
        }
    }

    // Compute SHA-256
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // Check against expected if provided and non-empty
    if let Some(ref expected) = entry.sha256 {
        if !expected.is_empty() && hash != *expected {
            return Err(DownloadError::HashMismatch(expected.clone(), hash));
        }
    }

    // Tarball detection — `.tar.gz` / `.tgz` (gzip) and `.tar.bz2` /
    // `.tbz2` / `.tbz` (bzip2) both handled. Added bz2 so the
    // upstream DejaVu release on SourceForge can be pulled directly.
    let is_tar_gz = entry.url.ends_with(".tar.gz") || entry.url.ends_with(".tgz");
    let is_tar_bz2 = entry.url.ends_with(".tar.bz2")
        || entry.url.ends_with(".tbz2")
        || entry.url.ends_with(".tbz");
    let is_tar = is_tar_gz || is_tar_bz2;

    if is_tar {
        let temp_dir = std::env::temp_dir().join(format!("lunco_{}", key));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| DownloadError::WriteFailed(temp_dir.clone(), e.to_string()))?;

        let ext = if is_tar_gz { "tar.gz" } else { "tar.bz2" };
        let tar_path = temp_dir.join(format!("asset.{ext}"));
        std::fs::write(&tar_path, &bytes)
            .map_err(|e| DownloadError::WriteFailed(tar_path.clone(), e.to_string()))?;

        let file =
            std::fs::File::open(&tar_path).map_err(|e| DownloadError::ReadFailed(e.to_string()))?;
        // Dispatch to the right decompressor. Both flate2::GzDecoder
        // and bzip2::read::BzDecoder implement `Read`, so the tar
        // unpacker receives a `Box<dyn Read>` either way.
        let reader: Box<dyn std::io::Read> = if is_tar_gz {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(bzip2::read::BzDecoder::new(file))
        };
        let mut archive = tar::Archive::new(reader);
        // Initial "0 extracted" tick so callers can flip phase state
        // before the first entry is unpacked.
        if let Some(cb) = control.extracting.as_mut() {
            cb(0);
        }
        let entries_iter = archive
            .entries()
            .map_err(|e| DownloadError::ExtractFailed(e.to_string()))?;
        let mut extracted: u64 = 0;
        for entry in entries_iter {
            if cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let mut entry = entry.map_err(|e| DownloadError::ExtractFailed(e.to_string()))?;
            entry
                .unpack_in(&temp_dir)
                .map_err(|e| DownloadError::ExtractFailed(e.to_string()))?;
            extracted += 1;
            if extracted.is_multiple_of(64) {
                if let Some(cb) = control.extracting.as_mut() {
                    cb(extracted);
                }
            }
        }
        if let Some(cb) = control.extracting.as_mut() {
            cb(extracted);
        }

        // Find extracted dir
        let entries: Vec<_> = std::fs::read_dir(&temp_dir)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        if entries.is_empty() {
            return Err(DownloadError::ExtractFailed(
                "No directories in tarball".into(),
            ));
        }

        let source_dir = &entries[0].path();

        if let Some(inner) = entry.extract.as_ref() {
            // Single-file extraction mode: pick just the named file
            // from inside the archive, write it to `dest`, discard
            // the rest. `dest` is interpreted as a file path.
            let src_file = source_dir.join(inner);
            if !src_file.is_file() {
                return Err(DownloadError::ExtractFailed(format!(
                    "archive does not contain `{}` (looked in {})",
                    inner,
                    source_dir.display()
                )));
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| DownloadError::WriteFailed(parent.to_path_buf(), e.to_string()))?;
            }
            std::fs::copy(&src_file, &dest)
                .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;
        } else {
            // Whole-archive mode: copy the extracted tree to `dest`.
            if dest.exists() {
                std::fs::remove_dir_all(&dest)
                    .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;
            }
            std::fs::create_dir_all(dest.parent().unwrap_or(&dest))
                .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;

            copy_dir_all(source_dir, &dest)
                .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DownloadError::WriteFailed(parent.to_path_buf(), e.to_string()))?;
        }
        std::fs::write(&dest, &bytes)
            .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;
    }

    // Write version file for future checks
    if let Some(ref ver) = entry.version {
        let version_file = dest.parent().unwrap_or(&dest).join(".version");
        let _ = std::fs::write(version_file, ver);
    }

    println!("  ✓ installed at {}", dest.display());
    if entry.sha256.as_deref().unwrap_or("").is_empty() {
        println!("    sha256 = \"{}\"", hash);
        println!("    (add this to Assets.toml for integrity verification)");
    }

    Ok(())
}

/// Reads the `max_parallel_downloads` limit from settings.json (default: 3).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_download_parallel_limit() -> usize {
    let settings_file = crate::user_config_dir().join("settings.json");
    if let Ok(text) = std::fs::read_to_string(&settings_file) {
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(limit) = raw
                .get("download")
                .and_then(|d| d.get("max_parallel_downloads"))
                .and_then(|v| v.as_u64())
            {
                return (limit as usize).max(1);
            }
        }
    }
    3
}

/// Downloads every asset in one engine manifest group with a parallel download limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_group_with_limit(
    group: &str,
    max_parallel: usize,
) -> Result<(), DownloadError> {
    let path = crate::manifests_dir().join(format!("{group}.toml"));
    let manifest = AssetManifest::from_file(&path)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", path.display());
        return Ok(());
    }

    let limit = max_parallel.max(1);
    println!("Downloading assets for `{group}` (parallel limit: {limit})...");

    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(limit)
        .build()
        .map_err(|e| DownloadError::ManifestFailed(format!("Failed to build thread pool: {e}")))?;

    let errors = std::sync::Mutex::new(Vec::new());
    pool.scope(|s| {
        for (key, entry) in entries {
            let errors = &errors;
            s.spawn(move |_| {
                if let Err(e) = download_asset(&entry, &key, None) {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });

    let errs = errors.into_inner().unwrap();
    if let Some(err) = errs.into_iter().next() {
        return Err(err);
    }

    Ok(())
}

/// Downloads every asset in one engine manifest group
/// (`assets/manifests/<group>.toml`). Resolves each `dest` against the shared
/// cache root — engine declarations are not Twin-owned, so their downloads
/// belong in the machine-wide pool.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_group(group: &str) -> Result<(), DownloadError> {
    download_all_for_group_with_limit(group, load_download_parallel_limit())
}

/// Downloads all assets from a Twin folder's `Assets.toml` with a specified parallel limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_twin_with_limit(
    twin_root: &Path,
    max_parallel: usize,
) -> Result<(), DownloadError> {
    let manifest = AssetManifest::from_crate_dir(twin_root)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    if manifest.assets.is_empty() {
        println!(
            "No assets declared in {}",
            twin_root.join("Assets.toml").display()
        );
        return Ok(());
    }
    let limit = max_parallel.max(1);
    println!(
        "Downloading assets for twin {} (parallel limit: {limit})...",
        twin_root.display()
    );
    let dest_root = crate::twin_cache_dir(twin_root);
    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(limit)
        .build()
        .map_err(|e| DownloadError::ManifestFailed(format!("Failed to build thread pool: {e}")))?;

    let errors = std::sync::Mutex::new(Vec::new());
    pool.scope(|s| {
        for (key, entry) in entries {
            let dest_root = &dest_root;
            let errors = &errors;
            s.spawn(move |_| {
                if let Err(e) = download_asset(&entry, &key, Some(dest_root)) {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });

    let errs = errors.into_inner().unwrap();
    if let Some(err) = errs.into_iter().next() {
        return Err(err);
    }
    Ok(())
}

/// Downloads all assets from a **Twin folder's** `Assets.toml` into that
/// Twin's own cache ([`crate::twin_cache_dir`]), using the parallel download limit
/// configured in settings.json (default: 3).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_twin(twin_root: &Path) -> Result<(), DownloadError> {
    download_all_for_twin_with_limit(twin_root, load_download_parallel_limit())
}

/// Downloads a single asset by key from a **Twin folder's** `Assets.toml` —
/// the `-a KEY` filter composed with `--twin <DIR>`. A twin that manifests
/// every candidate terrain site would otherwise pull multiple GB of DTMs on
/// each provisioning run just to refresh one site.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_one_for_twin(twin_root: &Path, asset_key: &str) -> Result<(), DownloadError> {
    let manifest = AssetManifest::from_crate_dir(twin_root)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;
    match manifest.assets.get(asset_key) {
        Some(entry) => download_asset(entry, asset_key, Some(&crate::twin_cache_dir(twin_root))),
        None => Err(DownloadError::ManifestFailed(format!(
            "no asset `{}` in {}",
            asset_key,
            twin_root.join("Assets.toml").display()
        ))),
    }
}

/// Downloads a single asset by key, searching every engine manifest group.
/// Returns the first match.
///
/// Use case: `cargo run -p lunco-assets -- download -a dejavu_sans`
/// — pulls only the DejaVu font without refetching 20+ MB of NASA
/// textures from an unrelated group.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_one_engine(asset_key: &str) -> Result<(), DownloadError> {
    for (group, path) in crate::engine_manifests() {
        let Ok(manifest) = AssetManifest::from_file(&path) else {
            continue;
        };
        if let Some(entry) = manifest.assets.get(asset_key) {
            println!("Downloading `{asset_key}` from `{group}`...");
            return download_asset(entry, asset_key, None);
        }
    }

    Err(DownloadError::ManifestFailed(format!(
        "asset `{asset_key}` not declared in any manifest under {}",
        crate::manifests_dir().display()
    )))
}

/// Downloads every asset declared by every engine manifest group.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_engine() -> Result<(), DownloadError> {
    for (group, _) in crate::engine_manifests() {
        download_all_for_group(&group)?;
    }
    Ok(())
}

/// Lists all assets in one manifest FILE. `dest_root` selects the base `dest`
/// is probed against (`None` = shared cache; `Some` = that folder) so the
/// status reflects where a download would actually land. `label` names the set
/// in the heading — a group for the engine, the folder for a Twin.
pub fn list_manifest(
    manifest_path: &Path,
    label: &str,
    dest_root: Option<&Path>,
) -> Result<(), std::io::Error> {
    let manifest = AssetManifest::from_file(manifest_path)?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", manifest_path.display());
        return Ok(());
    }

    println!("Assets for {label}:");
    for (key, entry) in &manifest.assets {
        let dest = entry_dest_path(entry, dest_root);
        let status = if dest.exists() {
            if let Some(ref ver) = entry.version {
                let version_file = dest.parent().unwrap_or(&dest).join(".version");
                if version_file.exists() {
                    let installed_ver = std::fs::read_to_string(&version_file).unwrap_or_default();
                    if installed_ver.trim() == ver.trim() {
                        "✓ installed"
                    } else {
                        "⚠ version mismatch"
                    }
                } else {
                    "✓ exists"
                }
            } else {
                "✓ exists"
            }
        } else {
            "✗ not installed"
        };

        let version = entry.version.as_deref().unwrap_or("latest");
        let has_process = if entry.process.is_some() {
            " [process]"
        } else {
            ""
        };
        println!(
            "  {} [{}] {} → {}{}",
            key, version, entry.name, status, has_process
        );
    }

    Ok(())
}

/// Lists all assets from a **Twin folder's** `Assets.toml`, probing `dest`
/// against the Twin root so the status reflects where files land.
pub fn list_for_twin(twin_root: &Path) -> Result<(), std::io::Error> {
    let label = twin_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    list_manifest(
        &twin_root.join("Assets.toml"),
        &label,
        Some(&crate::twin_cache_dir(twin_root)),
    )
}

/// Lists one engine manifest group (`assets/manifests/<group>.toml`).
pub fn list_group(group: &str) -> Result<(), std::io::Error> {
    list_manifest(
        &crate::manifests_dir().join(format!("{group}.toml")),
        group,
        None,
    )
}

/// A `dest` path is "twin-safe" iff it is strictly relative, contains no
/// `..` or root/ prefix component, and uses no backslash — i.e. joining it
/// to a Twin root can never escape that root. Mirrors the traversal guard
/// `scenario_sync::safe_rel_path` applies to downloaded-scenario paths, so
/// a Twin's `Assets.toml` is held to the same standard as the network
/// download layer.
pub fn is_safe_rel_dest(dest: &str) -> bool {
    if dest.is_empty() || dest.contains('\\') {
        return false;
    }
    // Build from the string directly (not `Path::components`, which on this
    // target would normalise `..` away) — we want to SEE the `..`.
    for seg in dest.split(['/', '\\']) {
        if seg.is_empty() || seg == "." || seg == ".." {
            return false;
        }
    }
    // Reject absolute paths. `Path::is_absolute` covers Unix roots and
    // Windows UNC on Windows, but on *this* target a Windows drive path
    // like `C:/Users/x` is NOT absolute (Linux has no drive concept) — so
    // also reject any leading `X:` drive-letter form, regardless of host.
    if Path::new(dest).is_absolute() {
        return false;
    }
    let bytes = dest.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    true
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Failed to read manifest: {0}")]
    ManifestFailed(String),
    #[error("Failed to download {0}: {1}")]
    DownloadFailed(String, String),
    #[error("Failed to read response: {0}")]
    ReadFailed(String),
    #[error("Failed to write to {0}: {1}")]
    WriteFailed(PathBuf, String),
    #[error("Failed to extract archive: {0}")]
    ExtractFailed(String),
    #[error("SHA-256 mismatch: expected {0}, got {1}")]
    HashMismatch(String, String),
    #[error("cancelled by caller")]
    Cancelled,
}

/// Caller-supplied control surface for a download. Three optional
/// signals: HTTP read progress, tar-extraction progress, and a cancel
/// flag. All default to "do nothing" so callers opt in to each
/// independently.
///
/// - `progress` runs from the read loop on every chunk (~64 KiB) with
///   `(bytes_done, bytes_total)`. `bytes_total = 0` means the server
///   didn't advertise Content-Length.
/// - `extracting` runs from the tar walk every few entries with
///   `entries_done`. Total file count is not known up-front (we
///   stream the archive), so callers should display a count or a
///   spinner rather than a percentage. Fires once with `0` before
///   the first entry so callers can flip phase state.
/// - `cancel` is checked between chunks during download and between
///   entries during extract; flipping it to `true` aborts with
///   [`DownloadError::Cancelled`].
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct DownloadControl<'a> {
    /// Called as bytes stream in. Keep the closure cheap — it runs on
    /// the read loop's hot path.
    pub progress: Option<Box<dyn FnMut(u64, u64) + Send + 'a>>,
    /// Called while a tarball is being unpacked. Argument is the
    /// running count of unpacked entries.
    pub extracting: Option<Box<dyn FnMut(u64) + Send + 'a>>,
    /// Cancellation flag shared with the caller.
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_dest_accepts_plain_relative() {
        assert!(is_safe_rel_dest("terrain/apollo15/.cache/dtm.tif"));
        assert!(is_safe_rel_dest("textures/moon.png"));
        assert!(is_safe_rel_dest("fonts/DejaVuSans.ttf"));
    }

    #[test]
    fn safe_rel_dest_rejects_traversal_and_absolute() {
        // Parent escape — the whole point of the guard.
        assert!(!is_safe_rel_dest("../escape.tif"));
        assert!(!is_safe_rel_dest("terrain/../../escape.tif"));
        assert!(!is_safe_rel_dest("a/../b/../../x"));
        // Absolute (Unix + Windows drive).
        assert!(!is_safe_rel_dest("/etc/passwd"));
        assert!(!is_safe_rel_dest("C:/Users/x"));
        // Backslash is a traversal vector on Windows; reject everywhere.
        assert!(!is_safe_rel_dest("terrain\\..\\x"));
        // Empty / leading-slash-adjacent.
        assert!(!is_safe_rel_dest(""));
        assert!(!is_safe_rel_dest("."));
        assert!(!is_safe_rel_dest(".."));
    }

    /// A `dest_root = Some(twin)` download that fails the traversal guard
    /// must error *before* touching the network — the manifest's `url` is a
    /// bogus local string so a real fetch would also fail, but the guard is
    /// the thing under test and it fires first.
    #[test]
    fn twin_download_rejects_unsafe_dest_without_network() {
        let entry = AssetEntry {
            name: "evil".into(),
            version: None,
            url: "http://0.0.0.0:0/never-fetched".into(),
            dest: Some("../escape.tif".into()),
            extract: None,
            shared: false,
            sha256: None,
            process: None,
            extra: Default::default(),
        };
        let err = download_asset(&entry, "evil", Some(std::path::Path::new("/tmp")))
            .expect_err("traversal must be rejected");
        assert!(matches!(err, DownloadError::ManifestFailed(_)));
    }

    /// The default is the OWNER's cache; `shared = true` is the opt-out.
    #[test]
    fn a_twins_download_lands_in_that_twins_cache_unless_it_opts_into_the_shared_pool() {
        let twin_cache = crate::twin_cache_dir(std::path::Path::new("/tmp/twin"));
        let mut entry = AssetEntry {
            name: "dtm".into(),
            version: None,
            url: "https://example.invalid/NAC_DTM.TIF".into(),
            dest: None,
            extract: None,
            shared: false,
            sha256: None,
            process: None,
            extra: Default::default(),
        };

        // Default: twin-local pool — the Twin stays self-contained.
        let local = entry_dest_path(&entry, Some(&twin_cache));
        assert!(
            local.starts_with(&twin_cache),
            "expected the twin cache, got {}",
            local.display()
        );
        assert!(local.ends_with("NAC_DTM.TIF"));

        // Opt-in: the one global pool, whatever root was offered.
        entry.shared = true;
        let shared = entry_dest_path(&entry, Some(&twin_cache));
        assert!(
            !shared.starts_with(&twin_cache) && shared.starts_with(cache_dir()),
            "shared = true must reach the global pool, got {}",
            shared.display()
        );

        // An authored `dest` is still twin-relative.
        entry.shared = false;
        entry.dest = Some("terrain/apollo15/dtm.tif".into());
        assert_eq!(
            entry_dest_path(&entry, Some(&twin_cache)),
            twin_cache.join("terrain/apollo15/dtm.tif")
        );
    }

    /// Sanity-check that `list_manifest` honours `dest_root` so `--twin`
    /// reports against the Twin folder, not the cache. We can't exercise a
    /// real `Assets.toml` without a fixture, but the path-join is the only
    /// behaviour the twin path adds to `list`, so assert the resolved probe
    /// dir matches the twin root for a synthetic manifest.
    #[test]
    fn list_for_twin_probes_twin_root() {
        // Build a throwaway twin folder with an Assets.toml.
        let tmp = std::env::temp_dir().join(format!("lunco-assets-twin-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(
            tmp.join("Assets.toml"),
            "[x]\nname = \"X\"\nurl = \"http://x/x\"\ndest = \"terrain/x.tif\"\n",
        )
        .unwrap();
        // Not downloaded yet → "not installed", but the function must not
        // panic and must complete (i.e. dest_root was accepted).
        let res = list_manifest(&tmp.join("Assets.toml"), "twin", Some(&tmp));
        assert!(res.is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
