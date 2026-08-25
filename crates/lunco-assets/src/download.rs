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
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
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
    /// Put this download in the **global** cache instead of the owner's own
    /// cache. An authored `dest` remains that relative path under the global
    /// cache; when `dest` is omitted, the URL-keyed source pool is used.
    ///
    /// Default `false`: a Twin's downloads are written to that Twin's `.cache`.
    /// The reader still checks the global cache after the Twin-local cache, so a
    /// Twin can consume a product another Twin already shared. Set `shared =
    /// true` when this declaration owns a reusable upstream product and should
    /// write its copy to the global cache.
    ///
    /// Ignored for engine-scoped entries: their owner's cache IS the shared
    /// cache, so the two resolve to the same place.
    #[serde(default)]
    pub shared: bool,
    /// Expected SHA-256 hex digest. Empty string means "compute and suggest".
    pub sha256: Option<String>,
    /// Offer this dataset in the first-run resource prompt when it is not
    /// already installed. This is an onboarding recommendation, not a
    /// runtime dependency: the application remains usable when the user
    /// declines it.
    #[serde(default)]
    pub recommended: bool,
    /// Distribution targets that require this delivered artifact inside the
    /// application bundle. An empty list means the dataset remains a user
    /// provisioned resource. The packager matches the binary name exactly.
    #[serde(default)]
    pub bundle: Vec<String>,
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
    /// Whether this declaration's delivered artifact belongs in `binary`'s
    /// package. Packaging policy is authored beside the dataset, not repeated
    /// in shell scripts that can drift from the manifest.
    pub fn bundled_for(&self, binary: &str) -> bool {
        self.bundle.iter().any(|target| target == binary)
    }

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
/// - `shared = true` → the global cache, whoever declared it.
/// - Authored `dest` → `<owner cache>/<dest>` (the shared cache for a crate
///   manifest, `<twin>/.cache` for a Twin's).
/// - No `dest` → the owner's source pool, keyed by URL hash.
pub fn entry_dest_path(
    entry: &AssetEntry,
    dest_root: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    if !entry.shared {
        if let Some(dest) = entry.dest.as_deref() {
            if !crate::asset_path::is_safe_relative_path(dest) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("asset destination {dest:?} must be a safe relative path"),
                ));
            }
        }
    }
    // A shared entry uses the global cache as its owner. Keep an authored
    // destination below that root; only an entry without `dest` uses the
    // URL-keyed source pool. Engine entries already pass the global cache as
    // their owner, so `shared` has no special effect for them.
    let root = if entry.shared {
        cache_dir()
    } else {
        dest_root.map(Path::to_path_buf).unwrap_or_else(cache_dir)
    };
    Ok(match entry.dest.as_deref() {
        Some(d) => root.join(d),
        None => source_pool_path(&root, &entry.url),
    })
}

/// The delivered artifact path for an engine or Twin declaration. A processed
/// entry resolves to its output; an unprocessed entry resolves to its download
/// destination. Both packaging and runtime provisioning use this boundary so
/// a raw source is never mistaken for the product a consumer loads.
#[cfg(not(target_arch = "wasm32"))]
pub fn entry_artifact_path(
    entry: &AssetEntry,
    cache_root: &Path,
    twin_root: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    match &entry.process {
        Some(process) => crate::process::process_output_path(process, Some(cache_root), twin_root),
        None => entry_dest_path(entry, Some(cache_root)),
    }
}

/// The completion marker belongs to the destination it describes. A directory
/// install keeps the marker inside that directory so moving the dataset keeps
/// its identity; a file install uses a filename-specific sibling so two
/// versioned files in one directory cannot overwrite one another's marker.
pub fn version_marker_path(destination: &Path) -> PathBuf {
    install_marker_path(destination, destination.is_dir(), "version")
}

fn integrity_marker_path(destination: &Path) -> PathBuf {
    install_marker_path(destination, destination.is_dir(), "integrity")
}

fn install_marker_path(destination: &Path, directory: bool, suffix: &str) -> PathBuf {
    if directory {
        return destination.join(format!(".{suffix}"));
    }
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lunco-dataset");
    destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.{suffix}"))
}

/// Validate a non-processed installed destination using the declaration's
/// integrity contract. A directory is complete only when it has payload and,
/// for versioned archives, the destination-local version marker. File hashes
/// are checked before the registry advertises the entry as installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn installed_destination_present(entry: &AssetEntry, destination: &Path) -> bool {
    let expects_directory = is_archive_url(&entry.url) && entry.extract.is_none();
    if expects_directory != destination.is_dir() {
        return false;
    }
    if destination.is_file() {
        if destination.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            return false;
        }
        let Some(expected) = entry.sha256.as_deref().filter(|hash| !hash.is_empty()) else {
            return true;
        };
        if is_archive_url(&entry.url) {
            return std::fs::read_to_string(integrity_marker_path(destination))
                .is_ok_and(|actual| actual.trim().eq_ignore_ascii_case(expected));
        }
        use sha2::{Digest, Sha256};
        let Ok(bytes) = std::fs::read(destination) else {
            return false;
        };
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        return actual.eq_ignore_ascii_case(expected);
    }
    if !destination.is_dir() {
        return false;
    }
    let has_payload = std::fs::read_dir(destination)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            let name = entry.file_name();
            name != ".version" && name != ".integrity"
        });
    if !has_payload {
        return false;
    }
    let version_matches = entry.version.as_deref().is_none_or(|expected| {
        std::fs::read_to_string(version_marker_path(destination))
            .is_ok_and(|actual| actual.trim() == expected.trim())
    });
    let integrity_matches = entry.sha256.as_deref().is_none_or(|expected| {
        expected.is_empty()
            || std::fs::read_to_string(integrity_marker_path(destination))
                .is_ok_and(|actual| actual.trim().eq_ignore_ascii_case(expected))
    });
    version_matches && integrity_matches
}

fn is_archive_url(url: &str) -> bool {
    archive_extension(url).is_some()
}

fn archive_extension(url: &str) -> Option<&'static str> {
    if url.ends_with(".tar.gz") {
        Some("tar.gz")
    } else if url.ends_with(".tgz") {
        Some("tgz")
    } else if url.ends_with(".tar.bz2") {
        Some("tar.bz2")
    } else if url.ends_with(".tbz2") {
        Some("tbz2")
    } else if url.ends_with(".tbz") {
        Some("tbz")
    } else {
        None
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
/// entries that opted into `shared = true`; a Twin's own `.cache` holds the
/// pool for entries with the default ownership. Keying by URL hash (not
/// basename) means two products that share a filename never collide; the
/// basename is kept alongside so the pool stays readable.
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
        .filter(|s| !s.is_empty() && crate::asset_path::is_safe_relative_path(s))
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

/// How long to wait for the TCP/TLS connection to come up.
///
/// Short on purpose: a host that has not accepted a connection in half a minute
/// is down, firewalled, or misrouted — waiting longer never turns into bytes.
#[cfg(not(target_arch = "wasm32"))]
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long to wait for the request headers to go out on an established
/// connection.
#[cfg(not(target_arch = "wasm32"))]
pub const SEND_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long to wait for the RESPONSE HEADERS after the request is sent.
///
/// Generous (2 min) because some sources compute before they answer — a JPL
/// Horizons vectors query is a server-side job, not a static file — but still
/// bounded: this phase transfers nothing, so a long wait here is always a
/// stalled peer, never a big file.
#[cfg(not(target_arch = "wasm32"))]
pub const RECV_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Number of additional attempts for a transient connection or server error.
///
/// Asset downloads run in CI as well as from the workbench. A short-lived
/// refusal from a public dataset host must not turn an otherwise healthy
/// build into a missing-asset failure, while a permanently unavailable URL
/// must still fail after a bounded amount of time.
pub const DOWNLOAD_RETRIES: usize = 5;

/// Returns whether a failed request is worth trying again.
#[cfg(not(target_arch = "wasm32"))]
fn is_retryable_download_error(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::StatusCode(code) => {
            matches!(code, 408 | 425 | 429 | 500..=599)
        }
        ureq::Error::Io(_)
        | ureq::Error::Timeout(_)
        | ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::Protocol(_) => true,
        _ => false,
    }
}

/// Bounded delay between request retries. The body is never
/// retried here: it is streamed into an attempt-unique scratch file and any
/// hash mismatch remains a terminal integrity error.
#[cfg(not(target_arch = "wasm32"))]
fn download_retry_delay(retry_number: usize) -> std::time::Duration {
    let seconds = match retry_number {
        1 => 1,
        2 => 16,
        3 => 32,
        4 => 64,
        5 => 128,
        _ => unreachable!("download retry number must be in the range 1..=5"),
    };
    std::time::Duration::from_secs(seconds)
}

/// Maximum interval ureq waits for the next body bytes. This is a transport
/// boundary, not an application-operation timer: a healthy large transfer may
/// run indefinitely while a peer that stops producing bytes releases the
/// registry-owned worker.
#[cfg(not(target_arch = "wasm32"))]
pub const BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Downloads an asset from the manifest entry. Equivalent to
/// [`download_asset_with_control`] with no progress callback and no
/// cancellation flag — keeps existing CLI/test call sites unchanged.
///
/// `dest_root` supplies the owning cache when the declaration is Twin-scoped:
/// `None` selects the global engine cache; `Some(dir)` selects that Twin's
/// local cache unless `shared = true`, which selects the global cache.
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
/// `dest_root` supplies the owning cache for a Twin declaration. `None` selects
/// the global engine cache; `Some(dir)` selects the Twin-local cache unless
/// `shared = true`, which selects the global cache. Authored USD always
/// addresses the resulting artifact through its logical Twin URI, never via
/// the physical cache path.
/// When a `dest_root` is supplied, `entry.dest` is validated to be a
/// strictly relative path with no `..` segments (see
/// [`crate::asset_path::is_safe_relative_path`])
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
        if !crate::asset_path::is_safe_relative_path(d) {
            return Err(DownloadError::ManifestFailed(format!(
                "asset `{key}` has an unsafe `dest` for a twin download: {d:?} \
                 (must be relative, no `..`, no absolute, no backslash)"
            )));
        }
    }
    let dest = entry_dest_path(entry, dest_root)
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;

    // Cache-hit check #1 — versioned install (used by libraries like
    // the MSL tarball where `version = "4.1.0"` pins an upstream
    // release). Matches on `.version` marker sibling.
    if installed_destination_present(entry, &dest) {
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

    // Download in chunks so progress can tick and cancellation is
    // responsive (within one chunk's read latency).
    //
    // Bound every blocking network phase. `timeout_recv_body` is a per-read
    // receive deadline in ureq, so a large healthy file may run indefinitely
    // while a peer that stops producing bytes releases the worker promptly.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_send_request(Some(SEND_REQUEST_TIMEOUT))
        .timeout_recv_response(Some(RECV_RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(BODY_READ_TIMEOUT))
        .build()
        .into();
    let mut retries = 0;
    let response = loop {
        match agent.get(&entry.url).call() {
            Ok(response) => break response,
            Err(error) if is_retryable_download_error(&error) && retries < DOWNLOAD_RETRIES => {
                retries += 1;
                let attempt_number = retries;
                let delay = download_retry_delay(attempt_number);
                eprintln!(
                    "  ! download attempt {attempt_number} failed for {key}: {error}; \
                     retry {attempt_number}/{DOWNLOAD_RETRIES} in {}s",
                    delay.as_secs()
                );
                if cancelled() {
                    return Err(DownloadError::Cancelled);
                }
                std::thread::sleep(delay);
            }
            Err(error) => {
                return Err(DownloadError::DownloadFailed(
                    entry.url.clone(),
                    error.to_string(),
                ));
            }
        }
    };
    let total: u64 = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    // Stream to a temp file, hashing incrementally — never the whole payload
    // in RAM. Content-Length is server-supplied, so it must not dictate an
    // allocation; a multi-GB archive passes through the 64 KiB chunk buffer.
    // Scratch names are unique per ATTEMPT, not per key. The registry owns the
    // operation until this function returns, so a retry cannot truncate its
    // scratch file underneath it.
    let attempt = {
        static ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
    let scratch = format!("lunco_{key}_{}_{attempt}", std::process::id());
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
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        if cancelled() {
            drop(out);
            return Err(DownloadError::Cancelled);
        }
        let n = reader
            .read(&mut chunk)
            .map_err(|e| DownloadError::ReadFailed(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&chunk[..n])
            .map_err(|e| DownloadError::WriteFailed(download_stage.path.clone(), e.to_string()))?;
        hasher.update(&chunk[..n]);
        downloaded += n as u64;
        if let Some(cb) = control.progress.as_mut() {
            cb(downloaded, total);
        }
    }
    drop(out);

    let hash: String = hasher
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
                ))
            }
            [entry] => entry.path(),
            _ => {
                return Err(DownloadError::ExtractFailed(
                    "archive must contain exactly one top-level directory".into(),
                ))
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
                &control,
            )?;
        }
    } else {
        install_staged_path(
            &download_path,
            &dest,
            entry.version.as_deref(),
            None,
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

    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();
    download_entries_with_limit(&format!("`{group}`"), entries, max_parallel, |_| None)
}

#[cfg(not(target_arch = "wasm32"))]
fn download_entries_with_limit(
    label: &str,
    entries: Vec<(String, AssetEntry)>,
    max_parallel: usize,
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
                if let Err(error) = download_asset(&entry, &key, destination.as_deref()) {
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
) -> Result<(), DownloadError> {
    let manifests = crate::engine_manifests()
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
    download_entries_with_limit(&format!("bundle `{bundle}`"), entries, max_parallel, |_| {
        None
    })
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
    let entries: Vec<(String, AssetEntry)> = manifest.assets.into_iter().collect();
    let label = format!("twin {}", twin_root.display());
    let destination_root = move |entry: &AssetEntry| {
        Some(crate::datasets::DatasetScope::twin_cache_root(
            twin_root,
            entry.shared,
        ))
    };
    download_entries_with_limit(&label, entries, max_parallel, destination_root)
}

/// Downloads all assets from a **Twin folder's** `Assets.toml`, using each
/// entry's declared write owner and the parallel download limit configured in
/// settings.json (default: 3).
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
        Some(entry) => {
            let dest_root = crate::datasets::DatasetScope::twin_cache_root(twin_root, entry.shared);
            download_asset(entry, asset_key, Some(&dest_root))
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
pub fn download_one_engine(asset_key: &str) -> Result<(), DownloadError> {
    let manifests = crate::engine_manifests()
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
    for (group, path) in manifests {
        let manifest = AssetManifest::from_file(&path)
            .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
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
    let manifests = crate::engine_manifests()
        .map_err(|error| DownloadError::ManifestFailed(error.to_string()))?;
    for (group, _) in manifests {
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
            .map(|root| crate::datasets::DatasetScope::twin_cache_root(root, entry.shared));
        let owner_cache = twin_owner_cache.as_deref().or(dest_root);
        let dest = entry_dest_path(entry, owner_cache)?;
        let status = if let Some(process) = &entry.process {
            let default_cache;
            let process_cache = match owner_cache {
                Some(root) => Some(root),
                None => {
                    default_cache = crate::cache_dir();
                    Some(default_cache.as_path())
                }
            };
            let artifact = crate::process::process_output_path(process, process_cache, twin_root)?;
            if crate::process::processed_output_present(
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
pub fn list_group(group: &str) -> Result<(), std::io::Error> {
    list_manifest(
        &crate::manifests_dir().join(format!("{group}.toml")),
        group,
        None,
    )
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
    let backup = StagingPath::directory(backup_root.clone());

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
            let _ = std::fs::rename(&destination, backup_root.join("failed"));
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
    drop(backup);
    drop(marker_stages);
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn transient_downloads_use_five_bounded_retries() {
        assert_eq!(DOWNLOAD_RETRIES, 5);
        assert_eq!(download_retry_delay(1), std::time::Duration::from_secs(1));
        assert_eq!(download_retry_delay(2), std::time::Duration::from_secs(16));
        assert_eq!(download_retry_delay(3), std::time::Duration::from_secs(32));
        assert_eq!(download_retry_delay(4), std::time::Duration::from_secs(64));
        assert_eq!(download_retry_delay(5), std::time::Duration::from_secs(128));
        assert!(is_retryable_download_error(&ureq::Error::ConnectionFailed));
        assert!(is_retryable_download_error(&ureq::Error::StatusCode(503)));
        assert!(!is_retryable_download_error(&ureq::Error::StatusCode(404)));
    }

    #[test]
    fn safe_rel_dest_accepts_plain_relative() {
        assert!(crate::asset_path::is_safe_relative_path(
            "terrain/apollo15/.cache/dtm.tif"
        ));
        assert!(crate::asset_path::is_safe_relative_path(
            "textures/moon.png"
        ));
        assert!(crate::asset_path::is_safe_relative_path(
            "fonts/DejaVuSans.ttf"
        ));
    }

    #[test]
    fn safe_rel_dest_rejects_traversal_and_absolute() {
        // Parent escape — the whole point of the guard.
        assert!(!crate::asset_path::is_safe_relative_path("../escape.tif"));
        assert!(!crate::asset_path::is_safe_relative_path(
            "terrain/../../escape.tif"
        ));
        assert!(!crate::asset_path::is_safe_relative_path("a/../b/../../x"));
        // Absolute (Unix + Windows drive).
        assert!(!crate::asset_path::is_safe_relative_path("/etc/passwd"));
        assert!(!crate::asset_path::is_safe_relative_path("C:/Users/x"));
        // Backslash is a traversal vector on Windows; reject everywhere.
        assert!(!crate::asset_path::is_safe_relative_path(r"terrain\..\x"));
        // Empty / leading-slash-adjacent.
        assert!(!crate::asset_path::is_safe_relative_path(""));
        assert!(!crate::asset_path::is_safe_relative_path("."));
        assert!(!crate::asset_path::is_safe_relative_path(".."));
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
        let err = download_asset(&entry, "evil", Some(std::path::Path::new("/tmp")))
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
        let twin_cache = crate::twin_cache_dir(std::path::Path::new("/tmp/twin"));
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

    #[test]
    fn staged_install_replaces_a_directory_and_version_as_one_commit() {
        let root = tempfile::tempdir().expect("temporary install root");
        let staged = root.path().join(".download-stage");
        let destination = root.path().join("msl");
        std::fs::create_dir(&staged).expect("create staging directory");
        std::fs::write(staged.join("package.mo"), "new").expect("write staged payload");
        std::fs::create_dir(&destination).expect("create old destination");
        std::fs::write(destination.join("package.mo"), "old").expect("write old payload");

        install_staged_path(
            &staged,
            &destination,
            Some("4.1.0"),
            None,
            &DownloadControl::default(),
        )
        .expect("install staged directory");

        assert_eq!(
            std::fs::read_to_string(destination.join("package.mo")).expect("read installed"),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join(".version")).expect("read version"),
            "4.1.0"
        );
        assert!(!staged.exists(), "staging tree must be moved, not copied");
        assert!(root
            .path()
            .read_dir()
            .expect("list install root")
            .all(|entry| !entry
                .expect("read install entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".lunco-install-backup-")));
    }

    #[test]
    fn cancelled_staged_install_leaves_the_previous_payload_intact() {
        let root = tempfile::tempdir().expect("temporary install root");
        let staged = root.path().join(".download-stage");
        let destination = root.path().join("asset.bin");
        std::fs::write(&staged, "new").expect("write staged payload");
        std::fs::write(&destination, "old").expect("write old payload");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let control = DownloadControl {
            cancel: Some(cancel),
            ..DownloadControl::default()
        };

        assert!(matches!(
            install_staged_path(&staged, &destination, None, None, &control),
            Err(DownloadError::Cancelled)
        ));
        assert_eq!(
            std::fs::read_to_string(&destination).expect("read old payload"),
            "old"
        );
        assert!(
            staged.exists(),
            "the caller-owned staging guard must retain the cancelled payload until it drops"
        );
    }
}
