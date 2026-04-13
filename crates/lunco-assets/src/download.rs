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
    /// For tarballs: extracted here.
    /// For single files: written directly.
    pub dest: String,
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

    // Check if already installed with correct version
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

    // Determine if tarball
    let is_tar = entry.url.ends_with(".tar.gz") || entry.url.ends_with(".tgz");

    if is_tar {
        let temp_dir = std::env::temp_dir().join(format!("lunco_{}", key));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| DownloadError::WriteFailed(temp_dir.clone(), e.to_string()))?;

        let tar_gz_path = temp_dir.join("asset.tar.gz");
        std::fs::write(&tar_gz_path, &bytes)
            .map_err(|e| DownloadError::WriteFailed(tar_gz_path.clone(), e.to_string()))?;

        let tar_gz = std::fs::File::open(&tar_gz_path)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?;
        let tar = flate2::read::GzDecoder::new(tar_gz);
        let mut archive = tar::Archive::new(tar);
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
        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;
        }
        std::fs::create_dir_all(dest.parent().unwrap_or(&dest))
            .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;

        copy_dir_all(source_dir, &dest)
            .map_err(|e| DownloadError::WriteFailed(dest.clone(), e.to_string()))?;

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
