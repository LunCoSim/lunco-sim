//! `Assets.toml` declarations and artifact path/integrity contracts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lunco_assets_core::cache_dir;
use serde::{Deserialize, Serialize};

/// A single asset entry from `Assets.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntry {
    /// Human-readable name.
    pub name: String,
    /// Semantic version used by archive installs.
    pub version: Option<String>,
    /// URL to download from.
    pub url: String,
    /// Destination relative to the owning cache, or an URL-keyed source pool
    /// when omitted.
    #[serde(default)]
    pub dest: Option<String>,
    /// Optional archive-internal path to extract into `dest`.
    #[serde(default)]
    pub extract: Option<String>,
    /// Put this download in the global cache instead of a Twin-local cache.
    #[serde(default)]
    pub shared: bool,
    /// Expected SHA-256 hex digest. An empty value means no pinned digest.
    pub sha256: Option<String>,
    /// Offer this dataset during onboarding when it is missing.
    #[serde(default)]
    pub recommended: bool,
    /// Binary names for which the delivered artifact belongs in the package.
    #[serde(default)]
    pub bundle: Vec<String>,
    /// Optional native post-processing declaration.
    #[serde(default)]
    pub process: Option<ProcessConfig>,
    /// Domain-owned metadata carried without interpretation by this crate.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl AssetEntry {
    /// Whether this entry is bundled for `binary`.
    pub fn bundled_for(&self, binary: &str) -> bool {
        self.bundle.iter().any(|target| target == binary)
    }

    /// Deserialize one domain-owned metadata table, if present.
    pub fn domain<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Option<Result<T, toml::de::Error>> {
        let raw = self.extra.get(key)?.clone();
        Some(raw.try_into())
    }
}

/// Processing configuration from an `Assets.toml` declaration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProcessConfig {
    /// Pipeline selector. Built-ins include `texture`, `gltf`, `dem`, `map`,
    /// `albedo`, and `normalmap`; native hosts may register additional kinds.
    pub kind: String,
    /// Target dimensions for image-like pipelines.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub target_resolution: Option<[u32; 2]>,
    /// Output path relative to `output_root`.
    pub output: String,
    /// Output owner: `cache`, `assets`, or `twin`.
    #[serde(default = "default_output_root")]
    pub output_root: String,
    /// Center latitude for a DEM crop, in degrees.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub center_lat: Option<f64>,
    /// Center longitude for a DEM crop, in degrees.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub center_lon: Option<f64>,
    /// Side length of a DEM crop in metres.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub window_m: Option<f64>,
    /// Source metres per pixel for a DEM crop.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default = "default_dem_pixel_scale_m")]
    pub pixel_scale_m: f64,
    /// Source-height units converted to metres.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default = "default_dem_height_scale")]
    pub source_height_scale_m_per_unit: f64,
    /// Height offset applied after the source scale.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub source_height_offset_m: f64,
    /// Minimum source latitude for the DEM affine mapping.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub src_min_lat: Option<f64>,
    /// Maximum source latitude for the DEM affine mapping.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub src_max_lat: Option<f64>,
    /// Minimum source longitude for the DEM affine mapping.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub src_min_lon: Option<f64>,
    /// Maximum source longitude for the DEM affine mapping.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub src_max_lon: Option<f64>,
    /// Optional DEM site identity.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub site_id: Option<String>,
    /// Optional source lunar reference frame.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub frame: Option<String>,
    /// Linear material albedo used by the `albedo` pipeline when the source is
    /// a grayscale orthophoto rather than a calibrated reflectance raster.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub albedo_base_linear: Option<f64>,
    /// Maximum relative albedo variation retained from the source's local
    /// detail. The `albedo` pipeline removes the low-frequency illumination
    /// field before applying this contrast.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub albedo_detail_strength: Option<f64>,
    /// Positive radius of the illumination field removed from an orthophoto, in
    /// metres. Calibrated reflectance should use the `texture` pipeline instead.
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(default)]
    pub albedo_illumination_radius_m: Option<f64>,
    /// Processor-specific parameters preserved for registered native
    /// processors. Keeping these values in the manifest makes the dispatch
    /// contract extensible without adding a new field to this shared crate for
    /// every domain-specific baker.
    #[serde(flatten)]
    #[serde(default)]
    pub parameters: BTreeMap<String, toml::Value>,
}

fn default_output_root() -> String {
    "cache".to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
/// Default source pixel scale used when a DEM declaration omits one.
pub fn default_dem_pixel_scale_m() -> f64 {
    2.0
}

#[cfg(not(target_arch = "wasm32"))]
fn default_dem_height_scale() -> f64 {
    1.0
}

/// Parsed `Assets.toml` from a crate, engine manifest, or Twin.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetManifest {
    /// Entries keyed by their manifest table name.
    #[serde(flatten)]
    pub assets: BTreeMap<String, AssetEntry>,
}

impl std::str::FromStr for AssetManifest {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })
    }
}

impl AssetManifest {
    /// Read and parse an `Assets.toml` file.
    pub fn from_file(path: &Path) -> Result<Self, std::io::Error> {
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No manifest at {}", path.display()),
            ));
        }
        let content = std::fs::read_to_string(path)?;
        content.parse()
    }

    /// Read and parse the `Assets.toml` at a Twin root.
    pub fn from_crate_dir(crate_dir: &Path) -> Result<Self, std::io::Error> {
        Self::from_file(&crate_dir.join("Assets.toml"))
    }
}

/// Resolve the download destination under the declaration's owning cache.
pub fn entry_dest_path(
    entry: &AssetEntry,
    dest_root: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    if !entry.shared {
        if let Some(dest) = entry.dest.as_deref() {
            if !lunco_assets_path::is_safe_relative_path(dest) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("asset destination {dest:?} must be a safe relative path"),
                ));
            }
        }
    }
    let root = if entry.shared {
        cache_dir()
    } else {
        dest_root.map(Path::to_path_buf).unwrap_or_else(cache_dir)
    };
    Ok(match entry.dest.as_deref() {
        Some(destination) => root.join(destination),
        None => source_pool_path(&root, &entry.url),
    })
}

/// Resolve the delivered artifact path for a native process declaration.
#[cfg(not(target_arch = "wasm32"))]
pub fn entry_artifact_path(
    entry: &AssetEntry,
    cache_root: &Path,
    twin_root: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    match &entry.process {
        Some(process) => process_output_path(process, Some(cache_root), twin_root),
        None => entry_dest_path(entry, Some(cache_root)),
    }
}

/// Path of a version marker stored beside an installed destination.
pub fn version_marker_path(destination: &Path) -> PathBuf {
    install_marker_path(destination, destination.is_dir(), "version")
}

/// Resolve the marker path associated with an installed destination.
pub fn install_marker_path(destination: &Path, directory: bool, suffix: &str) -> PathBuf {
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

#[cfg(not(target_arch = "wasm32"))]
fn integrity_marker_path(destination: &Path) -> PathBuf {
    install_marker_path(destination, destination.is_dir(), "integrity")
}

/// Recognize archive suffixes supported by the native downloader.
#[cfg(not(target_arch = "wasm32"))]
pub fn archive_extension(url: &str) -> Option<&'static str> {
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

#[cfg(not(target_arch = "wasm32"))]
fn is_archive_url(url: &str) -> bool {
    archive_extension(url).is_some()
}

/// Check a non-processed destination against its manifest integrity contract.
#[cfg(not(target_arch = "wasm32"))]
pub fn installed_destination_present(entry: &AssetEntry, destination: &Path) -> bool {
    let expects_directory = is_archive_url(&entry.url) && entry.extract.is_none();
    if expects_directory != destination.is_dir() {
        return false;
    }
    if destination.is_file() {
        if destination
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(true)
        {
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

/// Resolve a URL-keyed source-pool location under `root`.
pub fn source_pool_path(root: &Path, url: &str) -> PathBuf {
    use sha2::{Digest, Sha256};

    let hash: String = Sha256::digest(url.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let base = url
        .split(['?', '#'])
        .next()
        .and_then(|url| url.rsplit('/').next())
        .filter(|name| !name.is_empty() && lunco_assets_path::is_safe_relative_path(name))
        .unwrap_or("download.bin");
    root.join("sources").join(&hash[..16]).join(base)
}

/// Current processing pipeline identity used by bake completion stamps.
pub const PROCESS_PIPELINE_VERSION: u32 = 7;

/// Resolve the output path of a native process declaration.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_output_path(
    process: &ProcessConfig,
    cache_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    if !lunco_assets_path::is_safe_relative_path(&process.output) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "process output {:?} must be a safe relative path",
                process.output
            ),
        ));
    }
    match process.output_root.as_str() {
        "assets" => Ok(lunco_assets_core::assets_dir_abs().join(&process.output)),
        "twin" => twin_root
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "process output_root=\"twin\" requires an open Twin root",
                )
            })
            .map(|root| root.join(&process.output)),
        "cache" => cache_root
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "process output_root=\"cache\" requires an owning cache root",
                )
            })
            .map(|root| root.join(&process.output)),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unknown process output_root `{other}` (expected \"assets\", \"cache\", or \"twin\")"
            ),
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Path of the completion stamp for a processed artifact.
pub fn bake_stamp_path(output_path: &Path) -> PathBuf {
    if output_path.extension().is_none() {
        return output_path.join(".bakekey");
    }
    let mut name = output_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(".bakekey");
    output_path.with_file_name(name)
}

#[cfg(not(target_arch = "wasm32"))]
/// Content-address the source, process declaration, and pipeline identity.
pub fn bake_key(source: &Path, config: &ProcessConfig) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(source)?;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    // TOML is the manifest's canonical serialization format. It is stable for
    // this field-ordered config and avoids pulling JSON into the lightweight
    // registry crate solely for internal change detection.
    let config_toml = toml::to_string(config)
        .map_err(|error| std::io::Error::other(format!("serializing ProcessConfig: {error}")))?;
    hasher.update(config_toml.as_bytes());
    hasher.update(PROCESS_PIPELINE_VERSION.to_le_bytes());
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(not(target_arch = "wasm32"))]
/// Check that a processed artifact has a valid payload and bake stamp.
pub fn processed_output_present(
    output_path: &Path,
    process: &ProcessConfig,
    source_path: Option<&Path>,
) -> bool {
    let payload_present = match process.kind.as_str() {
        "dem" => {
            output_path.is_dir()
                && output_path
                    .join("materials/textures/heightmap.tif")
                    .is_file()
        }
        "map" | "albedo" | "gltf" | "normalmap" | "texture" => output_path.is_file(),
        // Registered extension processors use the same atomic output
        // contract. A file output must be non-directory; a directory output
        // must contain at least one payload besides its bake stamp. The
        // processor registry owns deeper format validation when it runs.
        _ => {
            output_path.is_file()
                || (output_path.is_dir()
                    && std::fs::read_dir(output_path)
                        .ok()
                        .into_iter()
                        .flatten()
                        .any(|entry| {
                            entry
                                .ok()
                                .is_some_and(|entry| entry.file_name() != ".bakekey")
                        }))
        }
    };
    if !payload_present {
        return false;
    }
    let Ok(stamp) = std::fs::read_to_string(bake_stamp_path(output_path)) else {
        return false;
    };
    let stamp = stamp.trim();
    !stamp.is_empty()
        && source_path.is_none_or(|source| bake_key(source, process).is_ok_and(|key| key == stamp))
}
