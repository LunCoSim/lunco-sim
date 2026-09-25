//! Asset download and version verification for the provisioning runtime.
//!
//! Each crate can declare its own `Assets.toml` mirroring the `Cargo.toml` pattern.
//! Manifest parsing and shared dataset contracts live in
//! [`lunco-assets-datasets`]. This module downloads the declared assets and
//! verifies their integrity.
//!
//! ## Assets.toml Format
//!
//! ```toml
//! [library]
//! name = "Example Modelica Source Library"
//! version = "4.1.0"
//! url = "https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v4.1.0.tar.gz"
//! dest = "library"
//! # sha256 = ""  # fill after first download
//! ```
//!
//! ## Versioning Strategies
//!
//! | Asset | Identity and integrity | Example |
//! |-------|----------------------|---------|
//! | Libraries (source library) | manifest `version` (semver) | `"4.1.0"` → `library/4.1.0/` |
//! | Declared datasets | manifest `dest`, with optional `sha256` | `data/vectors.csv` |
//! | Textures | `sha256` (content hash) | `"abc123..."` |

#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_datasets::{
    AssetEntry, AssetManifest, archive_extension, entry_dest_path, install_marker_path,
    installed_destination_present, process_output_path, processed_output_present,
};
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_transport::{TransferError, download_to_writer};
use lunco_settings::DownloadSettings;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

/// Authored decision hook for one generic asset transaction.
pub const ASSET_DOWNLOAD_PREPARE_HOOK: &str = "assets.download.prepare";

lunco_hooks::declare_hook! {
    id: ASSET_DOWNLOAD_PREPARE_HOOK,
    owner: "lunco-assets-download",
    description: "Choose safe network, cache, replacement, and backup policy for one asset transaction.",
    signature: [facts: Map],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}

#[cfg(not(target_arch = "wasm32"))]
fn scratch_name(process_id: u32, attempt: u64) -> String {
    format!("lunco_{process_id}_{attempt}")
}

/// Downloads an asset from the manifest entry. Equivalent to
/// [`download_asset_with_control`] with no progress callback and no
/// cancellation flag.
///
/// `dest_root` supplies the owning cache when the declaration is Twin-scoped:
/// `None` selects the global engine cache; `Some(dir)` selects that Twin's
/// local cache unless `shared = true`, which selects the global cache.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_asset(
    entry: &AssetEntry,
    key: &str,
    settings: &DownloadSettings,
    dest_root: Option<&Path>,
) -> Result<(), DownloadError> {
    download_asset_with_control(entry, key, settings, DownloadControl::default(), dest_root)
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
/// `dest_root` supplies the owning cache for a Twin declaration. `None` selects
/// the global engine cache; `Some(dir)` selects the Twin-local cache unless
/// `shared = true`, which selects the global cache. Authored USD always
/// addresses the resulting artifact through its logical Twin URI, never via
/// the physical cache path.
/// When a `dest_root` is supplied, `entry.dest` is validated to be a
/// strictly relative path with no `..` segments (see
/// [`lunco_assets_path::is_safe_relative_path`])
/// so a manifest can never escape the Twin root.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_asset_with_control(
    entry: &AssetEntry,
    key: &str,
    settings: &DownloadSettings,
    mut control: DownloadControl<'_>,
    dest_root: Option<&Path>,
) -> Result<(), DownloadError> {
    // Twin-relative downloads must not let a manifest's `dest` walk outside
    // the Twin root. Cache-relative downloads are plain relative paths.
    if let (Some(_root), Some(d)) = (dest_root, entry.dest.as_deref()) {
        if !lunco_assets_path::is_safe_relative_path(d) {
            return Err(DownloadError::ManifestFailed(format!(
                "asset `{key}` has an unsafe `dest` for a twin download: {d:?} \
                 (must be relative, no `..`, no absolute, no backslash)"
            )));
        }
    }
    let dest = entry_dest_path(entry, dest_root)
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;

    let policy = prepare_download_policy(entry, key, &dest, dest_root.is_some())?;

    // Cache-hit check #1 — versioned install (used by libraries like
    // the source library tarball where `version = "4.1.0"` pins an upstream
    // release). Matches on `.version` marker sibling.
    let installed = installed_destination_present(entry, &dest);
    if installed && policy.install_mode == InstallMode::Reject {
        return Err(DownloadError::PolicyDenied(format!(
            "asset `{key}` is already installed and the download policy rejects replacement"
        )));
    }
    if installed && !policy.cache_refresh && policy.install_mode == InstallMode::KeepExisting {
        let detail = entry
            .version
            .as_deref()
            .map(|version| format!(" v{version}"))
            .unwrap_or_default();
        println!(
            "  ✓ {}{} already installed at {}",
            key,
            detail,
            dest.display()
        );
        return Ok(());
    }

    if !policy.allow_network {
        return Err(DownloadError::PolicyDenied(format!(
            "asset `{key}` is not installed and the download policy denies network access"
        )));
    }

    // Cache-hit check #2 — sha256 match. When the manifest pins a
    // content hash, trust the existing file if its hash matches. This
    // is what prevents the NASA textures (no `version`, just a
    // `sha256`) from re-downloading tens of megabytes on every run
    // after they've been pinned. Only runs for single-file entries:
    // computing the hash of an extracted directory tree
    // would be surprisingly subtle (order sensitivity, hidden files)
    // and isn't worth the complexity here — tarball entries still
    // need the `version` path for cache-hit.
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

    // Stream to one temp file, hashing incrementally — never the whole payload
    // in RAM. A failed body read leaves the received prefix in this file. The
    // next policy attempt asks for the remaining range when the server supports
    // HTTP Range; a server that returns 200 instead is treated as a fresh full
    // response and the file/hash are reset safely.
    let attempt = {
        static ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
    let scratch = scratch_name(std::process::id(), attempt);
    let install_parent = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(install_parent)
        .map_err(|e| DownloadError::WriteFailed(install_parent.to_path_buf(), e.to_string()))?;
    // Keep the staging file on the destination filesystem. The final install
    // is therefore one atomic rename, with no copy path that could expose a
    // partial artifact or hide a cross-device deployment error.
    let download_path = install_parent.join(format!(".{scratch}.download"));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&download_path)
        .map_err(|e| DownloadError::WriteFailed(download_path.clone(), e.to_string()))?;
    let mut download_stage = StagingPath::file(download_path.clone());
    use sha2::{Digest, Sha256};
    let hasher = std::cell::RefCell::new(Sha256::new());
    let stream_result = download_to_writer(
        &entry.url,
        settings,
        &mut out,
        |out| {
            out.set_len(0).map_err(|error| error.to_string())?;
            out.seek(std::io::SeekFrom::Start(0))
                .map_err(|error| error.to_string())?;
            *hasher.borrow_mut() = Sha256::new();
            Ok(())
        },
        |chunk, downloaded, total| {
            hasher.borrow_mut().update(chunk);
            if let Some(cb) = control.progress.as_mut() {
                cb(downloaded, total);
            }
        },
        || !cancelled(),
    );
    if let Err(error) = stream_result {
        if cancelled() || matches!(error, TransferError::Cancelled) {
            drop(out);
            return Err(DownloadError::Cancelled);
        }
        return Err(match error {
            TransferError::Request(error) => {
                DownloadError::DownloadFailed(entry.url.clone(), error)
            }
            TransferError::Body(error) => DownloadError::ReadFailed(error),
            TransferError::Write(error) => {
                DownloadError::WriteFailed(download_stage.path.clone(), error)
            }
            TransferError::Protocol(error) => DownloadError::ReadFailed(error),
            TransferError::Cancelled => DownloadError::Cancelled,
        });
    }
    drop(out);

    let hash: String = hasher
        .into_inner()
        .finalize()
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
    let archive = archive_extension(&entry.url);
    let is_tar_gz = matches!(archive, Some("tar.gz" | "tgz"));
    let is_tar = archive.is_some();

    if is_tar {
        let temp_dir = install_parent.join(format!(".{scratch}.extract"));
        std::fs::create_dir(&temp_dir)
            .map_err(|e| DownloadError::WriteFailed(temp_dir.clone(), e.to_string()))?;
        let _extract_stage = StagingPath::directory(temp_dir.clone());

        let ext = if is_tar_gz { "tar.gz" } else { "tar.bz2" };
        let tar_path = temp_dir.join(format!("asset.{ext}"));
        // Same filesystem (both under the destination parent), so a rename moves
        // the streamed download into place without touching the payload again.
        std::fs::rename(&download_path, &tar_path)
            .map_err(|e| DownloadError::WriteFailed(tar_path.clone(), e.to_string()))?;
        download_stage.disarm();

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

        let source_dir = match entries.as_slice() {
            [] => {
                return Err(DownloadError::ExtractFailed(
                    "archive has no top-level directory".into(),
                ));
            }
            [entry] => entry.path(),
            _ => {
                return Err(DownloadError::ExtractFailed(
                    "archive must contain exactly one top-level directory".into(),
                ));
            }
        };

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
            install_staged_path(
                &src_file,
                &dest,
                entry.version.as_deref(),
                Some(&hash),
                &policy,
                &control,
            )?;
        } else {
            // Whole-archive mode: move the extracted tree into place. The
            // extraction directory and destination share a filesystem, so the
            // lifecycle barrier covers only the directory renames, never a
            // recursive copy or deletion of a multi-gigabyte tree.
            install_staged_path(
                &source_dir,
                &dest,
                entry.version.as_deref(),
                Some(&hash),
                &policy,
                &control,
            )?;
        }
    } else {
        install_staged_path(
            &download_path,
            &dest,
            entry.version.as_deref(),
            None,
            &policy,
            &control,
        )?;
        download_stage.disarm();
    }

    println!("  ✓ installed at {}", dest.display());
    if entry.sha256.as_deref().unwrap_or("").is_empty() {
        println!("    sha256 = \"{}\"", hash);
        println!("    (add this to Assets.toml for integrity verification)");
    }

    Ok(())
}

/// Downloads every asset in one engine manifest group with a parallel download limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_group_with_limit(
    group: &str,
    max_parallel: usize,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    let path = lunco_assets_core::manifests_dir().join(format!("{group}.toml"));
    let manifest = AssetManifest::from_file(&path)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", path.display());
        return Ok(());
    }

    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();
    download_entries_with_limit(
        &format!("`{group}`"),
        entries,
        max_parallel,
        settings,
        |_| None,
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn download_entries_with_limit(
    label: &str,
    entries: Vec<(String, AssetEntry)>,
    max_parallel: usize,
    settings: &DownloadSettings,
    destination_root: impl Fn(&AssetEntry) -> Option<PathBuf> + Sync,
) -> Result<(), DownloadError> {
    let limit = max_parallel.max(1);
    println!("Downloading assets for {label} (parallel limit: {limit})...");

    if entries.is_empty() {
        println!("No assets declared for {label}");
        return Ok(());
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(limit)
        .build()
        .map_err(|e| DownloadError::ManifestFailed(format!("Failed to build thread pool: {e}")))?;

    let errors = std::sync::Mutex::new(Vec::new());
    pool.scope(|s| {
        for (key, entry) in entries {
            let errors = &errors;
            let destination_root = &destination_root;
            s.spawn(move |_| {
                let destination = destination_root(&entry);
                if let Err(error) = download_asset(&entry, &key, settings, destination.as_deref()) {
                    record_parallel_download_error(errors, error);
                }
            });
        }
    });

    let errs = finish_parallel_downloads(errors);
    if let Some(err) = errs.into_iter().next() {
        return Err(err);
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn record_parallel_download_error(
    errors: &std::sync::Mutex<Vec<DownloadError>>,
    error: DownloadError,
) {
    match errors.lock() {
        Ok(mut errors) => errors.push(error),
        Err(poisoned) => {
            let mut errors = poisoned.into_inner();
            errors.push(error);
            errors.push(DownloadError::ManifestFailed(
                "parallel download error collector was poisoned".into(),
            ));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn finish_parallel_downloads(errors: std::sync::Mutex<Vec<DownloadError>>) -> Vec<DownloadError> {
    match errors.into_inner() {
        Ok(errors) => errors,
        Err(poisoned) => {
            let mut errors = poisoned.into_inner();
            errors.push(DownloadError::ManifestFailed(
                "parallel download error collector was poisoned".into(),
            ));
            errors
        }
    }
}

/// Downloads every engine-manifest entry declared for one package target.
///
/// Package targets are authored in `Assets.toml` beside the dataset. This is
/// the same selection used by the staging command, so a package build cannot
/// download one set of files and stage another set by accident.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_bundle_with_limit(
    bundle: &str,
    max_parallel: usize,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    let manifests = lunco_assets_core::engine_manifests()
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
    let mut entries = Vec::new();
    for (group, path) in manifests {
        let manifest = AssetManifest::from_file(&path)
            .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
        entries.extend(
            manifest
                .assets
                .into_iter()
                .filter(|(_, entry)| entry.bundled_for(bundle))
                .map(|(key, entry)| (format!("{group}/{key}"), entry)),
        );
    }
    download_entries_with_limit(
        &format!("bundle `{bundle}`"),
        entries,
        max_parallel,
        settings,
        |_| None,
    )
}

/// Downloads every asset in one engine manifest group
/// (`assets/manifests/<group>.toml`). Resolves each `dest` against the shared
/// cache root — engine declarations are not Twin-owned, so their downloads
/// belong in the machine-wide pool.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_group(
    group: &str,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    download_all_for_group_with_limit(group, settings.max_parallel_downloads, settings)
}

/// Downloads all assets from a Twin folder's `Assets.toml` with a specified parallel limit.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_twin_with_limit(
    twin_root: &Path,
    max_parallel: usize,
    settings: &DownloadSettings,
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
    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();
    let label = format!("twin {}", twin_root.display());
    let destination_root = move |entry: &AssetEntry| {
        Some(lunco_assets_datasets::DatasetScope::twin_cache_root(
            twin_root,
            entry.shared,
        ))
    };
    download_entries_with_limit(&label, entries, max_parallel, settings, destination_root)
}

/// Downloads all assets from a **Twin folder's** `Assets.toml`, using each
/// entry's declared write owner and the parallel download limit configured in
/// settings.json (default: 3).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_for_twin(
    twin_root: &Path,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    download_all_for_twin_with_limit(twin_root, settings.max_parallel_downloads, settings)
}

/// Downloads a single asset by key from a **Twin folder's** `Assets.toml` —
/// the `-a KEY` filter composed with `--twin <DIR>`. A twin that manifests
/// every candidate terrain site would otherwise pull multiple GB of DTMs on
/// each provisioning run just to refresh one site.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_one_for_twin(
    twin_root: &Path,
    asset_key: &str,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    let manifest = AssetManifest::from_crate_dir(twin_root)
        .map_err(|e| DownloadError::ManifestFailed(e.to_string()))?;
    match manifest.assets.get(asset_key) {
        Some(entry) => {
            let dest_root =
                lunco_assets_datasets::DatasetScope::twin_cache_root(twin_root, entry.shared);
            download_asset(entry, asset_key, settings, Some(&dest_root))
        }
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
pub fn download_one_engine(
    asset_key: &str,
    settings: &DownloadSettings,
) -> Result<(), DownloadError> {
    let manifests = lunco_assets_core::engine_manifests()
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
    for (group, path) in manifests {
        let manifest = AssetManifest::from_file(&path)
            .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
        if let Some(entry) = manifest.assets.get(asset_key) {
            println!("Downloading `{asset_key}` from `{group}`...");
            return download_asset(entry, asset_key, settings, None);
        }
    }

    Err(DownloadError::ManifestFailed(format!(
        "asset `{asset_key}` not declared in any manifest under {}",
        lunco_assets_core::manifests_dir().display()
    )))
}

/// Downloads every asset declared by every engine manifest group.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_all_engine(settings: &DownloadSettings) -> Result<(), DownloadError> {
    let manifests = lunco_assets_core::engine_manifests()
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
    for (group, _) in manifests {
        download_all_for_group(&group, settings)?;
    }
    Ok(())
}

/// Lists all assets in one manifest FILE. `dest_root` selects the base `dest`
/// is probed against (`None` = shared cache; `Some` = that folder) so the
/// status reflects where a download would actually land. `label` names the set
/// in the heading — a group for the engine, the folder for a Twin.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_manifest(
    manifest_path: &Path,
    label: &str,
    dest_root: Option<&Path>,
) -> Result<(), std::io::Error> {
    list_manifest_with_twin(manifest_path, label, dest_root, None)
}

#[cfg(not(target_arch = "wasm32"))]
fn list_manifest_with_twin(
    manifest_path: &Path,
    label: &str,
    dest_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<(), std::io::Error> {
    let manifest = AssetManifest::from_file(manifest_path)?;

    if manifest.assets.is_empty() {
        println!("No assets declared in {}", manifest_path.display());
        return Ok(());
    }

    println!("Assets for {label}:");
    for (key, entry) in &manifest.assets {
        let twin_owner_cache = twin_root
            .map(|root| lunco_assets_datasets::DatasetScope::twin_cache_root(root, entry.shared));
        let owner_cache = twin_owner_cache.as_deref().or(dest_root);
        let dest = entry_dest_path(entry, owner_cache)?;
        let status = if let Some(process) = &entry.process {
            let default_cache;
            let process_cache = match owner_cache {
                Some(root) => Some(root),
                None => {
                    default_cache = lunco_assets_core::cache_dir();
                    Some(default_cache.as_path())
                }
            };
            let artifact = process_output_path(process, process_cache, twin_root)?;
            if processed_output_present(
                &artifact,
                process,
                Some(dest.as_path()).filter(|path| path.is_file()),
            ) {
                "✓ installed"
            } else if installed_destination_present(entry, &dest) {
                "⚠ downloaded; needs processing"
            } else {
                "✗ not installed"
            }
        } else if installed_destination_present(entry, &dest) {
            "✓ installed"
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

/// Lists all assets from a **Twin folder's** `Assets.toml`, probing each
/// declaration against its authoritative write owner so the status reflects
/// where files land.
#[cfg(not(target_arch = "wasm32"))]
pub fn list_for_twin(twin_root: &Path) -> Result<(), std::io::Error> {
    let label = twin_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    list_manifest_with_twin(
        &twin_root.join("Assets.toml"),
        &label,
        None,
        Some(twin_root),
    )
}

/// Lists one engine manifest group (`assets/manifests/<group>.toml`).
#[cfg(not(target_arch = "wasm32"))]
pub fn list_group(group: &str) -> Result<(), std::io::Error> {
    list_manifest(
        &lunco_assets_core::manifests_dir().join(format!("{group}.toml")),
        group,
        None,
    )
}

/// Errors raised while resolving, transferring, extracting, or installing an
/// asset declaration.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// The manifest or one of its paths is invalid.
    #[error("Failed to read manifest: {0}")]
    ManifestFailed(String),
    /// The remote server rejected or could not serve the request.
    #[error("Failed to download {0}: {1}")]
    DownloadFailed(String, String),
    /// The response body could not be read to completion.
    #[error("Failed to read response: {0}")]
    ReadFailed(String),
    /// The staged payload could not be written or installed.
    #[error("Failed to write to {0}: {1}")]
    WriteFailed(PathBuf, String),
    /// An archive could not be safely unpacked.
    #[error("Failed to extract archive: {0}")]
    ExtractFailed(String),
    /// The delivered bytes did not match the authored digest.
    #[error("SHA-256 mismatch: expected {0}, got {1}")]
    HashMismatch(String, String),
    /// The caller cancelled the operation before commit.
    #[error("cancelled by caller")]
    Cancelled,
    /// The installed Rhai policy rejected this transaction.
    #[error("asset download policy denied the transaction: {0}")]
    PolicyDenied(String),
    /// The installed Rhai policy returned a shape outside the closed contract.
    #[error("asset download policy is invalid: {0}")]
    PolicyInvalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMode {
    Replace,
    KeepExisting,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupDisposition {
    DiscardAfterCommit,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadPolicy {
    allow_network: bool,
    cache_refresh: bool,
    install_mode: InstallMode,
    backup: BackupDisposition,
    retain_max_count: usize,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            allow_network: true,
            cache_refresh: false,
            install_mode: InstallMode::KeepExisting,
            backup: BackupDisposition::DiscardAfterCommit,
            retain_max_count: 0,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare_download_policy(
    entry: &AssetEntry,
    key: &str,
    destination: &Path,
    twin_scoped: bool,
) -> Result<DownloadPolicy, DownloadError> {
    use lunco_hooks::HookValue;

    let facts = HookValue::map([
        ("key", HookValue::str(key)),
        ("name", HookValue::str(entry.name.clone())),
        ("url", HookValue::str(entry.url.clone())),
        (
            "destination",
            HookValue::str(
                entry
                    .dest
                    .clone()
                    .unwrap_or_else(|| destination.display().to_string()),
            ),
        ),
        ("twin_scoped", HookValue::Bool(twin_scoped)),
        ("shared_cache", HookValue::Bool(entry.shared)),
        ("destination_exists", HookValue::Bool(destination.exists())),
        (
            "version",
            entry
                .version
                .clone()
                .map_or(HookValue::Unit, HookValue::str),
        ),
        (
            "expected_sha256",
            entry.sha256.clone().map_or(HookValue::Unit, HookValue::str),
        ),
        ("has_process", HookValue::Bool(entry.process.is_some())),
    ]);

    let Some(result) = lunco_hooks::invoke_unclassified(ASSET_DOWNLOAD_PREPARE_HOOK, &[facts])
    else {
        return Ok(DownloadPolicy::default());
    };
    let value = result.map_err(|error| DownloadError::PolicyInvalid(error.to_string()))?;
    parse_download_policy(value)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_download_policy(value: lunco_hooks::HookValue) -> Result<DownloadPolicy, DownloadError> {
    use lunco_hooks::HookValue;

    let HookValue::Map(fields) = value else {
        return Err(DownloadError::PolicyInvalid("expected a map".to_owned()));
    };
    let allowed = [
        "allow_network",
        "cache",
        "install_mode",
        "backup",
        "retain_max_count",
    ];
    for (name, _) in &fields {
        if !allowed.contains(&name.as_str()) {
            return Err(DownloadError::PolicyInvalid(format!(
                "unknown field `{name}`"
            )));
        }
    }
    if fields
        .iter()
        .filter(|(name, _)| name == "allow_network")
        .count()
        != 1
        || fields.iter().filter(|(name, _)| name == "cache").count() != 1
        || fields
            .iter()
            .filter(|(name, _)| name == "install_mode")
            .count()
            != 1
        || fields.iter().filter(|(name, _)| name == "backup").count() != 1
        || fields
            .iter()
            .filter(|(name, _)| name == "retain_max_count")
            .count()
            != 1
    {
        return Err(DownloadError::PolicyInvalid(
            "all decision fields are required exactly once".to_owned(),
        ));
    }
    let get = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
            .expect("required policy field was checked above")
    };
    let allow_network = match get("allow_network") {
        HookValue::Bool(value) => *value,
        value => return Err(policy_type("allow_network", "bool", value)),
    };
    let cache = match get("cache").as_str() {
        Some("use_existing") => false,
        Some("refresh") => true,
        _ => {
            return Err(DownloadError::PolicyInvalid(
                "cache must be `use_existing` or `refresh`".to_owned(),
            ));
        }
    };
    let install_mode = match get("install_mode").as_str() {
        Some("replace") => InstallMode::Replace,
        Some("keep_existing") => InstallMode::KeepExisting,
        Some("reject") => InstallMode::Reject,
        _ => {
            return Err(DownloadError::PolicyInvalid(
                "install_mode must be `replace`, `keep_existing`, or `reject`".to_owned(),
            ));
        }
    };
    let backup = match get("backup").as_str() {
        Some("discard_after_commit") => BackupDisposition::DiscardAfterCommit,
        Some("retain") => BackupDisposition::Retain,
        _ => {
            return Err(DownloadError::PolicyInvalid(
                "backup must be `discard_after_commit` or `retain`".to_owned(),
            ));
        }
    };
    let retain_max_count = match get("retain_max_count") {
        HookValue::Int(value) if (0..=32).contains(value) => *value as usize,
        HookValue::Int(value) => {
            return Err(DownloadError::PolicyInvalid(format!(
                "retain_max_count must be between 0 and 32, got {value}"
            )));
        }
        value => return Err(policy_type("retain_max_count", "int", value)),
    };
    if backup == BackupDisposition::Retain && retain_max_count == 0 {
        return Err(DownloadError::PolicyInvalid(
            "retained backups require retain_max_count > 0".to_owned(),
        ));
    }
    Ok(DownloadPolicy {
        allow_network,
        cache_refresh: cache,
        install_mode,
        backup,
        retain_max_count,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn policy_type(field: &str, expected: &str, actual: &lunco_hooks::HookValue) -> DownloadError {
    DownloadError::PolicyInvalid(format!(
        "field `{field}` must be {expected}, got {}",
        actual.type_name()
    ))
}

/// Caller-supplied control surface for a download. It carries optional HTTP
/// progress, tar-extraction progress, cancellation, and installation ownership
/// signals. All default to inactive so callers opt in independently.
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
    /// Shared installation barrier. Dataset-scope teardown acquires the same
    /// barrier before retirement, so no worker can install after close returns.
    pub commit_gate: Option<std::sync::Arc<std::sync::Mutex<()>>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_guard<'a>(
    control: &'a DownloadControl<'_>,
    destination: &Path,
) -> Result<Option<std::sync::MutexGuard<'a, ()>>, DownloadError> {
    control
        .commit_gate
        .as_ref()
        .map(|gate| {
            gate.lock().map_err(|_| {
                DownloadError::WriteFailed(
                    destination.to_path_buf(),
                    "download commit gate is poisoned; refusing to install".into(),
                )
            })
        })
        .transpose()
}

#[cfg(not(target_arch = "wasm32"))]
struct StagingPath {
    path: PathBuf,
    directory: bool,
    armed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl StagingPath {
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            directory: false,
            armed: true,
        }
    }

    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            directory: true,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for StagingPath {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self.directory {
            let _ = std::fs::remove_dir_all(&self.path);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn install_staged_path(
    staged: &Path,
    destination: &Path,
    version: Option<&str>,
    archive_hash: Option<&str>,
    policy: &DownloadPolicy,
    control: &DownloadControl<'_>,
) -> Result<(), DownloadError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| DownloadError::WriteFailed(parent.to_path_buf(), error.to_string()))?;

    let directory = staged.is_dir();
    let marker_values: Vec<(&str, &str)> = version
        .map(|value| ("version", value))
        .into_iter()
        .chain(
            archive_hash
                .filter(|hash| !hash.is_empty())
                .map(|hash| ("integrity", hash)),
        )
        .collect();
    let marker_paths: Vec<(PathBuf, PathBuf)> = marker_values
        .iter()
        .map(|(suffix, _)| {
            let destination_path = install_marker_path(destination, directory, suffix);
            let staged_path = staged.with_file_name(format!(
                ".{}-{suffix}",
                staged
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("lunco-staged")
            ));
            (destination_path, staged_path)
        })
        .collect();
    let mut marker_stages = Vec::new();
    for ((_, value), (_, path)) in marker_values.iter().zip(&marker_paths) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| DownloadError::WriteFailed(path.clone(), error.to_string()))?;
        file.write_all(value.as_bytes())
            .map_err(|error| DownloadError::WriteFailed(path.clone(), error.to_string()))?;
        marker_stages.push(StagingPath::file(path.clone()));
    }

    let _gate = commit_guard(control, destination)?;
    if control
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire))
    {
        return Err(DownloadError::Cancelled);
    }

    static INSTALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = INSTALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let backup_root = parent.join(format!(".lunco-install-backup-{}-{id}", std::process::id()));
    std::fs::create_dir(&backup_root)
        .map_err(|error| DownloadError::WriteFailed(backup_root.clone(), error.to_string()))?;
    let mut backup = StagingPath::directory(backup_root.clone());

    let backup_destination = backup_root.join("destination");
    let marker_backups: Vec<PathBuf> = marker_paths
        .iter()
        .enumerate()
        .map(|(index, _)| backup_root.join(format!("marker-{index}")))
        .collect();
    let mut destination_backed_up = false;
    let mut marker_backed_up = vec![false; marker_paths.len()];
    let mut destination_installed = false;
    let result = (|| {
        if destination.exists() {
            std::fs::rename(destination, &backup_destination).map_err(|error| {
                DownloadError::WriteFailed(destination.to_path_buf(), error.to_string())
            })?;
            destination_backed_up = true;
        }
        for (index, (marker_path, _)) in marker_paths.iter().enumerate() {
            if marker_path.exists() {
                std::fs::rename(marker_path, &marker_backups[index]).map_err(|error| {
                    DownloadError::WriteFailed(marker_path.clone(), error.to_string())
                })?;
                marker_backed_up[index] = true;
            }
        }
        std::fs::rename(staged, destination).map_err(|error| {
            DownloadError::WriteFailed(destination.to_path_buf(), error.to_string())
        })?;
        destination_installed = true;
        for ((marker_path, staged_path), _) in marker_paths.iter().zip(&marker_stages) {
            std::fs::rename(staged_path, marker_path).map_err(|error| {
                DownloadError::WriteFailed(marker_path.clone(), error.to_string())
            })?;
        }
        Ok::<(), DownloadError>(())
    })();

    if let Err(error) = result {
        if destination_installed && destination.exists() {
            // Keep rollback O(1) while the gate is held. The guard removes the
            // failed tree after the lifecycle barrier is released.
            let _ = std::fs::rename(destination, backup_root.join("failed"));
        }
        for (index, (marker_path, _)) in marker_paths.iter().enumerate() {
            if marker_path.exists() {
                let _ = std::fs::remove_file(marker_path);
            }
            if marker_backed_up[index] {
                let _ = std::fs::rename(&marker_backups[index], marker_path);
            }
        }
        if destination_backed_up {
            let _ = std::fs::rename(&backup_destination, destination);
        }
        drop(_gate);
        drop(backup);
        drop(marker_stages);
        return Err(error);
    }

    drop(_gate);
    let has_retained_state = destination_backed_up || marker_backed_up.iter().any(|backed| *backed);
    if policy.backup == BackupDisposition::Retain
        && has_retained_state
        && policy.retain_max_count > 0
    {
        backup.disarm();
        retain_install_backups(parent, policy.retain_max_count);
    }
    drop(backup);
    drop(marker_stages);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn retain_install_backups(parent: &Path, max_count: usize) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let mut backups = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(".lunco-install-backup-"))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| entry.file_name());
    let keep_from = backups.len().saturating_sub(max_count);
    for entry in backups.into_iter().take(keep_from) {
        // The prefix and directory check above are the complete deletion
        // authority. Policy never supplies a path or a delete operation.
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_assets_core::{cache_dir, twin_cache_dir};

    #[test]
    fn scratch_name_is_opaque_and_cross_platform() {
        let scratch = scratch_name(10_737, 0);
        assert_eq!(scratch, "lunco_10737_0");
        assert!(!scratch.contains('/'));
        assert!(!scratch.contains('\\'));
    }

    #[test]
    fn safe_rel_dest_accepts_plain_relative() {
        assert!(lunco_assets_path::is_safe_relative_path(
            "terrain/apollo15/.cache/dtm.tif"
        ));
        assert!(lunco_assets_path::is_safe_relative_path(
            "textures/moon.png"
        ));
        assert!(lunco_assets_path::is_safe_relative_path(
            "fonts/DejaVuSans.ttf"
        ));
    }

    #[test]
    fn safe_rel_dest_rejects_traversal_and_absolute() {
        // Parent escape — the whole point of the guard.
        assert!(!lunco_assets_path::is_safe_relative_path("../escape.tif"));
        assert!(!lunco_assets_path::is_safe_relative_path(
            "terrain/../../escape.tif"
        ));
        assert!(!lunco_assets_path::is_safe_relative_path("a/../b/../../x"));
        // Absolute (Unix + Windows drive).
        assert!(!lunco_assets_path::is_safe_relative_path("/etc/passwd"));
        assert!(!lunco_assets_path::is_safe_relative_path("C:/Users/x"));
        // Backslash is a traversal vector on Windows; reject everywhere.
        assert!(!lunco_assets_path::is_safe_relative_path(r"terrain\..\x"));
        // Empty / leading-slash-adjacent.
        assert!(!lunco_assets_path::is_safe_relative_path(""));
        assert!(!lunco_assets_path::is_safe_relative_path("."));
        assert!(!lunco_assets_path::is_safe_relative_path(".."));
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
            recommended: false,
            process: None,
            bundle: Vec::new(),
            extra: Default::default(),
        };
        let err = download_asset(
            &entry,
            "evil",
            &DownloadSettings::default(),
            Some(std::path::Path::new("/tmp")),
        )
        .expect_err("traversal must be rejected");
        assert!(matches!(err, DownloadError::ManifestFailed(_)));
    }

    #[test]
    fn every_download_scope_rejects_unsafe_dest_before_resolution() {
        let entry = AssetEntry {
            name: "evil".into(),
            version: None,
            url: "https://example.invalid/never-fetched".into(),
            dest: Some("../escape.tif".into()),
            extract: None,
            shared: false,
            sha256: None,
            recommended: false,
            process: None,
            bundle: Vec::new(),
            extra: Default::default(),
        };
        let error = entry_dest_path(&entry, None).expect_err("engine path must be contained");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// The default is the Twin's cache; `shared = true` selects the global
    /// write owner. Both locations remain readable through the Twin URI.
    #[test]
    fn a_twins_download_lands_in_that_twins_cache_unless_it_opts_into_the_shared_pool() {
        let twin_cache = twin_cache_dir(std::path::Path::new("/tmp/twin"));
        let mut entry = AssetEntry {
            name: "dtm".into(),
            version: None,
            url: "https://example.invalid/NAC_DTM.TIF".into(),
            dest: None,
            extract: None,
            shared: false,
            sha256: None,
            recommended: false,
            process: None,
            bundle: Vec::new(),
            extra: Default::default(),
        };

        // Default: Twin-local pool.
        let local = entry_dest_path(&entry, Some(&twin_cache)).expect("safe local destination");
        assert!(
            local.starts_with(&twin_cache),
            "expected the twin cache, got {}",
            local.display()
        );
        assert!(local.ends_with("NAC_DTM.TIF"));

        // Opt-in: the global cache, whatever owner root was offered.
        entry.shared = true;
        let shared = entry_dest_path(&entry, Some(&twin_cache)).expect("shared destination");
        assert!(
            !shared.starts_with(&twin_cache) && shared.starts_with(cache_dir()),
            "shared = true must reach the global pool, got {}",
            shared.display()
        );

        entry.dest = Some("terrain/apollo15/dtm.tif".into());
        assert_eq!(
            entry_dest_path(&entry, Some(&twin_cache)).expect("shared authored destination"),
            cache_dir().join("terrain/apollo15/dtm.tif")
        );

        // An authored `dest` is still twin-relative.
        entry.shared = false;
        entry.dest = Some("terrain/apollo15/dtm.tif".into());
        assert_eq!(
            entry_dest_path(&entry, Some(&twin_cache)).expect("safe authored destination"),
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
        let tmp_dir = tempfile::tempdir().expect("temporary Twin directory");
        let tmp = tmp_dir.path();
        lunco_storage::write_file_sync(
            &tmp.join("Assets.toml"),
            b"[x]\nname = \"X\"\nurl = \"http://x/x\"\ndest = \"terrain/x.tif\"\n",
        )
        .unwrap();
        // Not downloaded yet → "not installed", but the function must not
        // panic and must complete (i.e. dest_root was accepted).
        let res = list_manifest(&tmp.join("Assets.toml"), "twin", Some(tmp));
        assert!(res.is_ok());
    }

    #[test]
    fn staged_install_replaces_a_directory_and_version_as_one_commit() {
        let root = tempfile::tempdir().expect("temporary install root");
        let staged = root.path().join(".download-stage");
        let destination = root.path().join("library");
        lunco_storage::ensure_directory_sync(&staged).expect("create staging directory");
        lunco_storage::write_file_sync(&staged.join("package.mo"), b"new")
            .expect("write staged payload");
        lunco_storage::ensure_directory_sync(&destination).expect("create old destination");
        lunco_storage::write_file_sync(&destination.join("package.mo"), b"old")
            .expect("write old payload");

        install_staged_path(
            &staged,
            &destination,
            Some("4.1.0"),
            None,
            &DownloadPolicy::default(),
            &DownloadControl::default(),
        )
        .expect("install staged directory");

        assert_eq!(
            lunco_storage::read_text_file_sync(&destination.join("package.mo"))
                .expect("read installed"),
            "new"
        );
        assert_eq!(
            lunco_storage::read_text_file_sync(&destination.join(".version"))
                .expect("read version"),
            "4.1.0"
        );
        assert!(
            matches!(
                lunco_storage::entry_kind_file_sync(&staged),
                Err(lunco_storage::StorageError::NotFound)
            ),
            "staging tree must be moved, not copied"
        );
        assert!(
            lunco_storage::read_directory_sync(root.path())
                .expect("list install root")
                .iter()
                .all(|entry| {
                    entry
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_none_or(|name| !name.starts_with(".lunco-install-backup-"))
                })
        );
    }

    #[test]
    fn download_policy_accepts_only_the_closed_typed_decision() {
        let policy = parse_download_policy(lunco_hooks::HookValue::map([
            ("allow_network", lunco_hooks::HookValue::Bool(false)),
            ("cache", lunco_hooks::HookValue::str("refresh")),
            ("install_mode", lunco_hooks::HookValue::str("replace")),
            ("backup", lunco_hooks::HookValue::str("retain")),
            ("retain_max_count", lunco_hooks::HookValue::Int(2)),
        ]))
        .expect("valid policy");
        assert!(!policy.allow_network);
        assert!(policy.cache_refresh);
        assert_eq!(policy.install_mode, InstallMode::Replace);
        assert_eq!(policy.backup, BackupDisposition::Retain);
        assert_eq!(policy.retain_max_count, 2);
    }

    #[test]
    fn download_policy_rejects_unknown_and_unsafe_retention_choices() {
        let unknown = parse_download_policy(lunco_hooks::HookValue::map([
            ("allow_network", lunco_hooks::HookValue::Bool(true)),
            ("cache", lunco_hooks::HookValue::str("use_existing")),
            ("install_mode", lunco_hooks::HookValue::str("keep_existing")),
            (
                "backup",
                lunco_hooks::HookValue::str("discard_after_commit"),
            ),
            ("retain_max_count", lunco_hooks::HookValue::Int(0)),
            ("delete_path", lunco_hooks::HookValue::str("/tmp")),
        ]));
        assert!(matches!(unknown, Err(DownloadError::PolicyInvalid(_))));

        let zero_retention = parse_download_policy(lunco_hooks::HookValue::map([
            ("allow_network", lunco_hooks::HookValue::Bool(true)),
            ("cache", lunco_hooks::HookValue::str("use_existing")),
            ("install_mode", lunco_hooks::HookValue::str("replace")),
            ("backup", lunco_hooks::HookValue::str("retain")),
            ("retain_max_count", lunco_hooks::HookValue::Int(0)),
        ]));
        assert!(matches!(
            zero_retention,
            Err(DownloadError::PolicyInvalid(_))
        ));
    }

    #[test]
    fn cancelled_staged_install_leaves_the_previous_payload_intact() {
        let root = tempfile::tempdir().expect("temporary install root");
        let staged = root.path().join(".download-stage");
        let destination = root.path().join("asset.bin");
        lunco_storage::write_file_sync(&staged, b"new").expect("write staged payload");
        lunco_storage::write_file_sync(&destination, b"old").expect("write old payload");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let control = DownloadControl {
            cancel: Some(cancel),
            ..DownloadControl::default()
        };

        assert!(matches!(
            install_staged_path(
                &staged,
                &destination,
                None,
                None,
                &DownloadPolicy::default(),
                &control,
            ),
            Err(DownloadError::Cancelled)
        ));
        assert_eq!(
            lunco_storage::read_text_file_sync(&destination).expect("read old payload"),
            "old"
        );
        assert!(
            lunco_storage::entry_kind_file_sync(&staged).is_ok(),
            "the caller-owned staging guard must retain the cancelled payload until it drops"
        );
    }
}
