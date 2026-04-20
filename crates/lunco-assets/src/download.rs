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

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use serde::Deserialize;
use crate::{cache_dir, process::ProcessConfig};

/// A single asset entry from `Assets.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntry {
    /// Human-readable name.
    pub name: String,
    /// Semantic version (for libraries). Changes trigger re-download.
    pub version: Option<String>,
    /// URL to download from.
    pub url: String,
    /// Destination path relative to the shared cache root.
    ///
    /// For tarballs without `extract`: the archive is extracted into
    /// this directory.
    ///
    /// For tarballs WITH `extract`: only the named file inside the
    /// archive is copied to this path (`dest` becomes the final
    /// output file, not a directory).
    ///
    /// For single-file downloads: the bytes are written directly here.
    pub dest: String,
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
    /// Expected SHA-256 hex digest. Empty string means "compute and suggest".
    pub sha256: Option<String>,
    /// Optional post-processing step (resize, convert).
    #[serde(default)]
    pub process: Option<ProcessConfig>,
}

/// Parsed `Assets.toml` from a crate.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetManifest {
    #[serde(flatten)]
    pub assets: BTreeMap<String, AssetEntry>,
}

impl AssetManifest {
    /// Reads and parses `Assets.toml` from the given crate directory.
    pub fn from_crate_dir(crate_dir: &Path) -> Result<Self, std::io::Error> {
        let path = crate_dir.join("Assets.toml");
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No Assets.toml found in {}", crate_dir.display()),
            ));
        }
        let content = std::fs::read_to_string(&path)?;
        toml::from_str(&content).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }
}

/// Downloads an asset from the manifest entry.
///
/// 1. Checks if already installed (version + path exist).
/// 2. Downloads to temp file.
/// 3. Verifies or computes SHA-256.
/// 4. Extracts (if tarball) or writes (if single file).
/// 5. Prints the computed SHA-256 for the user to fill in.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_asset(entry: &AssetEntry, key: &str) -> Result<(), DownloadError> {
    let dest = cache_dir().join(&entry.dest);

    // Cache-hit check #1 — versioned install (used by libraries like
    // the MSL tarball where `version = "4.1.0"` pins an upstream
    // release). Matches on `.version` marker sibling.
    if dest.exists() {
        if let Some(ref ver) = entry.version {
            let version_file = dest.parent().unwrap_or(&dest).join(".version");
            if version_file.exists() {
                let installed_ver = std::fs::read_to_string(&version_file).unwrap_or_default();
                if installed_ver.trim() == ver.trim() {
                    println!("  ✓ {} v{} already installed at {}", key, ver, dest.display());
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
                    let hash = format!("{:x}", Sha256::digest(&bytes));
                    if hash == *expected {
                        println!(
                            "  ✓ {} already installed at {} (sha256 match)",
                            key,
                            dest.display()
                        );
                        return Ok(());
                    }
                }
            }
        }
    }

    println!("  ↓ downloading {} ({})...", entry.name, entry.url);

    // Download
    let response = ureq::get(&entry.url).call()
        .map_err(|e| DownloadError::DownloadFailed(entry.url.clone(), e.to_string()))?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes)
        .map_err(|e| DownloadError::ReadFailed(e.to_string()))?;

    // Compute SHA-256
    use sha2::{Sha256, Digest};
    let hash = format!("{:x}", Sha256::digest(&bytes));

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

        let file = std::fs::File::open(&tar_path)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?;
        // Dispatch to the right decompressor. Both flate2::GzDecoder
        // and bzip2::read::BzDecoder implement `Read`, so the tar
        // unpacker receives a `Box<dyn Read>` either way.
        let reader: Box<dyn std::io::Read> = if is_tar_gz {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(bzip2::read::BzDecoder::new(file))
        };
        let mut archive = tar::Archive::new(reader);
        archive.unpack(&temp_dir)
            .map_err(|e| DownloadError::ExtractFailed(e.to_string()))?;

        // Find extracted dir
        let entries: Vec<_> = std::fs::read_dir(&temp_dir)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        if entries.is_empty() {
            return Err(DownloadError::ExtractFailed("No directories in tarball".into()));
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
                std::fs::create_dir_all(parent).map_err(|e| {
                    DownloadError::WriteFailed(parent.to_path_buf(), e.to_string())
                })?;
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

/// Downloads all assets from the given crate's `Assets.toml`.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_crate(crate_dir: &Path) -> Result<(), DownloadError> {
    let manifest = AssetManifest::from_crate_dir(crate_dir)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", crate_dir.join("Assets.toml").display());
        return Ok(());
    }

    println!("Downloading assets for {}...", crate_dir.file_name().unwrap_or_default().to_string_lossy());

    for (key, entry) in &manifest.assets {
        download_asset(entry, key)?;
    }

    Ok(())
}

/// Downloads a single asset by key, searching every crate's
/// `Assets.toml` across the workspace. Returns the first match.
///
/// Use case: `cargo run -p lunco-assets -- download -a dejavu_sans`
/// — pulls only the DejaVu font without refetching 20+ MB of NASA
/// textures from an unrelated crate.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_one_workspace(
    workspace_root: &Path,
    asset_key: &str,
) -> Result<(), DownloadError> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;
    let workspace: toml::Value = toml::from_str(&content)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    let members = workspace["workspace"]["members"]
        .as_array()
        .ok_or_else(|| {
            DownloadError::ManifestFailed("No workspace.members in Cargo.toml".into())
        })?;

    for member in members {
        let Some(path) = member.as_str() else { continue };
        let crate_dir = workspace_root.join(path);
        let manifest_path = crate_dir.join("Assets.toml");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = match AssetManifest::from_crate_dir(&crate_dir) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Some(entry) = manifest.assets.get(asset_key) {
            println!(
                "Downloading `{}` from {}...",
                asset_key,
                crate_dir.file_name().unwrap_or_default().to_string_lossy()
            );
            return download_asset(entry, asset_key);
        }
    }

    Err(DownloadError::ManifestFailed(format!(
        "asset `{asset_key}` not found in any Assets.toml across the workspace"
    )))
}

/// Downloads all assets from every crate in the workspace.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_workspace(workspace_root: &Path) -> Result<(), DownloadError> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;
    let workspace: toml::Value = toml::from_str(&content)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    let members = workspace["workspace"]["members"]
        .as_array()
        .ok_or_else(|| DownloadError::ManifestFailed("No workspace.members in Cargo.toml".into()))?;

    for member in members {
        if let Some(path) = member.as_str() {
            let crate_dir = workspace_root.join(path);
            if crate_dir.join("Assets.toml").exists() {
                download_all_for_crate(&crate_dir)?;
            }
        }
    }

    Ok(())
}

/// Lists all assets from a crate's `Assets.toml`.
pub fn list_for_crate(crate_dir: &Path) -> Result<(), std::io::Error> {
    let manifest = AssetManifest::from_crate_dir(crate_dir)?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", crate_dir.join("Assets.toml").display());
        return Ok(());
    }

    println!("Assets for {}:", crate_dir.file_name().unwrap_or_default().to_string_lossy());
    for (key, entry) in &manifest.assets {
        let dest = cache_dir().join(&entry.dest);
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
        let has_process = if entry.process.is_some() { " [process]" } else { "" };
        println!("  {} [{}] {} → {}{}", key, version, entry.name, status, has_process);
    }

    Ok(())
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
