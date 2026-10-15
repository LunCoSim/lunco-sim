//! Cross-target source-library storage and bundle contracts.
//!
//! A source library can be materialised on the host filesystem or kept as an
//! in-memory tree on wasm. Consumers use the same read and membership methods
//! for both backends; library-specific loaders own fetching, parsing, and
//! presentation.
//!
//! This module deliberately does not identify a particular Modelica library.
//! It owns only the reusable source-store and bundle-envelope mechanisms. The
//! Modelica integration supplies the library identity and decides when to load
//! it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Where a source library is materialised on the current target.
///
/// Construct one and install it during application setup. Readers use the
/// methods on this type instead of branching on the target or backend.
#[derive(Clone, Debug)]
pub enum LibrarySource {
    /// The library is materialised on the local filesystem at this root.
    Filesystem(PathBuf),
    /// The library has been fetched and decompressed into memory.
    InMemory(Arc<InMemoryLibrary>),
}

/// Decompressed source-library tree held entirely in memory.
#[derive(Debug)]
pub struct InMemoryLibrary {
    /// Files keyed by library-relative path.
    pub files: HashMap<PathBuf, Vec<u8>>,
}

impl InMemoryLibrary {
    /// Number of files in the tree.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Total size of the decompressed files.
    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|v| v.len() as u64).sum()
    }

    /// Materialise UTF-8 files as stable `(uri, source)` pairs.
    ///
    /// URIs are library-relative paths rendered with forward slashes. Invalid
    /// UTF-8 is skipped because Modelica source is UTF-8 text.
    pub fn as_source_pairs(&self) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(self.files.len());
        for (path, bytes) in &self.files {
            let uri = lunco_assets_path::slashed(path);
            if let Ok(source) = std::str::from_utf8(bytes) {
                out.push((uri, source.to_string()));
            }
        }
        out
    }
}

/// Process-wide, ordered source-library roots.
///
/// This is installed once during boot. The source list may contain multiple
/// libraries and may mix filesystem and in-memory backends. The Modelica
/// loader owns the admission policy; this store only provides read-side
/// access to the installed roots.
static GLOBAL_LIBRARY_SOURCES: OnceLock<Vec<LibrarySource>> = OnceLock::new();

/// Install the process-wide, ordered list of source-library roots.
pub fn install_global_library_sources(sources: Vec<LibrarySource>) -> std::io::Result<()> {
    install_sources(&GLOBAL_LIBRARY_SOURCES, sources)
}

fn install_sources(
    slot: &OnceLock<Vec<LibrarySource>>,
    sources: Vec<LibrarySource>,
) -> std::io::Result<()> {
    let sources = admit_sources(sources)?;
    let installed = slot.get_or_init(|| sources.clone());
    let same = installed.len() == sources.len()
        && installed
            .iter()
            .zip(&sources)
            .all(|(left, right)| match (left, right) {
                (LibrarySource::Filesystem(left), LibrarySource::Filesystem(right)) => {
                    left == right
                }
                (LibrarySource::InMemory(left), LibrarySource::InMemory(right)) => {
                    Arc::ptr_eq(left, right)
                }
                _ => false,
            });
    if same {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "installed source-library identities are immutable; restart to select another store",
        ))
    }
}

fn admit_sources(sources: Vec<LibrarySource>) -> std::io::Result<Vec<LibrarySource>> {
    sources
        .into_iter()
        .map(|source| match source {
            #[cfg(not(target_arch = "wasm32"))]
            LibrarySource::Filesystem(root) => {
                lunco_assets_core::canonical_directory_root(&root).map(LibrarySource::Filesystem)
            }
            #[cfg(target_arch = "wasm32")]
            LibrarySource::Filesystem(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "filesystem source libraries require a native host",
            )),
            LibrarySource::InMemory(tree) => {
                for path in tree.files.keys() {
                    if path.is_absolute() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "absolute in-memory library identity",
                        ));
                    }
                    validate_request(path)?;
                }
                Ok(LibrarySource::InMemory(tree))
            }
        })
        .collect()
}

/// Return the process-wide ordered source-library roots.
pub fn global_library_sources() -> &'static [LibrarySource] {
    GLOBAL_LIBRARY_SOURCES
        .get()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Return whether at least one source-library root is installed.
pub fn has_library_source() -> bool {
    !global_library_sources().is_empty()
}

/// Return whether `path` belongs to an installed filesystem-backed library.
#[cfg(not(target_arch = "wasm32"))]
pub fn owns_filesystem_path(path: &std::path::Path) -> bool {
    global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::Filesystem(root) if path.starts_with(root)))
}

/// wasm has no filesystem-backed source roots.
#[cfg(target_arch = "wasm32")]
pub fn owns_filesystem_path(_path: &std::path::Path) -> bool {
    false
}

/// Read from the first owning source. Only absence advances to the next root;
/// invalid paths, containment failures and I/O errors terminate the request.
/// Absolute native paths are accepted only by their owning filesystem source.
pub fn library_read(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    read_sources(global_library_sources(), path)
}

fn read_sources(
    sources: &[LibrarySource],
    path: &std::path::Path,
) -> std::io::Result<Option<Vec<u8>>> {
    validate_request(path)?;
    for source in sources {
        if let Some(bytes) = source.read(path)? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

fn validate_request(path: &std::path::Path) -> std::io::Result<()> {
    if !path.is_absolute() && !lunco_assets_path::is_safe_relative_components(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe library path `{}`", path.display()),
        ));
    }
    Ok(())
}

/// Open an owned file for streaming. Resolution uses the same containment and
/// ordered error contract as byte reads.
#[cfg(not(target_arch = "wasm32"))]
pub fn library_open(path: &std::path::Path) -> std::io::Result<Option<(PathBuf, std::fs::File)>> {
    validate_request(path)?;
    for source in global_library_sources() {
        if let LibrarySource::Filesystem(root) = source {
            if let Some(resolved) = owned_path(root, path)? {
                let file = std::fs::File::open(&resolved)?;
                return Ok(Some((resolved, file)));
            }
        }
    }
    Ok(None)
}

#[cfg(not(target_arch = "wasm32"))]
fn owned_path(root: &std::path::Path, path: &std::path::Path) -> std::io::Result<Option<PathBuf>> {
    let relative = if path.is_absolute() {
        // Other absolute files retain their document-admission owner; the
        // application library must never read or reclassify them.
        let Ok(relative) = path.strip_prefix(root) else {
            return Ok(None);
        };
        relative
    } else {
        path
    };
    lunco_assets_core::existing_path_within_root(root, relative)
}

/// Return whether an installed source is in memory.
pub fn has_in_memory_library() -> bool {
    global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::InMemory(_)))
}

/// Return the first filesystem-backed library root, if any.
pub fn primary_filesystem_library_root() -> Option<&'static std::path::Path> {
    global_library_sources()
        .iter()
        .find_map(|source| match source {
            LibrarySource::Filesystem(path) => Some(path.as_path()),
            LibrarySource::InMemory(_) => None,
        })
}

/// Count the Modelica source files in the installed filesystem roots.
///
/// The resident path set is the shared filesystem traversal used by source
/// membership queries, so callers do not need to maintain a second recursive
/// walker just to populate a load-status count.
#[cfg(not(target_arch = "wasm32"))]
pub fn filesystem_library_file_count() -> usize {
    native_path_set().len()
}

impl LibrarySource {
    /// Read an owned file through the common asset containment boundary.
    pub fn read(&self, path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
        validate_request(path)?;
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(root) => owned_path(root, path)?
                .map(|resolved| lunco_assets_core::read_asset_file_bytes(&resolved))
                .transpose(),
            #[cfg(target_arch = "wasm32")]
            Self::Filesystem(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "filesystem source libraries require a native host",
            )),
            Self::InMemory(inner) => Ok((!path.is_absolute())
                .then(|| inner.files.get(path).cloned())
                .flatten()),
        }
    }

    /// Base path used to construct candidate paths for this backend.
    pub fn base(&self) -> &std::path::Path {
        match self {
            Self::Filesystem(root) => root.as_path(),
            Self::InMemory(_) => std::path::Path::new(""),
        }
    }

    /// Return whether this source contains an already-joined candidate path.
    ///
    /// Filesystem membership uses one resident path set rather than a `stat`
    /// per candidate. In-memory membership is a map lookup.
    pub fn contains(&self, candidate: &std::path::Path) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(root) => {
                candidate.starts_with(root) && native_path_set().contains(candidate)
            }
            #[cfg(target_arch = "wasm32")]
            Self::Filesystem(_) => false,
            Self::InMemory(inner) => inner.files.contains_key(candidate),
        }
    }
}

/// Resident set of every `.mo` file across installed filesystem libraries.
///
/// It is built lazily by one sequential walk. An empty result before a
/// filesystem source is installed is not memoised, allowing a later source
/// installation to retry.
#[cfg(not(target_arch = "wasm32"))]
fn native_path_set() -> &'static std::collections::HashSet<PathBuf> {
    static SET: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();
    static EMPTY: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();

    if let Some(set) = SET.get() {
        return set;
    }
    if !global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::Filesystem(_)))
    {
        return EMPTY.get_or_init(std::collections::HashSet::new);
    }
    SET.get_or_init(build_native_path_set)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_native_path_set() -> std::collections::HashSet<PathBuf> {
    let start = std::time::Instant::now();
    let mut files = std::collections::HashSet::new();
    let mut root_count = 0usize;
    for source in global_library_sources() {
        if let LibrarySource::Filesystem(root) = source {
            root_count += 1;
            collect_modelica_files(root, &mut files);
        }
    }
    info!(
        "[LibraryFs] native path-set: {} .mo files across {} roots in {:?}",
        files.len(),
        root_count,
        start.elapsed()
    );
    files
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_modelica_files(dir: &std::path::Path, files: &mut std::collections::HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if matches!(name, "Resources" | "Images" | "test") {
                    continue;
                }
            }
            collect_modelica_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("mo")
        {
            files.insert(path);
        }
    }
}

/// Load state for a source-library installation.
#[derive(Resource, Debug, Clone, Default)]
pub enum LibraryLoadState {
    /// No load has been requested.
    #[default]
    NotStarted,
    /// The source or compiled artifact is being loaded.
    Loading {
        /// Current loading phase.
        phase: LibraryLoadPhase,
        /// Completed work. The meaning is phase-specific.
        bytes_done: u64,
        /// Total work, or zero when unknown.
        bytes_total: u64,
    },
    /// The source library is resident and usable.
    Ready {
        /// Number of source files made available.
        file_count: usize,
        /// Compressed artifact size, when applicable.
        compressed_bytes: u64,
        /// Decompressed source size, when applicable.
        uncompressed_bytes: u64,
    },
    /// Installation failed and requires visible user action.
    Failed(String),
}

impl LibraryLoadState {
    /// Return whether the library is ready for use.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Return whether loading has not reached a terminal state.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::NotStarted | Self::Loading { .. })
    }
}

/// Phases used by a remote source-library loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryLoadPhase {
    /// Fetching the bundle manifest.
    FetchingManifest,
    /// Fetching the source or parsed bundle.
    FetchingBundle,
    /// Loading a bundle from browser cache.
    LoadingCache,
    /// Decompressing a downloaded bundle.
    Decompressing,
    /// Parsing source or decoding a parsed artifact.
    Parsing,
}

impl LibraryLoadPhase {
    /// Human-readable status label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FetchingManifest => "fetching manifest",
            Self::FetchingBundle => "downloading library",
            Self::LoadingCache => "loading library from cache",
            Self::Decompressing => "decompressing",
            Self::Parsing => "parsing library",
        }
    }
}

/// Artifact codec/version tag shared by the Modelica library packager and
/// runtime. A mismatch is a packaging error, not a request to use an older
/// decoder.
pub const EXPECTED_RUMOCA_ARTIFACT_TAG: &str = "rumoca-0.10.2+bincode2-ast3";

/// Manifest for a source-library bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryManifest {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Compressed source archive metadata.
    pub sources: LibraryBundleEntry,
    /// Optional pre-parsed source artifact metadata.
    pub parsed: LibraryBundleEntry,
    /// Tag identifying the parser/codec that produced `parsed`.
    pub rumoca_artifact_tag: String,
}

/// Metadata for one bundle member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryBundleEntry {
    /// Bundle filename.
    pub filename: String,
    /// SHA-256 digest of the bundle.
    pub sha256: String,
    /// Uncompressed size.
    pub uncompressed_bytes: u64,
    /// Compressed size.
    pub compressed_bytes: u64,
    /// Number of files in the bundle.
    pub file_count: usize,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod resolution_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn asset_resolution_contract() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("library");
        std::fs::create_dir(&root).unwrap();
        let literal = Path::new("literal # %.bin");
        std::fs::write(root.join(literal), b"inside").unwrap();
        let outside = folder.path().join("outside.bin");
        std::fs::write(&outside, b"outside").unwrap();
        let native = admit_sources(vec![LibrarySource::Filesystem(root.clone())])
            .unwrap()
            .remove(0);
        assert!(admit_sources(vec![LibrarySource::Filesystem(PathBuf::from("relative"))]).is_err());
        assert!(admit_sources(vec![LibrarySource::Filesystem(outside.clone())]).is_err());
        let invalid = LibrarySource::InMemory(Arc::new(InMemoryLibrary {
            files: HashMap::from([(PathBuf::from("../escape"), vec![])]),
        }));
        assert!(admit_sources(vec![invalid]).is_err());
        let memory = LibrarySource::InMemory(Arc::new(InMemoryLibrary {
            files: HashMap::from([(literal.to_path_buf(), b"inside".to_vec())]),
        }));
        let slot = OnceLock::new();
        install_sources(&slot, vec![native.clone()]).unwrap();
        install_sources(&slot, vec![native.clone()]).unwrap();
        assert!(install_sources(&slot, vec![memory.clone()]).is_err());
        for source in [&native, &memory] {
            assert_eq!(source.read(literal).unwrap(), Some(b"inside".to_vec()));
            assert!(source.read(Path::new("../outside.bin")).is_err());
            assert!(source.read(&outside).unwrap().is_none());
            assert!(source.read(Path::new("missing.bin")).unwrap().is_none());
        }
        assert_eq!(
            native.read(&root.join(literal)).unwrap(),
            Some(b"inside".to_vec())
        );
        std::fs::create_dir(root.join("directory")).unwrap();
        let later = LibrarySource::InMemory(Arc::new(InMemoryLibrary {
            files: HashMap::from([(PathBuf::from("directory"), b"wrong owner".to_vec())]),
        }));
        assert!(read_sources(&[native.clone(), later], Path::new("directory")).is_err());
        #[cfg(unix)]
        {
            let alias = folder.path().join("alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            let alias = admit_sources(vec![LibrarySource::Filesystem(alias)])
                .unwrap()
                .remove(0);
            assert_eq!(
                alias.read(&root.join(literal)).unwrap(),
                Some(b"inside".to_vec())
            );
            std::os::unix::fs::symlink(&outside, root.join("escape.bin")).unwrap();
            assert_eq!(
                native.read(Path::new("escape.bin")).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
    }
}

/// Asset-source identity for the installed source-library store. Language URIs
/// resolve to this identity; readers never expose the store's cache location.
pub const LIBRARY_SCHEME: &str = "library";

#[derive(Default)]
struct LibraryReader {
    #[cfg(not(target_arch = "wasm32"))]
    native_readers: OnceLock<Vec<Option<lunco_assets_core::NativeAssetReader>>>,
}

impl bevy::asset::io::AssetReader for LibraryReader {
    async fn read<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, bevy::asset::io::AssetReaderError> {
        use bevy::asset::io::{AssetReaderError, Reader, SliceReader};
        let (_index, source, relative) = source_request(path)?;
        match source {
            #[cfg(not(target_arch = "wasm32"))]
            LibrarySource::Filesystem(_) => {
                let readers = self.native_readers.get_or_init(|| {
                    global_library_sources()
                        .iter()
                        .map(|source| match source {
                            LibrarySource::Filesystem(root) => {
                                Some(lunco_assets_core::NativeAssetReader::new(root))
                            }
                            LibrarySource::InMemory(_) => None,
                        })
                        .collect()
                });
                let reader = readers
                    .get(_index)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;
                Ok(Box::new(reader.read(relative).await?) as Box<dyn Reader + 'a>)
            }
            LibrarySource::InMemory(tree) => {
                let bytes = tree
                    .files
                    .get(relative)
                    .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;
                Ok(Box::new(SliceReader::new(bytes)) as Box<dyn Reader + 'a>)
            }
            #[cfg(target_arch = "wasm32")]
            LibrarySource::Filesystem(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "filesystem source libraries require a native host",
            )
            .into()),
        }
    }

    async fn read_meta<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<impl bevy::asset::io::Reader + 'a, bevy::asset::io::AssetReaderError> {
        Err::<bevy::asset::io::VecReader, _>(bevy::asset::io::AssetReaderError::NotFound(
            path.to_path_buf(),
        ))
    }

    async fn read_directory<'a>(
        &'a self,
        _path: &'a std::path::Path,
    ) -> Result<Box<bevy::asset::io::PathStream>, bevy::asset::io::AssetReaderError> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "source-library enumeration uses its admitted catalog",
        )
        .into())
    }

    async fn is_directory<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> Result<bool, bevy::asset::io::AssetReaderError> {
        let (_, source, relative) = source_request(path)?;
        match source {
            #[cfg(not(target_arch = "wasm32"))]
            LibrarySource::Filesystem(root) => {
                if let Some(path) = owned_path(root, &relative)? {
                    return Ok(path.is_dir());
                }
            }
            LibrarySource::InMemory(tree) => {
                return Ok(tree
                    .files
                    .keys()
                    .any(|file| file != &relative && file.starts_with(&relative)));
            }
            #[cfg(target_arch = "wasm32")]
            LibrarySource::Filesystem(_) => {}
        }
        Ok(false)
    }
}

pub(crate) fn library_asset_source() -> bevy::asset::io::AssetSourceBuilder {
    bevy::asset::io::AssetSourceBuilder::new(|| Box::new(LibraryReader::default()))
}

fn source_request(
    path: &std::path::Path,
) -> std::io::Result<(usize, &'static LibrarySource, &std::path::Path)> {
    validate_request(path)?;
    let mut components = path.components();
    let index = components
        .next()
        .and_then(|part| part.as_os_str().to_str())
        .and_then(|index| index.parse::<usize>().ok())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid source-library authority",
            )
        })?;
    let source = global_library_sources().get(index).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unknown source-library authority",
        )
    })?;
    let relative = components.as_path();
    if relative.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "absolute source-library address",
        ));
    }
    validate_request(&relative)?;
    Ok((index, source, relative))
}

/// Address a resource beside an already selected source file. The authority
/// pins the exact installed source, so another library cannot supply its bytes.
pub fn library_resource_path(
    source_file: &std::path::Path,
    relative: &std::path::Path,
) -> std::io::Result<bevy::asset::AssetPath<'static>> {
    validate_request(relative)?;
    if relative.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "absolute library resource",
        ));
    }
    for (index, source) in global_library_sources().iter().enumerate() {
        if !source.contains(source_file) {
            continue;
        }
        let source_relative = match source {
            LibrarySource::Filesystem(root) => source_file
                .strip_prefix(root)
                .map_err(std::io::Error::other)?,
            LibrarySource::InMemory(_) => source_file,
        };
        let parent = source_relative
            .parent()
            .ok_or_else(|| std::io::Error::other("source file has no directory"))?;
        let resource = PathBuf::from(index.to_string()).join(parent).join(relative);
        return Ok(bevy::asset::AssetPath::from_path_buf(resource).with_source(LIBRARY_SCHEME));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "selected source file has no installed library owner",
    ))
}

/// Native read-side mirror of the registered source-library reader.
pub fn library_resource_local_path(path: &std::path::Path) -> std::io::Result<Option<PathBuf>> {
    let (_, source, relative) = source_request(path)?;
    match source {
        #[cfg(not(target_arch = "wasm32"))]
        LibrarySource::Filesystem(root) => owned_path(root, &relative),
        _ => Ok(None),
    }
}
