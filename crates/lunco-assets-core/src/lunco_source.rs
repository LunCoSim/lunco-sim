//! The `lunco://` asset source — the engine asset **library**.
//!
//! `lunco://<rel>` is a *logical* identity: "this asset belongs to the LunCo
//! library". Where the bytes actually sit is a resolution detail, deliberately
//! not part of the address:
//!
//! 1. `assets/<rel>` — git-tracked, authored content
//! 2. `assets/.cache/<rel>` — the PACKED cache: binaries shipped inside the
//!    distribution, so a packaged build carries its own payload
//! 3. `<cache>/<rel>` — the shared machine-wide pool, filled by the downloader
//!
//! Large binaries stay out of git; they are resolved into the library rather
//! than addressed by a machine-local cache path, so authored USD remains
//! portable and third-party USD tools see the same logical identity.
//!
//! See `docs/architecture/56-asset-resolution-and-cache.md`.
//!
//! **One root plan, every platform.** Native sources use the shared entry
//! selector and storage opener; wasm uses Bevy's maintained HTTP reader. HTTP requests encode literal filename components through
//! its request mapper. The browser resolves the same chain over HTTP as
//! native resolves over directories — the fallback is not a native-only
//! convenience that silently disappears on web.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use bevy::asset::io::PathStream;
#[cfg(any(test, target_arch = "wasm32"))]
use bevy::asset::io::{AssetReader, AssetReaderError, Reader};
use bevy::asset::io::{AssetSource, AssetSourceBuilder, AssetWatcher, ErasedAssetReader};

/// The asset-source scheme for the engine asset library — the name it is
/// registered under, both as a Bevy `AssetSource` and in the
/// [`SchemeRegistry`](crate::scheme_registry::SchemeRegistry).
pub use lunco_assets_path::LUNCO_SCHEME;

/// The library-relative path of a `lunco://<rel>` reference, or `None` for a bare
/// or differently-schemed one. Unlike [`crate::engine_asset_rel`] (which treats a
/// bare path as already-relative), this distinguishes "explicitly addressed to
/// the engine library" — what a caller re-rooting an id back onto disk needs.
pub fn parse_lunco_uri(uri: &str) -> Option<&str> {
    let (scheme, rel) = crate::asset_path::split_scheme(uri)?;
    (scheme.eq_ignore_ascii_case(LUNCO_SCHEME)).then_some(rel)
}

/// The directory name the shipped asset library lives under (`assets`). The
/// `lunco://` source is anchored on it, so code walking a path's ancestors to
/// find that root must ask here rather than spell the literal again.
pub const ASSETS_DIR_NAME: &str = "assets";

/// The shipped-asset root (`…/assets`) an on-disk file lives under, if any —
/// the directory `lunco://` is anchored at *for that file*.
///
/// Distinct from [`crate::assets_dir_abs`], which selects the runtime library
/// root from executable and current-directory ancestry: this answers the
/// question for a file that may live outside the running project (a tool
/// composing a `.usda` by absolute path), so it walks that file's ancestors.
pub fn shipped_asset_root(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|a| a.file_name() == Some(std::ffi::OsStr::new(ASSETS_DIR_NAME)))
}

/// Read bytes for a canonical asset identity through the native asset-location
/// policy. USD composition deliberately calls this instead of touching the
/// filesystem: asset-root selection and diagnostic paths stay owned here.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes(id: &str, assets_root: Option<&Path>) -> std::io::Result<Vec<u8>> {
    read_asset_bytes_inner(id, assets_root, None, None)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_native_asset(path: PathBuf, max_bytes: Option<usize>) -> std::io::Result<Vec<u8>> {
    use lunco_storage::Storage;
    let storage = lunco_storage::FileStorage::new();
    let handle = lunco_storage::StorageHandle::File(path);
    let result = bevy::tasks::futures_lite::future::block_on(async {
        match max_bytes {
            Some(limit) => storage.read_bounded(&handle, limit).await,
            None => storage.read(&handle).await,
        }
    });
    result.map_err(|error| match error {
        lunco_storage::StorageError::NotFound => {
            std::io::Error::new(std::io::ErrorKind::NotFound, "asset file was not found")
        }
        lunco_storage::StorageError::Io(error) => error,
        error => std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_asset_bytes_inner(
    id: &str,
    assets_root: Option<&Path>,
    max_bytes: Option<usize>,
    twin: Option<&crate::TwinReadScope>,
) -> std::io::Result<Vec<u8>> {
    let Some(rel) = parse_lunco_uri(id) else {
        let path = lunco_storage::file_uri_to_path(id)
            .map_err(|error| match error {
                lunco_storage::StorageError::Io(error) => error,
                error => std::io::Error::other(error.to_string()),
            })?
            .or_else(|| {
                let path = PathBuf::from(id);
                path.is_absolute().then_some(path)
            })
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("asset `{id}` has no resolvable native root"),
                )
            })?;
        let path = if let Some(scope) = twin {
            let path =
                lunco_storage::canonicalize_file_path(&path).map_err(|error| match error {
                    lunco_storage::StorageError::Io(error) => error,
                    error => std::io::Error::other(error.to_string()),
                })?;
            if !path.starts_with(scope.root()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("native asset `{id}` is outside the captured Twin root"),
                ));
            }
            path
        } else {
            path
        };
        return read_native_asset(path, max_bytes);
    };
    let default_root;
    let root = match assets_root {
        Some(root) => root,
        None => {
            default_root = crate::assets_dir_abs()?;
            &default_root
        }
    };
    let relative = crate::asset_path::relative_path(rel).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe library asset path `{rel}`"),
        )
    })?;
    if let Some((_, path)) = resolve_native_asset(
        &crate::asset_read_roots(root),
        &relative,
        Some(lunco_storage::StorageEntryKind::File),
    )? {
        return read_native_asset(path, max_bytes);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("asset `{id}` was not found in the library or cache"),
    ))
}

/// Resolve an existing file-system entry under `root` without following a
/// symlink outside that root.
///
/// Lexical traversal checks are necessary for URI paths, but they do not stop
/// an authored or downloaded Twin from placing a symlink inside the root. A
/// canonicalized result is returned so the subsequent read does not repeat the
/// symlink lookup. Missing entries return `Ok(None)`; filesystem and containment
/// failures return an error so callers cannot silently fall through to another
/// root.
#[cfg(not(target_arch = "wasm32"))]
pub fn existing_path_within_root(root: &Path, relative: &Path) -> std::io::Result<Option<PathBuf>> {
    if !crate::asset_path::is_safe_relative_components(relative) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe relative path `{}`", relative.display()),
        ));
    }
    let candidate = root.join(relative);
    let mut probe = candidate.as_path();
    loop {
        match std::fs::symlink_metadata(probe) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if probe == root {
                    return Ok(None);
                }
                probe = probe.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_candidate = std::fs::canonicalize(probe)?;
    if !canonical_candidate.starts_with(canonical_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "asset path resolves outside its owning root",
        ));
    }
    if probe != candidate {
        // Windows can report a missing descendant when an existing parent is
        // a file. That source error must not expose a cached replacement.
        if !std::fs::metadata(canonical_candidate)?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "asset ancestor is not a directory",
            ));
        }
        return Ok(None);
    }
    Ok(Some(canonical_candidate))
}

/// Select the first existing entry, never a later cache copy of another kind.
/// Missing entries alone advance the search; containment, metadata and kind
/// errors are terminal. The root index also anchors native directory readers.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_native_asset(
    roots: &[PathBuf],
    relative: &Path,
    expected: Option<lunco_storage::StorageEntryKind>,
) -> std::io::Result<Option<(usize, PathBuf)>> {
    if roots.is_empty() || roots.iter().any(|root| !root.is_absolute()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "native asset resolution requires absolute source roots",
        ));
    }
    for (index, root) in roots.iter().enumerate() {
        let Some(path) = existing_path_within_root(root, relative)? else {
            continue;
        };
        if let Some(expected) = expected {
            let actual =
                lunco_storage::entry_kind_file_sync(&path).map_err(|error| match error {
                    lunco_storage::StorageError::Io(error) => error,
                    lunco_storage::StorageError::NotFound => {
                        std::io::Error::from(std::io::ErrorKind::NotFound)
                    }
                    error => std::io::Error::other(error.to_string()),
                })?;
            if actual != expected {
                let kind = if expected == lunco_storage::StorageEntryKind::Directory {
                    std::io::ErrorKind::NotADirectory
                } else {
                    std::io::ErrorKind::InvalidInput
                };
                return Err(std::io::Error::new(
                    kind,
                    format!(
                        "asset {} is {actual:?}, expected {expected:?}",
                        path.display()
                    ),
                ));
            }
        }
        return Ok(Some((index, path)));
    }
    Ok(None)
}

/// Bounded native bytes resolved within the exact captured source scope.
/// Namespace identity is checked before any Twin filesystem lookup.
/// Native file references in that scope must canonically belong to its root.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes_in_scope(
    id: &str,
    assets_root: Option<&Path>,
    twin: Option<&crate::TwinReadScope>,
    max_bytes: usize,
) -> std::io::Result<Vec<u8>> {
    if let Some((name, rel)) = crate::parse_twin_uri(id) {
        let scope = twin.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Twin asset `{id}` has no composing Twin root"),
            )
        })?;
        if !scope.accepts(name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Twin asset `{id}` is outside the captured source scope"),
            ));
        }
        let root = scope.root();
        let relative = crate::asset_path::relative_path(rel).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Twin asset `{id}` escapes its root"),
            )
        })?;
        let path = crate::twin_source::resolve_twin_relative_file(root, &relative)
            .map_err(|error| {
                let kind = match &error {
                    crate::twin_source::TwinRootsError::AssetResolution(kind, _) => *kind,
                    _ => std::io::ErrorKind::Other,
                };
                std::io::Error::new(
                    kind,
                    format!("could not resolve Twin asset `{id}`: {error}"),
                )
            })?
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Twin asset `{id}` was not found in its authored tree or cache"),
                )
            })?;
        return read_native_asset(path, Some(max_bytes));
    }
    read_asset_bytes_inner(id, assets_root, Some(max_bytes), twin)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn synchronous_twin_reads_share_the_canonical_traversal_guard() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        lunco_storage::write_file_sync(&root.path().join("scenario.rhai"), b"40 + 2")
            .expect("scenario");
        let cached = crate::twin_cache_dir(root.path());
        lunco_storage::ensure_directory_sync(&cached).expect("cache root");
        lunco_storage::write_file_sync(&cached.join("downloaded.rhai"), b"20 + 22")
            .expect("cached scenario");

        let id = "twin://example/scenario.rhai";
        assert_eq!(
            read_asset_bytes_in_scope(
                id,
                None,
                Some(&crate::TwinReadScope::declared("example", root.path()).unwrap()),
                1024
            )
            .expect("authored file"),
            b"40 + 2"
        );
        assert_eq!(
            read_asset_bytes_in_scope(
                "twin://example/downloaded.rhai",
                None,
                Some(&crate::TwinReadScope::declared("example", root.path()).unwrap()),
                1024
            )
            .expect("cached file"),
            b"20 + 22"
        );
        for id in [
            "twin://example/../outside.rhai",
            "twin://example/a/../../outside.rhai",
            r"twin://example/..\outside.rhai",
        ] {
            let error = read_asset_bytes_in_scope(
                id,
                None,
                Some(&crate::TwinReadScope::declared("example", root.path()).unwrap()),
                1024,
            )
            .expect_err("unsafe Twin path must be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn library_ids_cannot_escape_the_asset_root() {
        let root = tempfile::tempdir().expect("temporary asset root");
        for id in [
            "lunco://../outside.usda",
            "lunco://terrain/../../outside.usda",
            r"lunco://terrain\..\outside.usda",
        ] {
            assert!(
                read_asset_bytes(id, Some(root.path())).is_err(),
                "unsafe library id must be rejected: {id}"
            );
        }
    }

    #[test]
    fn unanchored_relative_ids_are_not_assigned_a_host_root() {
        assert_eq!(
            read_asset_bytes("scenes/scene.usda", None)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn file_uri_reader_decodes_native_paths_and_rejects_invalid_input() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("провајдер with spaces.usda");
        lunco_storage::write_file_sync(&source, b"#usda 1.0\n").unwrap();
        let id = lunco_storage::file_path_to_uri(&source).unwrap();
        assert_eq!(read_asset_bytes(&id, None).unwrap(), b"#usda 1.0\n");
        assert_eq!(
            read_asset_bytes("file:///invalid%00.usda", None)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn library_reads_default_to_the_runtime_asset_root() {
        let bytes = read_asset_bytes("lunco://scenes/base/lunar_surface.usda", None)
            .expect("authored engine asset is available without a caller root");
        assert!(bytes.starts_with(b"#usda 1.0"));
    }

    #[test]
    fn library_reads_an_explicit_package_cache_before_the_shared_cache() {
        let package_assets = tempfile::tempdir().expect("temporary package assets root");
        let packed = package_assets.path().join(".cache/scenes/base");
        lunco_storage::ensure_directory_sync(&packed).expect("packed cache directory");
        lunco_storage::write_file_sync(&packed.join("lunar_surface.usda"), b"packed package asset")
            .expect("packed asset");

        assert_eq!(
            read_asset_bytes(
                "lunco://scenes/base/lunar_surface.usda",
                Some(package_assets.path())
            )
            .expect("explicit package cache asset"),
            b"packed package asset"
        );
    }

    #[test]
    fn library_reads_normalize_windows_separators_before_lookup() {
        let root = tempfile::tempdir().expect("temporary asset root");
        let scene = root.path().join("scenes/base/lunar_surface.usda");
        lunco_storage::ensure_directory_sync(scene.parent().expect("scene parent"))
            .expect("scene directory");
        lunco_storage::write_file_sync(&scene, b"windows-authored path").expect("scene");

        assert_eq!(
            read_asset_bytes(r"lunco://scenes\base\lunar_surface.usda", Some(root.path()))
                .expect("Windows-authored URI"),
            b"windows-authored path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn twin_reads_reject_symlinks_that_leave_the_root() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let outside = tempfile::tempdir().expect("temporary outside root");
        let secret = outside.path().join("secret.txt");
        lunco_storage::write_file_sync(&secret, b"must not be read").expect("secret");
        lunco_storage::create_file_symlink_sync(&secret, &root.path().join("linked.txt"))
            .expect("symlink");

        assert!(
            existing_path_within_root(root.path(), Path::new("linked.txt"))
                .expect_err("escaping symlink must be rejected")
                .kind()
                == std::io::ErrorKind::PermissionDenied
        );
        let error = read_asset_bytes_in_scope(
            "twin://example/linked.txt",
            None,
            Some(&crate::TwinReadScope::declared("example", root.path()).unwrap()),
            1024,
        )
        .expect_err("Twin symlink must not escape its root");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn an_unsafe_authored_twin_entry_cannot_fall_through_to_cache() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let outside = tempfile::tempdir().expect("temporary outside root");
        let secret = outside.path().join("secret.txt");
        lunco_storage::write_file_sync(&secret, b"must not be read").expect("secret");
        lunco_storage::create_file_symlink_sync(&secret, &root.path().join("linked.txt"))
            .expect("symlink");

        let cache = crate::twin_cache_dir(root.path());
        lunco_storage::ensure_directory_sync(&cache).expect("cache root");
        lunco_storage::write_file_sync(&cache.join("linked.txt"), b"cache shadow")
            .expect("cache file");

        let error = read_asset_bytes_in_scope(
            "twin://example/linked.txt",
            None,
            Some(&crate::TwinReadScope::declared("example", root.path()).unwrap()),
            1024,
        )
        .expect_err("an unsafe authored path must block cache fallback");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}

/// Native read for a caller-selected root document. Kept in `lunco-assets-core` so
/// USD consumers never perform their own filesystem access.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    read_native_asset(path.to_path_buf(), None)
}

/// Read authored UTF-8 text through the asset boundary.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read_asset_file_bytes(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Build the `lunco://` [`AssetSourceBuilder`] from the authored library root,
/// its packed cache, and the shared cache in [`crate::asset_read_roots`] order.
///
/// Only the authored `assets/` tree is watched. Cache roots are read-only
/// materialized artifacts and can contain tens of thousands of directories;
/// recursively registering an OS watch for them is both unnecessary for live
/// authoring and can exhaust the process' watch quota before the app starts.
pub fn lunco_asset_source(assets_dir: &Path) -> AssetSourceBuilder {
    let watch_root = assets_dir.to_string_lossy().into_owned();
    let reader_roots = crate::asset_read_roots(assets_dir);
    AssetSourceBuilder::new(move || {
        #[cfg(not(target_arch = "wasm32"))]
        return Box::new(crate::NativeAssetReader::new(reader_roots.clone()))
            as Box<dyn ErasedAssetReader>;
        #[cfg(target_arch = "wasm32")]
        {
            let roots = reader_roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            Box::new(FallbackReader {
                readers: roots
                    .iter()
                    .map(|root| library_asset_reader(root))
                    .collect(),
                roots,
            }) as Box<dyn ErasedAssetReader>
        }
    })
    .with_watcher(move |sender| {
        if !Path::new(&watch_root).exists() {
            return None;
        }
        let mut build =
            AssetSource::get_default_watcher(watch_root.clone(), Duration::from_millis(300));
        build(sender)
            .map(|watcher| Box::new(FallbackWatcher { _watcher: watcher }) as Box<dyn AssetWatcher>)
    })
}

fn library_asset_reader(root: &str) -> Box<dyn ErasedAssetReader> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Box::new(crate::NativeAssetReader::new(vec![PathBuf::from(root)]))
    }
    #[cfg(target_arch = "wasm32")]
    {
        // TODO: stream browser HTTP bodies through the shared bounded reader.
        // HttpWasmAssetReader materializes the response before returning its
        // Reader, so consumer limits do not bound this backend's allocation.
        Box::new(
            bevy::asset::io::wasm::HttpWasmAssetReader::new(root).with_request_mapper(|path| {
                std::borrow::Cow::Owned(crate::asset_path::http_asset_path(path))
            }),
        )
    }
}

/// One directory source with the shared native containment or browser filename
/// transport and Bevy's platform watcher. This source has no cache search tier.
pub fn directory_asset_source(root: &Path) -> AssetSourceBuilder {
    let root = root.to_string_lossy().into_owned();
    AssetSourceBuilder::platform_default(&root, None)
        .with_reader(move || library_asset_reader(&root))
}

/// Keeps the authored-tree watcher backing the fallback reader alive.
///
/// The reader's existing priority still decides which bytes win, so a cache
/// artifact can never override an authored asset. Cache population is handled
/// by the asset/download boundary; it is not an authoring edit stream.
struct FallbackWatcher {
    _watcher: Box<dyn AssetWatcher>,
}

impl AssetWatcher for FallbackWatcher {}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_transport_tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    // The existing runner serves these generic plaintext temporary fixtures
    // from its owned working directory; setup is documented in asset I/O.
    #[wasm_bindgen_test(async)]
    async fn browser_readers_preserve_literal_filename_payloads_and_missing_errors() {
        let mut default_source =
            directory_asset_source(Path::new("assets/http-path-fixture/default"));
        let mut library_source = lunco_asset_source(Path::new("assets/http-path-fixture/library"));
        let readers = [(default_source.reader)(), (library_source.reader)()];
        let fixtures = [
            ("payload # % 月.txt", b"literal-delimiters".as_slice()),
            ("payload%20one.txt", b"literal-percent20".as_slice()),
            ("payload one.txt", b"literal-space".as_slice()),
        ];
        for reader in &readers {
            for (filename, expected) in fixtures {
                let mut stream = ErasedAssetReader::read(reader.as_ref(), Path::new(filename))
                    .await
                    .expect("read exact literal filename over HTTP");
                let mut bytes = Vec::new();
                Reader::read_to_end(stream.as_mut(), &mut bytes)
                    .await
                    .expect("read HTTP payload");
                assert_eq!(bytes, expected, "{filename}");
            }
            let missing =
                ErasedAssetReader::read(reader.as_ref(), Path::new("missing # % 月.txt")).await;
            assert!(matches!(missing, Err(AssetReaderError::NotFound(_))));
        }

        let mut stream =
            ErasedAssetReader::read(readers[1].as_ref(), Path::new("cache # % 月.txt"))
                .await
                .expect("read packed-cache fallback through the same mapper");
        let mut bytes = Vec::new();
        Reader::read_to_end(stream.as_mut(), &mut bytes)
            .await
            .expect("read packed-cache payload");
        assert_eq!(bytes, b"packed-cache");

        let settings = lunco_settings::DownloadSettings::default();
        for (filename, expected) in fixtures {
            let url = crate::asset_path::web_url(&format!("http-path-fixture/default/{filename}"));
            let bytes = crate::web_fetch::network_fetch_uncached(&url, &settings)
                .await
                .expect("worker fetch uses the same literal filename transport");
            assert_eq!(bytes, expected, "{filename}");
        }
    }
}

/// Reads each root in turn, moving on only when the asset is absent there.
///
/// Order is priority: authored content wins over the packed cache, which wins
/// over the shared pool. So a file committed under `assets/` is never silently
/// replaced by whatever a download left behind, and a distribution's own
/// payload is never shadowed by a stale copy in the machine-wide cache.
///
/// Only [`AssetReaderError::NotFound`] falls through. A genuine I/O failure —
/// permissions, a truncated HTTP response — propagates immediately, because
/// retrying it against the next root would convert a real error into a
/// confusing "not found" and hide the actual cause. A complete miss returns
/// the reader-facing logical path, never the native path of the last root.
///
/// [`read`] also names every root in its warning. A miss on
/// `lunco://components/cameras/lunar_surface_camera.usda` therefore remains
/// actionable without making a machine-local cache path part of the asset
/// identity or user-facing error.
///
/// [`read`]: AssetReader::read
#[cfg(target_arch = "wasm32")]
struct FallbackReader {
    readers: Vec<Box<dyn ErasedAssetReader>>,
    /// Parallel to `readers`, kept solely so a miss can say where it looked.
    roots: Vec<String>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_resolution_contract_tests {
    use super::*;

    #[test]
    fn native_resolution_edges_cli_ancestor_is_terminal() {
        const CHILD: &str = "LUNCO_RESOLUTION_ANCESTOR_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let error =
                crate::resolve_asset_input("blocked/entry", lunco_storage::StorageEntryKind::File)
                    .expect_err("an invalid native ancestor must not select library bytes");
            assert_eq!(error.kind(), std::io::ErrorKind::NotADirectory);
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("cwd");
        let library = root.path().join("library");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::create_dir_all(library.join("blocked")).unwrap();
        std::fs::write(cwd.join("blocked"), b"file").unwrap();
        std::fs::write(library.join("blocked/entry"), b"wrong source").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lunco_source::native_resolution_contract_tests::native_resolution_edges_cli_ancestor_is_terminal"])
            .env(CHILD, "1").env(crate::ASSET_ROOT_ENV, &library).current_dir(cwd).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[cfg(unix)]
    fn native_resolution_edges_posix_double_root_is_native_input() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("entry");
        std::fs::write(&file, b"native").unwrap();
        let double_root = format!("/{}", file.display());
        assert_eq!(
            crate::resolve_asset_input(&double_root, lunco_storage::StorageEntryKind::File)
                .unwrap(),
            std::fs::canonicalize(file).unwrap()
        );
    }

    #[test]
    fn native_resolution_edges_scoped_file_uris_reject_foreign_files() {
        let root = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("entry"), b"owned").unwrap();
        std::fs::write(foreign.path().join("entry"), b"foreign").unwrap();
        let scope = crate::TwinReadScope::declared("owner", root.path()).unwrap();
        let owned_uri = lunco_storage::file_path_to_uri(&root.path().join("entry")).unwrap();
        assert_eq!(
            read_asset_bytes_in_scope(&owned_uri, None, Some(&scope), 1024).unwrap(),
            b"owned"
        );
        let foreign_uri = lunco_storage::file_path_to_uri(&foreign.path().join("entry")).unwrap();
        assert_eq!(
            read_asset_bytes_in_scope(&foreign_uri, None, Some(&scope), 1024)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn native_resolution_edges_reader_uses_logical_relative_grammar() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("folder")).unwrap();
        std::fs::write(root.path().join("folder/entry"), b"payload").unwrap();
        std::fs::create_dir(root.path().join("folder/directory")).unwrap();
        std::fs::write(root.path().join("folder/directory/child"), b"child").unwrap();
        let reader = crate::NativeAssetReader::new(vec![root.path().to_path_buf()]);
        bevy::tasks::futures_lite::future::block_on(async {
            for invalid in ["entry:stream", "entry\0suffix"] {
                assert!(
                    matches!(AssetReader::read(&reader, Path::new(invalid)).await,
                    Err(AssetReaderError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidInput)
                );
            }
            let mut source = AssetReader::read(&reader, Path::new(r"folder\entry"))
                .await
                .unwrap();
            let mut bytes = Vec::new();
            Reader::read_to_end(&mut source, &mut bytes).await.unwrap();
            assert_eq!(bytes, b"payload");
            assert!(
                AssetReader::is_directory(&reader, Path::new(""))
                    .await
                    .unwrap()
            );
            let mut entries = AssetReader::read_directory(&reader, Path::new(r"folder\directory"))
                .await
                .unwrap();
            use bevy::tasks::futures_lite::StreamExt;
            assert_eq!(
                entries.next().await,
                Some(PathBuf::from("folder/directory/child"))
            );
        });
    }

    #[test]
    #[cfg(windows)]
    fn native_resolution_edges_keep_verbatim_filename_semantics() {
        let folder = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(folder.path()).unwrap();
        let verbatim = root.join("native.");
        let ordinary = root.join("native");
        std::fs::create_dir(&verbatim).unwrap();
        std::fs::create_dir(&ordinary).unwrap();
        let file = verbatim.join("entry");
        std::fs::write(&file, b"verbatim").unwrap();
        std::fs::write(ordinary.join("entry"), b"ordinary").unwrap();
        assert_eq!(
            crate::resolve_asset_input(
                file.to_str().unwrap(),
                lunco_storage::StorageEntryKind::File
            )
            .unwrap(),
            file
        );
        let uri = lunco_storage::file_path_to_uri(&file).unwrap();
        assert_eq!(read_asset_bytes(&uri, None).unwrap(), b"verbatim");
        let ordinary_spelling = folder.path().join("native./entry");
        assert_eq!(std::fs::read(&ordinary_spelling).unwrap(), b"ordinary");
        let ordinary_uri = lunco_storage::file_path_to_uri(&ordinary_spelling).unwrap();
        assert_eq!(read_asset_bytes(&ordinary_uri, None).unwrap(), b"ordinary");
    }

    #[test]
    fn native_resolution_contract_preserves_first_entry_kind() {
        let folder = tempfile::tempdir().unwrap();
        let authored = folder.path().join("authored # % 月");
        let cached = folder.path().join("cached");
        std::fs::create_dir_all(&authored).unwrap();
        std::fs::create_dir_all(cached.join("entry")).unwrap();
        std::fs::write(authored.join("entry"), b"authored file").unwrap();
        std::fs::write(cached.join("entry/child"), b"cached directory").unwrap();
        let roots = vec![authored, cached];
        let reader = crate::NativeAssetReader::new(roots.clone());
        let mounts = crate::TwinRoots::default();
        let authority = mounts.register_authority("owner", &roots[0]).unwrap();
        let scope = mounts.read_scope(&authority).unwrap();
        for name in [&authority, "owner"] {
            assert_eq!(
                read_asset_bytes_in_scope(
                    &crate::twin_uri(name, "entry"),
                    None,
                    Some(&scope),
                    1024
                )
                .unwrap(),
                b"authored file"
            );
        }
        assert_eq!(
            read_asset_bytes_in_scope("twin://foreign/entry", None, Some(&scope), 1024)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        mounts.unregister_authority(&authority).unwrap();
        assert!(mounts.read_scope(&authority).is_err());

        let selected = resolve_native_asset(
            &roots,
            Path::new("entry"),
            Some(lunco_storage::StorageEntryKind::File),
        )
        .unwrap()
        .unwrap()
        .1;
        assert_eq!(
            selected,
            std::fs::canonicalize(roots[0].join("entry")).unwrap()
        );
        assert_eq!(
            resolve_native_asset(
                &roots,
                Path::new("entry"),
                Some(lunco_storage::StorageEntryKind::Directory)
            )
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::NotADirectory
        );
        assert!(
            resolve_native_asset(&roots, Path::new("missing"), None)
                .unwrap()
                .is_none()
        );
        assert!(resolve_native_asset(&roots, Path::new("entry/child"), None).is_err());
        assert!(resolve_native_asset(&roots, Path::new("../escape"), None).is_err());
        assert!(
            crate::resolve_asset_input(
                roots[0].to_str().unwrap(),
                lunco_storage::StorageEntryKind::File
            )
            .is_err()
        );
        assert!(
            crate::resolve_asset_input(
                roots[0].join("missing").to_str().unwrap(),
                lunco_storage::StorageEntryKind::File
            )
            .is_err()
        );
        for input in [
            selected.to_str().unwrap().to_owned(),
            lunco_storage::file_path_to_uri(&selected).unwrap(),
        ] {
            assert_eq!(
                crate::resolve_asset_input(&input, lunco_storage::StorageEntryKind::File).unwrap(),
                selected
            );
        }
        for input in [
            r"C:\native\source.bin",
            r"\\?\C:\native\source.bin",
            r"\\server\share\source.bin",
        ] {
            assert!(crate::asset_path::load_asset_path(input, None, None, None).is_err());
        }
        let origin = bevy::asset::AssetPath::parse("twin://owner/scenes/main.usda");
        assert!(
            crate::asset_path::load_asset_path("../../other/secret.bin", Some(&origin), None, None)
                .is_err()
        );
        assert!(
            crate::asset_path::load_asset_path("twin://other/../secret.bin", None, None, None)
                .is_err()
        );
        bevy::tasks::futures_lite::future::block_on(async {
            assert!(
                !AssetReader::is_directory(&reader, Path::new("entry"))
                    .await
                    .unwrap(),
                "a cached directory must not replace an authored file"
            );
            assert!(
                AssetReader::read_directory(&reader, Path::new("entry"))
                    .await
                    .is_err()
            );
            let mut source = AssetReader::read(&reader, Path::new("entry"))
                .await
                .unwrap();
            let mut bytes = Vec::new();
            Reader::read_to_end(&mut source, &mut bytes).await.unwrap();
            assert_eq!(bytes, b"authored file");
            assert!(
                !AssetReader::is_directory(&reader, Path::new("missing"))
                    .await
                    .unwrap()
            );
            assert!(matches!(
                AssetReader::read(&reader, Path::new("missing")).await,
                Err(AssetReaderError::NotFound(_))
            ));
        });
    }
}

#[cfg(all(test, unix))]
mod native_reader_contract_tests {
    use super::*;

    #[test]
    fn asset_resolution_contract() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("root");
        let later = folder.path().join("later");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&later).unwrap();
        let outside = folder.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("payload"), b"outside").unwrap();
        std::fs::write(root.join("literal # %.bin"), b"inside").unwrap();
        std::fs::write(later.join("fallback"), b"later").unwrap();
        std::fs::create_dir(later.join("escape")).unwrap();
        std::fs::write(later.join("escape/payload"), b"wrong owner").unwrap();
        std::fs::write(later.join("literal # %.bin.meta"), b"wrong owner").unwrap();
        std::os::unix::fs::symlink(outside.join("payload"), root.join("literal # %.bin.meta"))
            .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let root_for_default = root.clone();
        let reader = crate::NativeAssetReader::new(vec![root, later]);
        bevy::tasks::futures_lite::future::block_on(async {
            let mut source = directory_asset_source(&root_for_default);
            let default = (source.reader)();
            assert!(
                ErasedAssetReader::is_directory(default.as_ref(), Path::new(""))
                    .await
                    .unwrap()
            );
            assert!(
                AssetReader::read_directory(&reader, Path::new(""))
                    .await
                    .is_ok()
            );
            assert!(
                matches!(ErasedAssetReader::read_meta(default.as_ref(), Path::new("literal # %.bin")).await,
                Err(AssetReaderError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
            );
            let relative_root = crate::NativeAssetReader::new(vec![PathBuf::from("relative")]);
            assert!(
                AssetReader::read(&relative_root, Path::new("payload"))
                    .await
                    .is_err()
            );
            let mut stream = AssetReader::read(&reader, Path::new("literal # %.bin"))
                .await
                .unwrap();
            let mut bytes = Vec::new();
            Reader::read_to_end(&mut stream, &mut bytes).await.unwrap();
            assert_eq!(bytes, b"inside");
            assert!(
                AssetReader::read(&reader, Path::new("fallback"))
                    .await
                    .is_ok()
            );
            assert!(
                matches!(AssetReader::read(&reader, Path::new("escape/payload")).await,
                Err(AssetReaderError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
            );
            assert!(
                matches!(AssetReader::read_meta(&reader, Path::new("literal # %.bin")).await,
                Err(AssetReaderError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
            );
            assert!(
                AssetReader::is_directory(&reader, Path::new("escape"))
                    .await
                    .is_err()
            );
            assert!(
                AssetReader::read_directory(&reader, Path::new("escape"))
                    .await
                    .is_err()
            );
        });
    }
}

/// Try each root in order; the first non-`NotFound` answer wins.
#[cfg(target_arch = "wasm32")]
macro_rules! try_both {
    ($self:ident, $method:ident, $path:expr_2021) => {{
        // `readers` is non-empty by construction (`assets/` is always first),
        // so the loop always assigns before the unwrap.
        let mut last = None;
        for reader in &$self.readers {
            match reader.$method($path).await {
                Err(AssetReaderError::NotFound(_)) => {
                    last = Some(Err(AssetReaderError::NotFound($path.to_path_buf())))
                }
                other => return other,
            }
        }
        last.unwrap()
    }};
}

#[cfg(target_arch = "wasm32")]
impl AssetReader for FallbackReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        let result = try_both!(self, read, path);
        if matches!(result, Err(AssetReaderError::NotFound(_))) {
            // Only `read`. Bevy probes for a sibling `.meta` on EVERY asset and
            // almost never finds one, so warning from `read_meta` would bury
            // this line in noise from the ordinary case.
            bevy::log::warn!(
                "[lunco://] `{}` not found in any library root. Looked in order:\n{}",
                path.display(),
                self.roots
                    .iter()
                    .enumerate()
                    .map(|(i, root)| format!("  {}. {root}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        result
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        try_both!(self, read_meta, path)
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Err(AssetReaderError::NotFound(path.to_path_buf()));
        }
        try_both!(self, read_directory, path)
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        if !crate::asset_path::is_safe_relative_components(path) {
            return Ok(false);
        }
        // `is_directory` answers false rather than erroring for a missing path,
        // so `NotFound` is not the signal here — a plain `false` is.
        let mut last = Ok(false);
        for reader in &self.readers {
            match reader.is_directory(path).await {
                Ok(false) => last = Ok(false),
                Err(AssetReaderError::NotFound(p)) => last = Err(AssetReaderError::NotFound(p)),
                other => return other,
            }
        }
        last
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod windows_uri_tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn reads_a_windows_authored_twin_uri_below_its_registered_root() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        let scene = root.path().join("sim/scenes/traverse.usda");
        lunco_storage::ensure_directory_sync(scene.parent().expect("scene parent"))
            .expect("create scene parent");
        lunco_storage::write_file_sync(&scene, b"#usda 1.0\n").expect("write scene");

        assert_eq!(
            read_asset_bytes_in_scope(
                r"twin://fixture\sim\scenes\traverse.usda",
                None,
                Some(&crate::TwinReadScope::declared("fixture", root.path()).unwrap()),
                1024
            )
            .expect("Windows-authored Twin URI resolves through the Twin root"),
            b"#usda 1.0\n"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn missing_library_asset_reports_the_logical_path() {
        let root = tempfile::tempdir().expect("temporary asset root");
        let roots = [root.path().join("authored"), root.path().join("cache")];
        let reader = crate::NativeAssetReader::new(roots.to_vec());

        let error = match futures_lite::future::block_on(AssetReader::read(
            &reader,
            Path::new("vessels/markers/waypoint.usda"),
        )) {
            Ok(_) => panic!("missing asset unexpectedly loaded"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            AssetReaderError::NotFound(PathBuf::from("vessels/markers/waypoint.usda"))
        );
    }
}
