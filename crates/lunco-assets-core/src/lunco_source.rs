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
//! **One resolver, every platform.** Every root is read through Bevy's own
//! platform reader, which yields a file reader natively and Bevy's maintained
//! HTTP reader on wasm. HTTP requests encode literal filename components through
//! its request mapper. The browser resolves the same chain over HTTP as
//! native resolves over directories — the fallback is not a native-only
//! convenience that silently disappears on web.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSource, AssetSourceBuilder, AssetWatcher,
    ErasedAssetReader, PathStream, Reader,
};

/// The asset-source scheme for the engine asset library — the name it is
/// registered under, both as a Bevy `AssetSource` and in the
/// [`SchemeRegistry`](crate::scheme_registry::SchemeRegistry).
pub const LUNCO_SCHEME: &str = "lunco";

/// The library-relative path of a `lunco://<rel>` reference, or `None` for a bare
/// or differently-schemed one. Unlike [`crate::engine_asset_rel`] (which treats a
/// bare path as already-relative), this distinguishes "explicitly addressed to
/// the engine library" — what a caller re-rooting an id back onto disk needs.
pub fn parse_lunco_uri(uri: &str) -> Option<&str> {
    let (scheme, rel) = crate::asset_path::split_scheme(uri)?;
    (scheme == LUNCO_SCHEME).then_some(rel)
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

/// Map a canonical asset id to its native byte source.
///
/// Standard file URIs decode through `lunco-storage`; malformed URIs return
/// `InvalidInput`. Library identities require their explicit `assets_root`.
/// Raw native absolute paths remain accepted. Source-relative identities have
/// no implicit filesystem root and return `None`.
pub fn id_to_disk_path(id: &str, assets_root: Option<&Path>) -> std::io::Result<Option<PathBuf>> {
    if let Some(path) = lunco_storage::file_uri_to_path(id).map_err(|error| match error {
        lunco_storage::StorageError::Io(error) => error,
        error => std::io::Error::other(error.to_string()),
    })? {
        return Ok(Some(path));
    }
    match parse_lunco_uri(id) {
        Some(rel) => Ok(assets_root
            .zip(crate::asset_path::relative_path(rel))
            .map(|(root, rel)| root.join(rel))),
        None => {
            let p = PathBuf::from(id);
            // A scheme-less relative id has no owning root. Do not invent one
            // from the Unix filesystem root: that is wrong on Windows and
            // would make an unanchored reference depend on the host CWD.
            Ok(p.is_absolute().then_some(p))
        }
    }
}

/// Read bytes for a canonical asset identity through the native asset-location
/// policy. USD composition deliberately calls this instead of touching the
/// filesystem: asset-root selection and diagnostic paths stay owned here.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes(id: &str, assets_root: Option<&Path>) -> std::io::Result<Vec<u8>> {
    read_asset_bytes_inner(id, assets_root, None)
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
) -> std::io::Result<Vec<u8>> {
    let Some(rel) = parse_lunco_uri(id) else {
        let path = id_to_disk_path(id, assets_root)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("asset `{id}` has no resolvable native root"),
            )
        })?;
        return read_native_asset(path, max_bytes);
    };
    let default_root;
    let root = match assets_root {
        Some(root) => root,
        None => {
            default_root = crate::assets_dir_abs();
            &default_root
        }
    };
    let roots = crate::library_roots(root);
    let relative = crate::asset_path::relative_path(rel).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe library asset path `{rel}`"),
        )
    })?;
    for root in roots {
        match existing_path_within_root(&root, &relative)? {
            Some(path) => return read_native_asset(path, max_bytes),
            None => continue,
        }
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
    match std::fs::symlink_metadata(&candidate) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_candidate = std::fs::canonicalize(candidate)?;
    if !canonical_candidate.starts_with(canonical_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "asset path resolves outside its owning root",
        ));
    }
    Ok(Some(canonical_candidate))
}

/// Read a canonical asset identity when the composing document belongs to an
/// open Twin.  The synchronous USD composer cannot call Bevy's async reader,
/// so it uses this asset-owned equivalent of the registered `twin://` source.
/// Library assets keep the ordinary `assets_root` policy; Twin assets are
/// resolved inside the explicitly supplied Twin root and its cache.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes_with_twin_root(
    id: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> std::io::Result<Vec<u8>> {
    read_asset_bytes_with_twin_root_inner(id, assets_root, twin_root, None)
}

/// Bounded native read through the same Twin/library source resolution.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_bytes_bounded_with_twin_root(
    id: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
    max_bytes: usize,
) -> std::io::Result<Vec<u8>> {
    read_asset_bytes_with_twin_root_inner(id, assets_root, twin_root, Some(max_bytes))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_asset_bytes_with_twin_root_inner(
    id: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
    max_bytes: Option<usize>,
) -> std::io::Result<Vec<u8>> {
    if let Some((_name, rel)) = crate::parse_twin_uri(id) {
        let root = twin_root.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Twin asset `{id}` has no composing Twin root"),
            )
        })?;
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
        return read_native_asset(path, max_bytes);
    }
    read_asset_bytes_inner(id, assets_root, max_bytes)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn synchronous_twin_reads_share_the_canonical_traversal_guard() {
        let root = tempfile::tempdir().expect("temporary Twin root");
        lunco_storage::write_file_sync(&root.path().join("lesson.rhai"), b"40 + 2")
            .expect("lesson");
        let cached = crate::twin_cache_dir(root.path());
        lunco_storage::ensure_directory_sync(&cached).expect("cache root");
        lunco_storage::write_file_sync(&cached.join("downloaded.rhai"), b"20 + 22")
            .expect("cached lesson");

        let id = "twin://example/lesson.rhai";
        assert_eq!(
            read_asset_bytes_with_twin_root(id, None, Some(root.path())).expect("authored file"),
            b"40 + 2"
        );
        assert_eq!(
            read_asset_bytes_with_twin_root(
                "twin://example/downloaded.rhai",
                None,
                Some(root.path())
            )
            .expect("cached file"),
            b"20 + 22"
        );
        for id in [
            "twin://example/../outside.rhai",
            "twin://example/a/../../outside.rhai",
            r"twin://example/..\outside.rhai",
        ] {
            let error = read_asset_bytes_with_twin_root(id, None, Some(root.path()))
                .expect_err("unsafe Twin path must be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn library_ids_cannot_escape_the_asset_root() {
        let root = tempfile::tempdir().expect("temporary asset root");
        assert_eq!(
            id_to_disk_path("lunco://terrain/moon.usda", Some(root.path())).unwrap(),
            Some(root.path().join("terrain/moon.usda"))
        );
        assert_eq!(
            id_to_disk_path(r"lunco://terrain\moon.usda", Some(root.path())).unwrap(),
            Some(root.path().join("terrain/moon.usda"))
        );
        for id in [
            "lunco://../outside.usda",
            "lunco://terrain/../../outside.usda",
            r"lunco://terrain\..\outside.usda",
        ] {
            assert!(
                id_to_disk_path(id, Some(root.path())).unwrap().is_none(),
                "unsafe library id must be rejected: {id}"
            );
        }
    }

    #[test]
    fn unanchored_relative_ids_are_not_assigned_a_host_root() {
        assert_eq!(id_to_disk_path("scenes/scene.usda", None).unwrap(), None);
        let absolute = std::env::temp_dir().join("lunco-absolute-scene.usda");
        assert_eq!(
            id_to_disk_path(&absolute.to_string_lossy(), None).unwrap(),
            Some(absolute)
        );
    }

    #[test]
    fn file_uri_reader_decodes_native_paths_and_rejects_invalid_input() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("провајдер with spaces.usda");
        lunco_storage::write_file_sync(&source, b"#usda 1.0\n").unwrap();
        let id = lunco_storage::file_path_to_uri(&source).unwrap();
        assert_eq!(id_to_disk_path(&id, None).unwrap(), Some(source));
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
        let error =
            read_asset_bytes_with_twin_root("twin://example/linked.txt", None, Some(root.path()))
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

        let error =
            read_asset_bytes_with_twin_root("twin://example/linked.txt", None, Some(root.path()))
                .expect_err("an unsafe authored path must block cache fallback");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
}

/// Native read for a caller-selected root document. Kept in `lunco-assets-core` so
/// USD consumers never perform their own filesystem access.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Read authored UTF-8 text through the asset boundary.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_asset_file_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read_asset_file_bytes(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Build the `lunco://` [`AssetSourceBuilder`] from the authored library root,
/// its packed cache, and the shared cache in [`crate::library_roots`] order.
///
/// Only the authored `assets/` tree is watched. Cache roots are read-only
/// materialized artifacts and can contain tens of thousands of directories;
/// recursively registering an OS watch for them is both unnecessary for live
/// authoring and can exhaust the process' watch quota before the app starts.
pub fn lunco_asset_source(assets_dir: &Path) -> AssetSourceBuilder {
    let watch_root = assets_dir.to_string_lossy().into_owned();
    let roots = crate::library_roots(assets_dir)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let reader_roots = roots;
    AssetSourceBuilder::new(move || {
        Box::new(FallbackReader {
            readers: reader_roots
                .iter()
                .map(|r| library_asset_reader(r))
                .collect(),
            roots: reader_roots.clone(),
        }) as Box<dyn ErasedAssetReader>
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
        AssetSource::get_default_reader(root.to_string())()
    }
    #[cfg(target_arch = "wasm32")]
    {
        Box::new(
            bevy::asset::io::wasm::HttpWasmAssetReader::new(root).with_request_mapper(|path| {
                std::borrow::Cow::Owned(crate::asset_path::http_asset_path(path))
            }),
        )
    }
}

/// The default browser source uses the same filename transport mapping as
/// `lunco://`, with its configured root and Bevy's platform capabilities.
#[cfg(target_arch = "wasm32")]
pub fn web_asset_source(root: &Path) -> AssetSourceBuilder {
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
        let mut default_source = web_asset_source(Path::new("assets/http-path-fixture/default"));
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
struct FallbackReader {
    readers: Vec<Box<dyn ErasedAssetReader>>,
    /// Parallel to `readers`, kept solely so a miss can say where it looked.
    roots: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn local_root_allows(root: &str, path: &Path) -> std::io::Result<bool> {
    let root = Path::new(root);
    let candidate = root.join(path);
    if !candidate.exists() {
        return Ok(true);
    }
    crate::existing_path_within_root(root, path).map(|resolved| resolved.is_some())
}

#[cfg(target_arch = "wasm32")]
fn local_root_allows(_root: &str, _path: &Path) -> std::io::Result<bool> {
    Ok(true)
}

/// Try each root in order; the first non-`NotFound` answer wins.
macro_rules! try_both {
    ($self:ident, $method:ident, $path:expr_2021) => {{
        // `readers` is non-empty by construction (`assets/` is always first),
        // so the loop always assigns before the unwrap.
        let mut last = None;
        for (reader, root) in $self.readers.iter().zip($self.roots.iter()) {
            match local_root_allows(root, $path) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(AssetReaderError::from(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!(
                            "asset path `{}` escapes library root `{root}`",
                            $path.display()
                        ),
                    )));
                }
                Err(error) => return Err(AssetReaderError::from(error)),
            }
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
            read_asset_bytes_with_twin_root(
                r"twin://fixture\sim\scenes\traverse.usda",
                None,
                Some(root.path())
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
        let readers = roots
            .iter()
            .map(|root| AssetSource::get_default_reader(root.to_string_lossy().into_owned())())
            .collect();
        let reader = FallbackReader {
            readers,
            roots: roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
        };

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
